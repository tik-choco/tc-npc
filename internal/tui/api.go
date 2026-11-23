package tui

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"
)

const requestTimeout = 5 * time.Second

type avatarRef struct {
	Kind string `json:"kind"`
	File string `json:"file"`
}

func (a *avatarRef) UnmarshalJSON(data []byte) error {
	return decodeObject(data, frameField{"kind", &a.Kind, false}, frameField{"file", &a.File, false})
}

type characterSummary struct {
	ID     string     `json:"id"`
	Name   string     `json:"name"`
	Active bool       `json:"active"`
	Avatar *avatarRef `json:"avatar"`
}

func (c *characterSummary) UnmarshalJSON(data []byte) error {
	*c = characterSummary{}
	return decodeObject(data, frameField{"id", &c.ID, false}, frameField{"name", &c.Name, false},
		frameField{"active", &c.Active, false}, frameField{"avatar", &c.Avatar, true})
}

type apiClient struct {
	http *http.Client
	base string
}

func (c apiClient) listCharacters(ctx context.Context) ([]characterSummary, error) {
	const path = "/api/characters"
	body, err := c.requestJSON(ctx, http.MethodGet, path, requestTimeout)
	if err != nil {
		return nil, err
	}
	var characters []characterSummary
	if err := json.Unmarshal(body, &characters); err != nil {
		return nil, fmt.Errorf("parsing GET %s response: %w", path, err)
	}
	if characters == nil {
		return nil, fmt.Errorf("parsing GET %s response: expected a character array", path)
	}
	return characters, nil
}

func (c apiClient) activateCharacter(ctx context.Context, id string) (json.RawMessage, error) {
	return c.requestJSON(ctx, http.MethodPost, "/api/characters/"+encodePathSegment(id)+"/activate", requestTimeout)
}

// Encode all bytes except unreserved characters, so an id always occupies
// one path segment and cannot introduce any URL delimiters.
func encodePathSegment(s string) string {
	const hex = "0123456789ABCDEF"
	var out strings.Builder
	for i := 0; i < len(s); i++ {
		b := s[i]
		switch {
		case 'A' <= b && b <= 'Z', 'a' <= b && b <= 'z', '0' <= b && b <= '9', b == '-', b == '.', b == '_', b == '~':
			out.WriteByte(b)
		default:
			out.WriteByte('%')
			out.WriteByte(hex[b>>4])
			out.WriteByte(hex[b&15])
		}
	}
	return out.String()
}

func (c apiClient) requestJSON(ctx context.Context, method, path string, timeout time.Duration) (json.RawMessage, error) {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	response, err := c.readResponse(ctx, method, path)
	if errors.Is(ctx.Err(), context.DeadlineExceeded) {
		return nil, fmt.Errorf("npc-server did not respond to %s %s within %s (is `tc-npc serve` running and responsive at %s?): %w",
			method, path, timeout, c.base, ctx.Err())
	}
	return response, err
}

func (c apiClient) readResponse(ctx context.Context, method, path string) (json.RawMessage, error) {
	req, err := http.NewRequestWithContext(ctx, method, c.base+path, nil)
	if err != nil {
		return nil, fmt.Errorf("creating %s %s request: %w", method, path, err)
	}
	resp, err := c.http.Do(req)
	if err != nil {
		if errors.Is(err, io.ErrUnexpectedEOF) {
			return nil, fmt.Errorf("malformed HTTP response to %s %s: %w", method, path, err)
		}
		return nil, fmt.Errorf("calling %s %s: %w", method, path, err)
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, fmt.Errorf("reading %s %s response: %w", method, path, err)
	}

	if resp.StatusCode < 200 || resp.StatusCode >= 300 {
		detail := strings.TrimSpace(strings.ToValidUTF8(string(body), "�"))
		var failure struct {
			Error *string `json:"error"`
		}
		if json.Unmarshal(body, &failure) == nil && failure.Error != nil {
			detail = *failure.Error
		}
		if detail != "" {
			return nil, fmt.Errorf("%s %s failed: HTTP %d (%s)", method, path, resp.StatusCode, detail)
		}
		return nil, fmt.Errorf("%s %s failed: HTTP %d", method, path, resp.StatusCode)
	}
	if len(body) == 0 {
		return json.RawMessage("null"), nil
	}
	var raw json.RawMessage
	if err := json.Unmarshal(body, &raw); err != nil {
		return nil, fmt.Errorf("parsing JSON response body (%d bytes): %w", len(body), err)
	}
	return raw, nil
}
