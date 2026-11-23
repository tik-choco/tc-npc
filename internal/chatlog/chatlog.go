// Package chatlog stores the raw chat transcript as JSONL in chronological order.
package chatlog

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"unicode/utf8"
)

// ChatLogEntry is one chat turn. Older entries may have no speaker.
type ChatLogEntry struct {
	Time    string  `json:"time"`
	Speaker *string `json:"speaker,omitempty"`
	Input   string  `json:"input"`
	Output  string  `json:"output"`
}

func (e *ChatLogEntry) UnmarshalJSON(data []byte) error {
	var fields struct {
		Time    *string `json:"time"`
		Speaker *string `json:"speaker"`
		Input   *string `json:"input"`
		Output  *string `json:"output"`
	}
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	if fields.Time == nil || fields.Input == nil || fields.Output == nil {
		return fmt.Errorf("chat log entry requires time, input and output")
	}
	*e = ChatLogEntry{Time: *fields.Time, Speaker: fields.Speaker, Input: *fields.Input, Output: *fields.Output}
	return nil
}

const maxLines = 5000

// Cache counts per path so appending normally needs no recount. Hold the
// same lock across writes and rotation so a concurrent append cannot land
// in the file about to be replaced.
var appendMu sync.Mutex
var lineCounts = make(map[string]int)

func chatLogPath(dataDir string) string {
	return filepath.Join(dataDir, "chat-log.jsonl")
}

// Append writes one turn and gives the log a chance to trim its oldest lines.
func Append(dataDir string, entry *ChatLogEntry) error {
	return appendEntry(dataDir, entry, maxLines)
}

func appendEntry(dataDir string, entry *ChatLogEntry, retention int) error {
	line, err := json.Marshal(entry)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(dataDir, 0o777); err != nil {
		return err
	}

	appendMu.Lock()
	defer appendMu.Unlock()
	path := chatLogPath(dataDir)
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o666)
	if err != nil {
		return err
	}
	_, writeErr := file.Write(append(line, '\n'))
	if err := errors.Join(writeErr, file.Close()); err != nil {
		return err
	}
	return trackAppendAndMaybeRotate(path, retention)
}

// LoadRecent returns up to limit valid turns, oldest first. Logs may
// contain unusually long turns and corrupt lines left by interrupted writes.
func LoadRecent(dataDir string, limit int) []ChatLogEntry {
	entries := []ChatLogEntry{}
	if limit <= 0 {
		return entries
	}
	data, err := readLog(chatLogPath(dataDir))
	if err != nil {
		return entries
	}

	for line := range strings.SplitSeq(data, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		var entry ChatLogEntry
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			slog.Warn("skipping unreadable chat log line", "error", err)
			continue
		}
		entries = append(entries, entry)
	}
	if len(entries) > limit {
		entries = entries[len(entries)-limit:]
	}
	return entries
}

func readLog(path string) (string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return "", err
	}
	if !utf8.Valid(data) {
		return "", fmt.Errorf("%s contains invalid UTF-8", path)
	}
	return string(data), nil
}

func countLines(path string) (int, error) {
	data, err := readLog(path)
	if errors.Is(err, os.ErrNotExist) {
		return 0, nil
	}
	if err != nil {
		return 0, err
	}
	lines := strings.Count(data, "\n")
	if data != "" && !strings.HasSuffix(data, "\n") {
		lines++
	}
	return lines, nil
}

// A cached undercount from an external write only delays rotation. Once the
// count crosses twice the retention, maybeRotate measures the actual file.
func trackAppendAndMaybeRotate(path string, retention int) error {
	count, cached := lineCounts[path]
	if cached {
		count++
	} else {
		// An uncached measurement already includes the line just appended.
		var err error
		count, err = countLines(path)
		if err != nil {
			return err
		}
	}
	if count > 2*retention {
		var err error
		count, err = maybeRotate(path, retention)
		if err != nil {
			return err
		}
	}
	lineCounts[path] = count
	return nil
}

func maybeRotate(path string, retention int) (int, error) {
	data, err := readLog(path)
	if err != nil {
		return 0, err
	}
	lines := strings.Split(data, "\n")
	if lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	if len(lines) <= 2*retention {
		return len(lines), nil
	}

	tmp, err := os.CreateTemp(filepath.Dir(path), ".chat-log-*.tmp")
	if err != nil {
		return 0, err
	}
	defer os.Remove(tmp.Name())

	writer := bufio.NewWriter(tmp)
	for i := len(lines) - retention; i < len(lines); i++ {
		line := lines[i]
		if i < len(lines)-1 || strings.HasSuffix(data, "\n") {
			line = strings.TrimSuffix(line, "\r")
		}
		if _, err := writer.WriteString(line + "\n"); err != nil {
			tmp.Close()
			return 0, err
		}
	}
	if err := errors.Join(writer.Flush(), tmp.Close()); err != nil {
		return 0, err
	}
	if err := os.Rename(tmp.Name(), path); err != nil {
		return 0, err
	}
	return retention, nil
}
