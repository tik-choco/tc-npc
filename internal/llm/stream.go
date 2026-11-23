package llm

import (
	"bufio"
	"context"
	"encoding/json"
	"io"
	"math"
	"strings"
)

// ChatChunk contains text and tool-call fragments from the first choice of
// one SSE data line. A chunk has nonempty Content, ToolCalls, or both.
type ChatChunk struct {
	Content   string          `json:"content,omitempty"`
	ToolCalls []ToolCallDelta `json:"tool_calls,omitempty"`
}

// ToolCallDelta is a fragment of an assistant tool call. Index groups
// fragments for the same call across chunks.
type ToolCallDelta struct {
	Index    uint32             `json:"index"`
	ID       *string            `json:"id,omitempty"`
	Type     *string            `json:"type,omitempty"`
	Function *FunctionCallDelta `json:"function,omitempty"`
}

// FunctionCallDelta carries partial function data. Concatenate Arguments
// fragments in order to obtain the JSON-encoded arguments.
type FunctionCallDelta struct {
	Name      *string `json:"name,omitempty"`
	Arguments *string `json:"arguments,omitempty"`
}

// ChatStream reads a streaming chat completion one chunk at a time. Consume
// it with Next and defer Close in case iteration stops early. Next and Close
// are for a single consumer. Cancel the request context to interrupt a read.
type ChatStream struct {
	ctx    context.Context
	body   io.ReadCloser
	reader *bufio.Reader
}

// ChatStream posts a streaming chat completion request and checks the HTTP
// status before returning an iterator over the response body.
func (c *Client) ChatStream(ctx context.Context, req ChatRequest) (*ChatStream, error) {
	resp, err := c.startChat(ctx, req, true)
	if err != nil {
		return nil, err
	}
	return &ChatStream{ctx: ctx, body: resp.Body, reader: bufio.NewReader(resp.Body)}, nil
}

// Next returns the next text or tool-call chunk. It closes the response on
// [DONE], EOF, or error. [DONE] and a clean end of the body return io.EOF.
func (s *ChatStream) Next() (ChatChunk, error) {
	if s.body == nil {
		return ChatChunk{}, io.EOF
	}
	chunk, err := s.next()
	if err != nil {
		_ = s.Close()
	}
	return chunk, err
}

func (s *ChatStream) next() (ChatChunk, error) {
	for {
		if err := s.ctx.Err(); err != nil {
			return ChatChunk{}, &RequestError{Err: err}
		}
		// Read complete lines regardless of HTTP chunk or UTF-8 rune boundaries.
		// Like the Rust reader, only process newline-terminated data lines.
		line, err := s.reader.ReadString('\n')
		if ctxErr := s.ctx.Err(); ctxErr != nil {
			return ChatChunk{}, &RequestError{Err: ctxErr}
		}
		if err == io.EOF {
			return ChatChunk{}, io.EOF
		}
		if err != nil {
			return ChatChunk{}, &RequestError{Err: err}
		}
		data, ok := strings.CutPrefix(strings.TrimSpace(line), "data:")
		if !ok {
			continue
		}
		data = strings.TrimSpace(data)
		if data == "[DONE]" {
			return ChatChunk{}, io.EOF
		}
		var value any
		if err := json.Unmarshal([]byte(data), &value); err != nil {
			return ChatChunk{}, &ParseError{Err: err}
		}
		chunk := extractChatChunk(value)
		if chunk.Content != "" || len(chunk.ToolCalls) != 0 {
			return chunk, nil
		}
	}
}

// Close closes the response body if iteration ends early. It is idempotent.
func (s *ChatStream) Close() error {
	if s.body == nil {
		return nil
	}
	err := s.body.Close()
	s.body = nil
	s.reader = nil
	if err != nil {
		return &RequestError{Err: err}
	}
	return nil
}

func extractChatChunk(value any) ChatChunk {
	root, _ := value.(map[string]any)
	choices, _ := root["choices"].([]any)
	if len(choices) == 0 {
		return ChatChunk{}
	}
	choice, _ := choices[0].(map[string]any)
	delta, _ := choice["delta"].(map[string]any)
	content, _ := delta["content"].(string)
	chunk := ChatChunk{Content: content}
	tools, _ := delta["tool_calls"].([]any)
	for _, value := range tools {
		tool, ok := value.(map[string]any)
		if !ok {
			continue
		}
		var call ToolCallDelta
		if rawIndex, exists := tool["index"]; exists {
			index, ok := rawIndex.(float64)
			if !ok || index < 0 || index > math.MaxUint32 || index != math.Trunc(index) {
				continue
			}
			call.Index = uint32(index)
		}
		call.ID = stringPointer(tool["id"])
		call.Type = stringPointer(tool["type"])
		if function, ok := tool["function"].(map[string]any); ok {
			call.Function = &FunctionCallDelta{
				Name:      stringPointer(function["name"]),
				Arguments: stringPointer(function["arguments"]),
			}
		}
		chunk.ToolCalls = append(chunk.ToolCalls, call)
	}
	return chunk
}

func stringPointer(value any) *string {
	if text, ok := value.(string); ok {
		return &text
	}
	return nil
}
