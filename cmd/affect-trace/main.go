// affect-trace replays the emotion evaluation dataset through the live
// affect model. See eval/emotion/CONTRACT.md for the dataset and trace shape.
package main

import (
	"bytes"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/talk"
)

const (
	defaultDataset = "eval/emotion/dataset.jsonl"
	defaultOutput  = "eval/emotion/trace.json"
	turnStep       = 10 * time.Second
)

type datasetConversation struct {
	ID    *string       `json:"id"`
	Turns []datasetTurn `json:"turns"`
}

type datasetTurn struct {
	Speaker *string `json:"speaker"`
	Text    *string `json:"text"`
}

type trace struct {
	Conversations []conversationTrace `json:"conversations"`
}

type conversationTrace struct {
	ID    string      `json:"id"`
	Turns []turnTrace `json:"turns"`
}

type turnTrace struct {
	Index   int                 `json:"index"`
	Speaker *string             `json:"speaker"`
	Text    string              `json:"text"`
	Affect  talk.AffectSnapshot `json:"affect"`
}

func main() {
	datasetPath, outputPath := defaultDataset, defaultOutput
	if len(os.Args) > 1 {
		datasetPath = os.Args[1]
	}
	if len(os.Args) > 2 {
		outputPath = os.Args[2]
	}
	if err := run(datasetPath, outputPath); err != nil {
		fmt.Fprintln(os.Stderr, "Error:", err)
		os.Exit(1)
	}
}

func run(datasetPath, outputPath string) error {
	data, err := os.ReadFile(datasetPath)
	if err != nil {
		return fmt.Errorf("reading dataset %s: %w", datasetPath, err)
	}
	if !utf8.Valid(data) {
		return fmt.Errorf("reading dataset %s: invalid UTF-8", datasetPath)
	}

	traced := trace{Conversations: []conversationTrace{}}
	turnCount := 0
	absenceTimeout := time.Duration(config.DefaultAffectConfig().AbsenceTimeoutSecs) * time.Second
	for line := range strings.SplitSeq(string(data), "\n") {
		line = strings.TrimSuffix(line, "\r")
		if strings.TrimSpace(line) == "" {
			continue
		}
		var conversation datasetConversation
		if err := json.Unmarshal([]byte(line), &conversation); err != nil {
			return fmt.Errorf("parsing dataset line: %s: %w", line, err)
		}
		if conversation.ID == nil || conversation.Turns == nil {
			return fmt.Errorf("parsing dataset line: %s: conversation requires id and turns", line)
		}
		affect := talk.NewPartnerAffect()
		start := time.Now()
		turns := make([]turnTrace, 0, len(conversation.Turns))
		for index, turn := range conversation.Turns {
			if turn.Text == nil {
				return fmt.Errorf("parsing dataset line: %s: turn %d requires text", line, index)
			}
			// Follow the engine's order so snapshots see the new partner's state.
			now := start.Add(time.Duration(index) * turnStep)
			affect.CheckAbsence(now, absenceTimeout)
			affect.Observe(turn.Speaker)
			affect.Update(*turn.Text, now)
			turns = append(turns, turnTrace{index, turn.Speaker, *turn.Text, affect.Snapshot()})
		}
		turnCount += len(turns)
		traced.Conversations = append(traced.Conversations, conversationTrace{*conversation.ID, turns})
	}

	var output bytes.Buffer
	encoder := json.NewEncoder(&output)
	encoder.SetEscapeHTML(false)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(traced); err != nil {
		return err
	}
	parent := filepath.Dir(outputPath)
	if err := os.MkdirAll(parent, 0o777); err != nil {
		return fmt.Errorf("creating %s: %w", parent, err)
	}
	serialized := unescapeLineSeparators(bytes.TrimSuffix(output.Bytes(), []byte("\n")))
	if err := os.WriteFile(outputPath, serialized, 0o666); err != nil {
		return fmt.Errorf("writing trace %s: %w", outputPath, err)
	}
	fmt.Printf("%d conversations, %d turns -> %s\n", len(traced.Conversations), turnCount, outputPath)
	return nil
}

// Serde leaves Unicode line separators literal. Go escapes them even
// with HTML escaping disabled. Skip escaped backslashes so text such as
// a literal \u2028 stays intact.
func unescapeLineSeparators(data []byte) []byte {
	out := data[:0]
	for i := 0; i < len(data); i++ {
		if data[i] != '\\' || i+1 == len(data) {
			out = append(out, data[i])
			continue
		}
		if i+5 < len(data) && string(data[i+1:i+5]) == "u202" && (data[i+5] == '8' || data[i+5] == '9') {
			out = utf8.AppendRune(out, '\u2028'+rune(data[i+5]-'8'))
			i += 5
		} else {
			out = append(out, data[i], data[i+1])
			i++
		}
	}
	return out
}
