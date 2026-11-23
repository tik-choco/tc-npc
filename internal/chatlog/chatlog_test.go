package chatlog

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func newEntry(input, output string) ChatLogEntry {
	return ChatLogEntry{Time: time.Now().UTC().Format(time.RFC3339Nano), Input: input, Output: output}
}

func TestAppendAndLoadRoundTrip(t *testing.T) {
	dir := t.TempDir()
	e := newEntry("こんにちは", "やあ")
	speaker := "太郎"
	e.Speaker = &speaker
	if err := Append(dir, &e); err != nil {
		t.Fatal(err)
	}
	loaded := LoadRecent(dir, 10)
	if len(loaded) != 1 || loaded[0].Input != e.Input || loaded[0].Output != e.Output ||
		loaded[0].Time != e.Time || loaded[0].Speaker == nil || *loaded[0].Speaker != speaker {
		t.Errorf("loaded %+v", loaded)
	}
}

func TestLoadRecentTruncatesToTail(t *testing.T) {
	dir := t.TempDir()
	for i := 0; i < 5; i++ {
		e := numberedEntry(i)
		if err := Append(dir, &e); err != nil {
			t.Fatal(err)
		}
	}
	checkInputs(t, LoadRecent(dir, 2), "in3", "in4")
}

func TestLoadMissingLogIsEmpty(t *testing.T) {
	checkInputs(t, LoadRecent(t.TempDir(), 10))
}

func TestLoadRecentSkipsCorruptLinesAndKeepsRest(t *testing.T) {
	dir := t.TempDir()
	first, second := newEntry("good1", "ok1"), newEntry("good2", "ok2")
	if err := Append(dir, &first); err != nil {
		t.Fatal(err)
	}
	appendRaw(t, chatLogPath(dir), []byte("{ not json\n"))
	if err := Append(dir, &second); err != nil {
		t.Fatal(err)
	}
	checkInputs(t, LoadRecent(dir, 10), "good1", "good2")
}

func TestRotationTrimsToMostRecentMaxLines(t *testing.T) {
	dir := t.TempDir()
	path := chatLogPath(dir)
	for i := 0; i < 10; i++ {
		line, err := json.Marshal(numberedEntry(i))
		if err != nil {
			t.Fatal(err)
		}
		appendRaw(t, path, append(line, '\n'))
	}
	if count, err := maybeRotate(path, 3); count != 3 || err != nil {
		t.Fatalf("rotate left %d lines, %v", count, err)
	}
	checkInputs(t, LoadRecent(dir, 100), "in7", "in8", "in9")
}

func TestRotationIsNoOpBelowThreshold(t *testing.T) {
	dir := t.TempDir()
	e := newEntry("only", "one")
	if err := Append(dir, &e); err != nil {
		t.Fatal(err)
	}
	if count, err := maybeRotate(chatLogPath(dir), maxLines); count != 1 || err != nil {
		t.Fatalf("rotate left %d lines, %v", count, err)
	}
	checkInputs(t, LoadRecent(dir, 10), "only")
}

func TestTrackedAppendRotatesOnceThresholdIsCrossed(t *testing.T) {
	dir := t.TempDir()
	for i := 0; i < 7; i++ {
		e := numberedEntry(i)
		if err := appendEntry(dir, &e, 3); err != nil {
			t.Fatal(err)
		}
	}
	checkInputs(t, LoadRecent(dir, 100), "in4", "in5", "in6")
}

func TestGateDoesNotRereadLogOnceCacheIsWarm(t *testing.T) {
	dir := t.TempDir()
	path := chatLogPath(dir)
	e := numberedEntry(0)
	if err := appendEntry(dir, &e, 5); err != nil {
		t.Fatal(err)
	}
	// An actual recount would fail on this invalid UTF-8.
	appendRaw(t, path, []byte{0xff, 0xfe, '\n'})
	e = numberedEntry(1)
	if err := appendEntry(dir, &e, 5); err != nil {
		t.Fatal(err)
	}
}

func TestAppendOmitsAbsentSpeaker(t *testing.T) {
	dir := t.TempDir()
	e := ChatLogEntry{Time: "2026-01-02T00:00:00Z", Input: "hi", Output: "hello"}
	if err := Append(dir, &e); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(chatLogPath(dir))
	const want = `{"time":"2026-01-02T00:00:00Z","input":"hi","output":"hello"}` + "\n"
	if err != nil || string(data) != want {
		t.Errorf("wrote %q, %v; want %q", data, err, want)
	}
}

func numberedEntry(i int) ChatLogEntry {
	return newEntry(fmt.Sprintf("in%d", i), fmt.Sprintf("out%d", i))
}

func checkInputs(t *testing.T, got []ChatLogEntry, want ...string) {
	t.Helper()
	if len(got) != len(want) {
		t.Fatalf("loaded %+v; want inputs %q", got, want)
	}
	for i, input := range want {
		if got[i].Input != input {
			t.Errorf("entry %d input = %q; want %q", i, got[i].Input, input)
		}
	}
}

func appendRaw(t *testing.T, path string, data []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		t.Fatal(err)
	}
	_, writeErr := file.Write(data)
	if closeErr := file.Close(); writeErr != nil || closeErr != nil {
		t.Fatalf("append raw: %v, %v", writeErr, closeErr)
	}
}
