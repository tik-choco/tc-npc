package memory

import (
	"math"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"testing"
)

func TestSplitText(t *testing.T) {
	for _, tc := range []struct {
		name, text    string
		size, overlap uint32
		want          []string
	}{
		{"short text", "hello", 512, 64, []string{"hello"}},
		{"overlap", "abcdefghij", 4, 2, []string{"abcd", "cdef", "efgh", "ghij"}},
		{"multibyte", "あいうえおかきくけこ", 4, 0, []string{"あいうえ", "おかきく", "けこ"}},
		{"zero size", "hello world", 0, 0, []string{"hello world"}},
		{"overlap at least size", "abcd", 2, 4, []string{"ab", "bc", "cd"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := splitText(tc.text, tc.size, tc.overlap); !slices.Equal(got, tc.want) {
				t.Errorf("splitText = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestCosineSimilarity(t *testing.T) {
	for _, tc := range []struct {
		name string
		a, b []float32
		want float32
	}{
		{"identical", []float32{1, 2, 3}, []float32{1, 2, 3}, 1},
		{"orthogonal", []float32{1, 0}, []float32{0, 1}, 0},
		{"mismatched lengths", []float32{1, 2}, []float32{1}, 0},
		{"zero vector", []float32{0, 0}, []float32{1, 1}, 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := cosineSimilarity(tc.a, tc.b); math.Abs(float64(got-tc.want)) > 1e-6 {
				t.Errorf("cosineSimilarity = %v, want %v", got, tc.want)
			}
		})
	}
}

func TestCalculateHashIsStableAndSensitive(t *testing.T) {
	const want = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
	first, repeated, other := calculateHash("hello"), calculateHash("hello"), calculateHash("world")
	if first != want || repeated != first || other == first || len(other) != 64 {
		t.Errorf("hashes = %q, %q, %q", first, repeated, other)
	}
}

func TestAddChunkDedupsByHash(t *testing.T) {
	s := loadVectorStore(filepath.Join(t.TempDir(), storeFileName))
	s.addChunk("doc1", "same text", []float32{1, 0}, nil, 0)
	s.addChunk("doc2", "same text", []float32{0, 1}, nil, 1)
	if len(s.records) != 1 || s.records[0].DocID != "doc1" {
		t.Errorf("records = %+v", s.records)
	}
}

func TestSearchFiltersByThresholdAndTruncates(t *testing.T) {
	s := loadVectorStore(filepath.Join(t.TempDir(), storeFileName))
	s.addChunk("d", "a", []float32{1, 0}, nil, 0)
	s.addChunk("d", "b", []float32{0.9, 0.1}, nil, 0)
	s.addChunk("d", "c", []float32{0, 1}, nil, 0)
	query := []float32{1, 0}
	if hits := s.searchForPerson(query, 5, 0.5, nil); len(hits) != 2 || hits[0].text != "a" || hits[1].text != "b" {
		t.Errorf("hits = %+v, want a and b", hits)
	}
	if hits := s.searchForPerson(query, 1, 0.5, nil); len(hits) != 1 || hits[0].text != "a" {
		t.Errorf("top hit = %+v, want a", hits)
	}
}

func TestSearchPersonBonusAppliedBeforeThreshold(t *testing.T) {
	s := loadVectorStore(filepath.Join(t.TempDir(), storeFileName))
	angle := []float32{0.9, 0.43589}
	s.addChunk("d", "taro-linked", angle, map[string]string{"person_id": "taro"}, 0)
	s.addChunk("d", "unlinked-same-angle", angle, nil, 0)
	query := []float32{1, 0}
	hanako, taro := "hanako", "taro"
	for _, id := range []*string{nil, &hanako} {
		if hits := s.searchForPerson(query, 5, 0.93, id); len(hits) != 0 {
			t.Errorf("hits = %+v, want none for person filter %v", hits, id)
		}
	}
	if hits := s.searchForPerson(query, 5, 0.93, &taro); len(hits) != 1 || hits[0].text != "taro-linked" {
		t.Errorf("hits = %+v, want taro-linked", hits)
	}
}

func TestSearchForPersonStillReturnsUnlinkedRecords(t *testing.T) {
	s := loadVectorStore(filepath.Join(t.TempDir(), storeFileName))
	s.addChunk("d", "general fact", []float32{1, 0}, nil, 0)
	taro := "taro"
	if hits := s.searchForPerson([]float32{1, 0}, 5, 0.5, &taro); len(hits) != 1 || hits[0].text != "general fact" {
		t.Errorf("hits = %+v, want general fact", hits)
	}
}

func TestVectorStoreSaveLoadRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), storeFileName)
	s := loadVectorStore(path)
	s.addChunk("doc1", "hello world", []float32{1, 2, 3}, map[string]string{"type": "long_term_memory"}, 42)
	if err := s.save(); err != nil {
		t.Fatal(err)
	}
	if got := loadVectorStore(path).records; !reflect.DeepEqual(got, s.records) {
		t.Errorf("reloaded = %+v, want %+v", got, s.records)
	}
}

func TestReadLongTermRecordsProjectsRustStore(t *testing.T) {
	dir := t.TempDir()
	if got := ReadLongTermRecords(dir); got == nil || len(got) != 0 {
		t.Fatalf("missing store = %+v, want non-nil empty slice", got)
	}
	path := filepath.Join(dir, storeFileName)
	const rustStore = `[
		{"doc_id":"older","hash":"abc","text":"hello","embedding":[1.0,0.5],"created_at":42},
		{"doc_id":"newer","hash":"def","text":"太郎さん","embedding":[0.0,1.0],"metadata":{"person_id":"p1"},"created_at":43},
		{"doc_id":"tie","hash":"123","text":"same second","embedding":[],"metadata":{},"created_at":43}
	]`
	if err := os.WriteFile(path, []byte(rustStore), 0o600); err != nil {
		t.Fatal(err)
	}
	s := loadVectorStore(path)
	if len(s.records) != 3 || s.records[0].Metadata == nil || !slices.Equal(s.records[0].Embedding, []float32{1, 0.5}) {
		t.Fatalf("Rust vector records = %+v", s.records)
	}
	if err := s.save(); err != nil {
		t.Fatalf("replace existing store: %v", err)
	}
	want := []LongTermRecord{
		{DocID: "newer", Text: "太郎さん", CreatedAt: 43, Metadata: map[string]string{"person_id": "p1"}},
		{DocID: "tie", Text: "same second", CreatedAt: 43, Metadata: map[string]string{}},
		{DocID: "older", Text: "hello", CreatedAt: 42, Metadata: map[string]string{}},
	}
	if got := ReadLongTermRecords(dir); !reflect.DeepEqual(got, want) {
		t.Errorf("REST memory records = %+v, want %+v", got, want)
	}
	// The server projection still works when the ignored vector data cannot
	// be decoded as embeddings.
	if err := os.WriteFile(path, []byte(`[{"doc_id":"d","text":"visible","created_at":1,"embedding":false}]`), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := ReadLongTermRecords(dir); len(got) != 1 || got[0].Text != "visible" {
		t.Errorf("projected records = %+v", got)
	}
	if err := os.WriteFile(path, []byte(`[{"doc_id":"d","created_at":1}]`), 0o600); err != nil {
		t.Fatal(err)
	}
	if got := ReadLongTermRecords(dir); got == nil || len(got) != 0 {
		t.Errorf("malformed store = %+v, want non-nil empty slice", got)
	}
}
