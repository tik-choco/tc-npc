package memory

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"math"
	"os"
	"path/filepath"
	"sort"
	"sync"
	"unicode/utf8"
)

const (
	storeFileName    = "memory-store.json"
	personMatchBonus = float32(0.05)
)

// LongTermRecord is the text and bookkeeping for a stored chunk.
type LongTermRecord struct {
	DocID     string            `json:"doc_id"`
	Text      string            `json:"text"`
	Metadata  map[string]string `json:"metadata"`
	CreatedAt int64             `json:"created_at"` // Unix seconds.
}

// ReadLongTermRecords reads the store's text records, newest first. A missing,
// unreadable, or malformed store yields an empty slice. Callers can apply
// their own cap and filter by Metadata["person_id"].
func ReadLongTermRecords(dataDir string) []LongTermRecord {
	path := filepath.Join(dataDir, storeFileName)
	data, err := os.ReadFile(path)
	if err != nil {
		return []LongTermRecord{}
	}
	records, err := decodeRecords[LongTermRecord](data)
	if err != nil {
		slog.Warn("npc-memory: failed to parse memory store, returning empty", "path", path, "error", err)
		return []LongTermRecord{}
	}
	sort.SliceStable(records, func(i, j int) bool {
		return records[i].CreatedAt > records[j].CreatedAt
	})
	return records
}

type record struct {
	LongTermRecord
	Hash      string    `json:"hash"`
	Embedding []float32 `json:"embedding"`
}

// The REST reader and vector store require the same text fields, including
// a metadata default for older files. Only the vector store decodes vectors.
type recordFields struct {
	DocID     *string         `json:"doc_id"`
	Text      *string         `json:"text"`
	Metadata  json.RawMessage `json:"metadata"`
	CreatedAt *int64          `json:"created_at"`
}

func (f recordFields) record() (LongTermRecord, error) {
	if f.DocID == nil || f.Text == nil || f.CreatedAt == nil {
		return LongTermRecord{}, errors.New("memory record requires doc_id, text and created_at")
	}
	metadata := make(map[string]string)
	if f.Metadata != nil {
		var entries map[string]*string
		if err := json.Unmarshal(f.Metadata, &entries); err != nil {
			return LongTermRecord{}, fmt.Errorf("decode memory record metadata: %w", err)
		}
		if entries == nil {
			return LongTermRecord{}, errors.New("memory record metadata must be an object")
		}
		for key, value := range entries {
			if value == nil {
				return LongTermRecord{}, fmt.Errorf("memory record metadata %q must be a string", key)
			}
			metadata[key] = *value
		}
	}
	return LongTermRecord{DocID: *f.DocID, Text: *f.Text, Metadata: metadata, CreatedAt: *f.CreatedAt}, nil
}

func (r *LongTermRecord) UnmarshalJSON(data []byte) error {
	var fields recordFields
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	var err error
	*r, err = fields.record()
	return err
}

func (r *record) UnmarshalJSON(data []byte) error {
	var fields struct {
		recordFields
		Hash      *string    `json:"hash"`
		Embedding []*float32 `json:"embedding"`
	}
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	text, err := fields.record()
	if err != nil {
		return err
	}
	if fields.Hash == nil || fields.Embedding == nil {
		return errors.New("memory record requires hash and embedding")
	}
	embedding := make([]float32, len(fields.Embedding))
	for i, value := range fields.Embedding {
		if value == nil {
			return errors.New("memory record embedding components must be numbers")
		}
		embedding[i] = *value
	}
	*r = record{LongTermRecord: text, Hash: *fields.Hash, Embedding: embedding}
	return nil
}

func decodeRecords[T any](data []byte) ([]T, error) {
	if !utf8.Valid(data) {
		return nil, errors.New("memory store contains invalid UTF-8")
	}
	var records []T
	if err := json.Unmarshal(data, &records); err != nil {
		return nil, err
	}
	if records == nil {
		return nil, errors.New("memory store must be an array")
	}
	return records, nil
}

func calculateHash(text string) string {
	hash := sha256.Sum256([]byte(text))
	return hex.EncodeToString(hash[:])
}

// Split by Unicode characters so chunk boundaries never bisect an encoding.
func splitText(text string, chunkSize, overlap uint32) []string {
	runes := []rune(text)
	if chunkSize == 0 || uint64(len(runes)) <= uint64(chunkSize) {
		return []string{text}
	}
	var chunks []string
	step := max(int(chunkSize-min(chunkSize, overlap)), 1)
	for start := 0; ; start += step {
		end := start + min(int(chunkSize), len(runes)-start)
		chunks = append(chunks, string(runes[start:end]))
		if end == len(runes) {
			return chunks
		}
	}
}

func cosineSimilarity(a, b []float32) float32 {
	if len(a) != len(b) || len(a) == 0 {
		return 0
	}
	var dot, normA, normB float64
	for i, value := range a {
		x, y := float64(value), float64(b[i])
		dot += x * y
		normA += x * x
		normB += y * y
	}
	if normA == 0 || normB == 0 {
		return 0
	}
	return float32(dot / (math.Sqrt(normA) * math.Sqrt(normB)))
}

type searchHit struct {
	text  string
	score float32
}

// The event loop writes whole batches under mu while sense queries read.
type vectorStore struct {
	mu      sync.RWMutex
	path    string
	records []record
}

func loadVectorStore(path string) *vectorStore {
	store := &vectorStore{path: path, records: []record{}}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return store
	}
	if err != nil {
		slog.Warn("npc-memory: failed to read memory store, starting empty", "path", path, "error", err)
		return store
	}
	records, err := decodeRecords[record](data)
	if err != nil {
		slog.Warn("npc-memory: failed to parse memory store, starting empty", "path", path, "error", err)
		return store
	}
	store.records = records
	return store
}

// The caller holds mu across the entire batch and its save.
func (s *vectorStore) addChunk(docID, text string, embedding []float32, metadata map[string]string, createdAt int64) {
	hash := calculateHash(text)
	for _, r := range s.records {
		if r.Hash == hash {
			return
		}
	}
	if metadata == nil {
		metadata = map[string]string{}
	}
	if embedding == nil {
		embedding = []float32{}
	}
	s.records = append(s.records, record{
		LongTermRecord: LongTermRecord{DocID: docID, Text: text, Metadata: metadata, CreatedAt: createdAt},
		Hash:           hash, Embedding: embedding,
	})
}

func (s *vectorStore) searchForPerson(query []float32, topK uint32, threshold float32, personID *string) []searchHit {
	s.mu.RLock()
	var hits []searchHit
	for _, record := range s.records {
		score := cosineSimilarity(query, record.Embedding)
		// Boost matched records before thresholding and ranking, so a near
		// miss can be recalled alongside otherwise relevant general memories.
		if id, attributed := record.Metadata["person_id"]; personID != nil && attributed && id == *personID {
			score += personMatchBonus
		}
		if score >= threshold {
			hits = append(hits, searchHit{text: record.Text, score: score})
		}
	}
	s.mu.RUnlock()
	sort.SliceStable(hits, func(i, j int) bool { return hits[i].score > hits[j].score })
	if uint64(len(hits)) > uint64(topK) {
		return hits[:topK]
	}
	return hits
}

func (s *vectorStore) save() error {
	dir := filepath.Dir(s.path)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return err
	}
	data, err := json.MarshalIndent(s.records, "", "  ")
	if err != nil {
		return fmt.Errorf("encode memory store: %w", err)
	}
	// Write the complete new store beside the old one before replacing it.
	tmp, err := os.CreateTemp(dir, ".memory-store-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	_, writeErr := tmp.Write(data)
	if err := errors.Join(writeErr, tmp.Close()); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), s.path)
}
