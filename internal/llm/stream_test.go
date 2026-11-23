package llm

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func TestChatStream(t *testing.T) {
	tail := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		checkRequest(t, r, http.MethodPost, "/v1/chat/completions", "Bearer key", "application/json")
		checkJSON(t, r.Body, `{"model":"m","messages":[],"reasoning_effort":"none","stream":true}`)
		w.Header().Set("Content-Type", "text/event-stream")
		flush := w.(http.Flusher).Flush
		fmt.Fprint(w, ": keepalive\r\nevent: message\r\n\r\n")
		fmt.Fprint(w, "  data: {\"choices\":[{\"delta\":{\"content\":\"hel\xe6\x97")
		flush()
		fmt.Fprint(w, "\xa5\"}},{\"delta\":{\"content\":\"ignored\"}}]}\r\n\r\n")
		flush()

		select {
		case <-tail:
		case <-r.Context().Done():
			return
		}
		fmt.Fprintln(w, `data: {"choices":[]}`)
		fmt.Fprintln(w, `data: {"choices":[{"delta":{"role":"assistant","content":null}}]}`)
		fmt.Fprintln(w, `data: {"choices":[{"delta":{"tool_calls":[{"index":1,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\":"}},{"index":2,"id":"call_2","function":{"name":"search"}}]}}]}`)
		fmt.Fprint(w, `data: {"choices":[{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"1}"}}]},"finish_reason":"tool_calls"}]}`)
		flush()
		fmt.Fprint(w, "\n\ndata: [DONE]\r\n\r\ndata: invalid JSON\n")
	}))
	defer server.Close()
	ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
	defer cancel()
	req := NewChatRequest("m", nil)
	wantFalse := false
	req.Stream = &wantFalse
	stream, err := New(server.URL, "key").ChatStream(ctx, req)
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	if *req.Stream || req.ReasoningEffort != nil {
		t.Error("ChatStream changed the caller's request")
	}
	if first, err := stream.Next(); err != nil || first.Content != "hel日" || len(first.ToolCalls) != 0 {
		t.Fatalf("first chunk = %+v, %v, want first-choice text", first, err)
	}
	close(tail)

	start, err := stream.Next()
	if err != nil || start.Content != "" || len(start.ToolCalls) != 2 {
		t.Fatalf("tool chunk = %+v, %v", start, err)
	}
	first, second := start.ToolCalls[0], start.ToolCalls[1]
	if first.Index != 1 || first.ID == nil || *first.ID != "call_1" || first.Type == nil || *first.Type != "function" ||
		first.Function == nil || first.Function.Name == nil || *first.Function.Name != "lookup" || first.Function.Arguments == nil {
		t.Fatalf("unexpected first tool fragment %+v", first)
	}
	if second.Index != 2 || second.ID == nil || *second.ID != "call_2" || second.Function == nil || second.Function.Name == nil || *second.Function.Name != "search" {
		t.Errorf("unexpected second tool fragment %+v", second)
	}
	continuation, err := stream.Next()
	if err != nil || len(continuation.ToolCalls) != 1 {
		t.Fatalf("continuation = %+v, %v", continuation, err)
	}
	last := continuation.ToolCalls[0]
	if last.Index != first.Index || last.ID != nil || last.Function == nil || last.Function.Name != nil || last.Function.Arguments == nil {
		t.Fatalf("unexpected tool continuation %+v", last)
	}
	if got := *first.Function.Arguments + *last.Function.Arguments; got != `{"q":1}` {
		t.Errorf("joined arguments = %q", got)
	}
	if _, err := stream.Next(); err != io.EOF {
		t.Errorf("[DONE] = %v, want EOF", err)
	}
	if _, err := stream.Next(); err != io.EOF {
		t.Errorf("Next after [DONE] = %v, want EOF", err)
	}
}

func TestChatStreamLongAndUnterminatedLines(t *testing.T) {
	long := strings.Repeat("x", 70*1024)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		fmt.Fprintf(w, "data: {\"choices\":[{\"delta\":{\"content\":%q}}]}\n", long)
		fmt.Fprint(w, `data: {"choices":[{"delta":{"content":"unterminated"}}]}`)
	}))
	defer server.Close()
	stream, err := New(server.URL, "").ChatStream(t.Context(), NewChatRequest("m", nil))
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	if chunk, err := stream.Next(); err != nil || chunk.Content != long {
		t.Errorf("long line got length %d and error %v", len(chunk.Content), err)
	}
	if _, err := stream.Next(); err != io.EOF {
		t.Errorf("unterminated line = %v, want EOF", err)
	}
}

func TestChatStreamInvalidEvent(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, "data: null\ndata: {\"choices\":[{\"delta\":{\"content\":3}}]}\ndata: {\n")
	}))
	defer server.Close()
	stream, err := New(server.URL, "").ChatStream(t.Context(), NewChatRequest("m", nil))
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	var parseErr *ParseError
	if _, err := stream.Next(); !errors.As(err, &parseErr) {
		t.Errorf("invalid SSE data = %v, want ParseError", err)
	}
	if _, err := stream.Next(); err != io.EOF {
		t.Errorf("Next after ParseError = %v, want EOF", err)
	}
}

func TestChatStreamHTTPError(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusForbidden)
		fmt.Fprint(w, "denied")
	}))
	defer server.Close()
	stream, err := New(server.URL, "").ChatStream(t.Context(), NewChatRequest("m", nil))
	var apiErr *APIError
	if stream != nil || !errors.As(err, &apiErr) || apiErr.Status != http.StatusForbidden || apiErr.Body != "denied" {
		t.Errorf("ChatStream = %v, %v, want APIError with 403 and denied", stream, err)
	}
}

func TestChatStreamCancellation(t *testing.T) {
	serverCanceled := make(chan struct{})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, "data: {\"choices\":[{\"delta\":{\"content\":\"ready\"}}]}\n")
		w.(http.Flusher).Flush()
		<-r.Context().Done()
		close(serverCanceled)
	}))
	defer server.Close()
	deadline, stop := context.WithTimeout(t.Context(), 5*time.Second)
	defer stop()
	ctx, cancel := context.WithCancel(deadline)
	defer cancel()
	stream, err := New(server.URL, "").ChatStream(ctx, NewChatRequest("m", nil))
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	if chunk, err := stream.Next(); err != nil || chunk.Content != "ready" {
		t.Fatalf("first chunk = %+v, %v", chunk, err)
	}
	result := make(chan error, 1)
	go func() {
		_, err := stream.Next()
		result <- err
	}()
	cancel()
	select {
	case err := <-result:
		var requestErr *RequestError
		if !errors.Is(err, context.Canceled) || !errors.As(err, &requestErr) {
			t.Errorf("Next after cancel = %v, want wrapped context.Canceled", err)
		}
	case <-deadline.Done():
		// Join the reader before the deferred Close to preserve single-consumer use.
		<-result
		t.Error("stream read did not stop after cancellation")
	}
	select {
	case <-serverCanceled:
	case <-deadline.Done():
		t.Error("server request did not stop after cancellation")
	}
}
