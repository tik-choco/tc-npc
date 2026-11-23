package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
)

func TestTraceEncoding(t *testing.T) {
	dir := t.TempDir()
	dataset := filepath.Join(dir, "dataset.jsonl")
	output := filepath.Join(dir, "out", "trace.json")
	input := `{"id":"example","turns":[{"speaker":"<\u2029>","text":"あ\u2028い\\u2028"}]}`
	if err := os.WriteFile(dataset, []byte(input+"\n"), 0o666); err != nil {
		t.Fatal(err)
	}
	if err := run(dataset, output); err != nil {
		t.Fatal(err)
	}
	got, err := os.ReadFile(output)
	if err != nil {
		t.Fatal(err)
	}
	if !json.Valid(got) || bytes.HasSuffix(got, []byte("\n")) {
		t.Fatal("trace must be JSON without a trailing newline")
	}
	for _, fragment := range []string{
		"\"speaker\": \"<\u2029>\"",
		"\"partner\": \"<\u2029>\"",
		"\"text\": \"あ\u2028い\\\\u2028\"",
	} {
		if !bytes.Contains(got, []byte(fragment)) {
			t.Errorf("trace missing Rust encoding %q", fragment)
		}
	}
}
