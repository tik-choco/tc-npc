// Package memory logs chat turns, summarizes recent conversations, and
// retrieves long-term memories and person profiles over the bus.
package memory

import (
	"context"
	"fmt"
	"log/slog"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/google/uuid"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
)

// The short-term buffer flushes after this many turns.
const summaryTurnThreshold = 5

type memoryModule struct{}

// New constructs npc-memory. Run resolves the startup configuration.
func New(mc module.Ctx) (module.Module, error) { return memoryModule{}, nil }

func (memoryModule) Name() string { return "npc-memory" }

type memorySession struct {
	bus              *bus.Bus
	dataDir          string
	cfg              config.MemoryConfig
	chat, embed      *llm.Client
	chatModel        string
	embeddingModel   string
	store            *vectorStore
	tracker          *personTracker
	buffer           []string
	summary          string
	currentSpeakerID *string
}

func (memoryModule) Run(ctx context.Context, mc module.Ctx) error {
	// Resolve chat and embedding independently at startup. They share the
	// connection pool when their endpoint and credentials match.
	chat, err := mc.Config.Resolve(config.TaskMemory)
	if err != nil {
		return fmt.Errorf("resolve memory model: %w", err)
	}
	embedding, err := mc.Config.Resolve(config.TaskEmbedding)
	if err != nil {
		return fmt.Errorf("resolve embedding model: %w", err)
	}
	chatClient := llm.New(chat.BaseURL, chat.APIKey).WithReasoningEffort(chat.ReasoningEffort)
	embedClient := chatClient
	if embedding.BaseURL != chat.BaseURL || embedding.APIKey != chat.APIKey {
		embedClient = llm.New(embedding.BaseURL, embedding.APIKey)
	}

	cfg := mc.Config.Memory
	// The summaries become part of npc-talk's context, so they follow the
	// configured language even when the base prompts are in Japanese.
	cfg.SummarizePrompt = config.WithLanguageInstruction(cfg.SummarizePrompt, mc.Config.Language)
	cfg.ConsolidatePrompt = config.WithLanguageInstruction(cfg.ConsolidatePrompt, mc.Config.Language)
	s := memorySession{
		bus: mc.Bus, dataDir: mc.DataDir, cfg: cfg,
		chat: chatClient, embed: embedClient, chatModel: chat.Model, embeddingModel: embedding.Model,
		store: loadVectorStore(filepath.Join(mc.DataDir, storeFileName)),
	}
	if cfg.People.Enabled {
		s.tracker = loadPersonTracker(mc.DataDir, cfg.People.MaxFacts)
	}
	if s.embeddingModel == "" {
		slog.Info("npc-memory: embedding model is empty; RAG and long-term embedding are disabled, short-term summaries still run")
	}
	return s.run(ctx)
}

func (s *memorySession) run(ctx context.Context) error {
	sub := s.bus.Subscribe()
	defer sub.Close()
	idleDuration := time.Duration(max(1, s.cfg.IdleTimeoutMinutes)) * time.Minute
	idle := time.NewTimer(idleDuration)
	defer idle.Stop()
	ctx, cancel := context.WithCancel(ctx)
	var searches sync.WaitGroup
	defer func() {
		cancel()
		searches.Wait()
	}()
	var dropped uint64
	reportDropped := func() {
		if total := sub.Dropped(); total != dropped {
			slog.Warn("npc-memory: bus receiver lagged, some messages were dropped", "skipped", total-dropped)
			dropped = total
		}
	}

	for ctx.Err() == nil {
		select {
		case <-ctx.Done():
		case <-idle.C:
			if ctx.Err() == nil {
				s.flushIdle(ctx)
				idle.Reset(idleDuration)
			}
		case msg, ok := <-sub.C:
			if !ok {
				slog.Warn("npc-memory: bus channel closed, stopping")
				return nil
			}
			reportDropped()
			switch {
			case msg.Topic == bus.TopicChat && msg.Type == bus.MsgChatLog:
				if !s.recordChat(msg) {
					continue
				}
				idle.Reset(idleDuration)
				if len(s.buffer) >= summaryTurnThreshold && ctx.Err() == nil {
					turns := s.buffer
					s.buffer = nil
					s.summarize(ctx, s.summary, turns)
				}
			case msg.Topic == bus.TopicSense && (msg.Type == bus.MsgSpeech || msg.Type == bus.MsgVision):
				if msg.Type == bus.MsgSpeech && s.tracker != nil {
					s.observeSpeaker(parseSpeaker(msg))
				}
				if s.embeddingModel != "" {
					if content := parseSenseContent(msg); content != "" {
						personID := s.currentSpeakerID
						searches.Go(func() { s.handleSense(ctx, content, personID) })
					}
				}
			case msg.Topic == bus.TopicSense && msg.Type == bus.MsgPersonSeen:
				if s.tracker != nil && s.cfg.People.LinkVision {
					s.observeSighting(msg)
				}
			}
		}
	}

	slog.Info("npc-memory: shutdown requested, stopping")
	// A reply's chat_log can race with shutdown. Unsubscribe first so we
	// drain exactly the queued messages and still persist those last turns.
	sub.Close()
	reportDropped()
	for msg := range sub.C {
		if msg.Topic == bus.TopicChat && msg.Type == bus.MsgChatLog {
			s.recordChat(msg)
		}
	}
	// The raw transcript is already on disk, so give the final network
	// flush a bounded window before exiting.
	if len(s.buffer) > 0 && s.embeddingModel != "" {
		flushCtx, stop := context.WithTimeout(context.WithoutCancel(ctx), 10*time.Second)
		s.consolidate(flushCtx, s.summary, s.buffer)
		if flushCtx.Err() == context.DeadlineExceeded {
			slog.Warn("npc-memory: shutdown long-term consolidation timed out, giving up")
		}
		stop()
	}
	return nil
}

func (s *memorySession) recordChat(msg bus.Message) bool {
	// Keep every usable raw turn, even if embedding is disabled or a
	// completion fails. The talk module only emits completed turns.
	if entry := buildChatLogEntry(msg); entry != nil {
		if err := chatlog.Append(s.dataDir, entry); err != nil {
			slog.Warn("npc-memory: failed to append raw chat log", "error", err)
		}
	}
	turn := parseChatLog(msg)
	if turn == "" {
		return false
	}
	s.buffer = append(s.buffer, turn)
	return true
}

func (s *memorySession) observeSpeaker(name string) {
	p := s.tracker.observeFromChat(name)
	if p == nil {
		return
	}
	s.tracker.persistAndPublish(s.bus, p)
	// Update the encounter count every time, but only publish the profile
	// when the speaker changes, so a monologue doesn't spam person_memory.
	if s.currentSpeakerID == nil || *s.currentSpeakerID != p.ID {
		id := p.ID
		s.currentSpeakerID = &id
		s.bus.Publish(bus.TopicMem, bus.MsgPersonMemory, map[string]string{
			"person_id": id, "name": p.Name, "content": profileText(p),
		})
	}
}

func (s *memorySession) observeSighting(msg bus.Message) {
	if seen := parsePersonSeen(msg); seen != nil {
		if p := s.tracker.observeFromVision(string(seen.Name), string(seen.Appearance), string(seen.Note)); p != nil {
			s.tracker.persistAndPublish(s.bus, p)
		}
	}
}

func (s *memorySession) flushIdle(ctx context.Context) {
	if len(s.buffer) == 0 {
		return
	}
	slog.Debug("npc-memory: idle timeout reached, summarizing and consolidating")
	turns, prevSummary := s.buffer, s.summary
	s.buffer = nil
	s.summarize(ctx, prevSummary, turns)
	if s.embeddingModel != "" {
		s.consolidate(ctx, prevSummary, turns)
	}
	if s.tracker != nil {
		s.extractPersonFacts(ctx, turns)
	}
}

func buildPrompt(base, prevSummary string, turns []string) string {
	return base + "\nPrevious summary: " + prevSummary + "\nNew conversations:\n" + strings.Join(turns, "\n")
}

func (s *memorySession) complete(ctx context.Context, prompt string) *string {
	req := llm.NewChatRequest(s.chatModel, []llm.ChatMessage{llm.UserMessage(prompt)})
	resp, err := s.chat.Chat(ctx, req)
	if err != nil {
		slog.Warn("npc-memory: llm completion failed", "error", err)
		return nil
	}
	return resp.Content()
}

func (s *memorySession) summarize(ctx context.Context, prevSummary string, turns []string) {
	if text := s.complete(ctx, buildPrompt(s.cfg.SummarizePrompt, prevSummary, turns)); text != nil {
		s.summary = *text
		s.bus.Publish(bus.TopicMem, bus.MsgShortTermMemory, map[string]string{"content": *text})
	}
}

func (s *memorySession) consolidate(ctx context.Context, prevSummary string, turns []string) {
	text := s.complete(ctx, buildPrompt(s.cfg.ConsolidatePrompt, prevSummary, turns))
	if text == nil || strings.TrimSpace(*text) == "" {
		return
	}
	chunks := splitText(*text, s.cfg.ChunkSize, s.cfg.ChunkOverlap)
	embeddings, err := s.embed.Embed(ctx, s.embeddingModel, chunks)
	if err != nil {
		slog.Warn("npc-memory: failed to embed consolidated long-term memory", "error", err)
		return
	}
	if len(embeddings) != len(chunks) {
		slog.Warn("npc-memory: embedding count mismatch, skipping store", "expected", len(chunks), "got", len(embeddings))
		return
	}

	docID, now := uuid.NewString(), time.Now().UTC()
	metadata := map[string]string{"type": bus.MsgLongTermMemory, "timestamp": formatTimestamp(now)}
	// Tag the whole batch so a later query for the speaker can favor it.
	if s.currentSpeakerID != nil {
		metadata["person_id"] = *s.currentSpeakerID
	}
	s.store.mu.Lock()
	for i, chunk := range chunks {
		s.store.addChunk(docID, chunk, embeddings[i], metadata, now.Unix())
	}
	err = s.store.save()
	s.store.mu.Unlock()
	if err != nil {
		slog.Warn("npc-memory: failed to save memory store", "error", err)
	}
	s.bus.Publish(bus.TopicMem, bus.MsgLongTermMemory, map[string]string{"content": *text})
}

func (s *memorySession) handleSense(ctx context.Context, content string, personID *string) {
	embeddings, err := s.embed.Embed(ctx, s.embeddingModel, []string{content})
	if err != nil {
		slog.Warn("npc-memory: failed to embed sense query for RAG search", "error", err)
		return
	}
	if len(embeddings) == 0 {
		return
	}
	hits := s.store.searchForPerson(embeddings[0], s.cfg.TopK, s.cfg.Threshold, personID)
	if len(hits) == 0 || ctx.Err() != nil {
		return
	}
	texts := make([]string, len(hits))
	for i, hit := range hits {
		texts[i] = hit.text
	}
	s.bus.Publish(bus.TopicMem, bus.MsgLongTermMemory, map[string]string{"content": strings.Join(texts, "\n")})
}

func (s *memorySession) extractPersonFacts(ctx context.Context, turns []string) {
	// Extract facts from the turns themselves, without the carried summary.
	response := s.complete(ctx, s.cfg.People.ExtractPrompt+"\n\n"+strings.Join(turns, "\n"))
	if response == nil {
		return
	}
	extracted := parsePersonFactsResponse(*response)
	if extracted == nil {
		slog.Warn("npc-memory: failed to parse person fact extraction response, skipping", "response", *response)
		return
	}
	for _, entry := range extracted {
		if len(entry.Facts) == 0 {
			continue
		}
		if p := s.tracker.observeFromChat(entry.Name); p != nil {
			s.tracker.addFacts(p.ID, entry.Facts, "chat")
			s.tracker.persistAndPublish(s.bus, p)
		}
	}
}
