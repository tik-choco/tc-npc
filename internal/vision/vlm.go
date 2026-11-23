package vision

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"slices"
	"strings"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/llm"
)

const (
	robotTool  = "robot_command"
	personTool = "observe_person"
	maxTokens  = 500
)

func analyzeImage(ctx context.Context, client *llm.Client, model, systemPrompt, dataURL, memory string, b *bus.Bus, personDetection bool) (string, error) {
	userText := "現在の視界を分析し、必要に応じて行動を選択してください。"
	if memory != "" {
		userText = "【短期記憶】: " + memory + "\n\n" + userText
	}
	messages := []llm.ChatMessage{
		llm.SystemMessage(systemPrompt),
		llm.UserPartsMessage([]llm.ContentPart{llm.ImageDataURLPart(dataURL), llm.TextPart(userText)}),
	}
	tokenLimit := uint32(maxTokens)
	req := llm.NewChatRequest(model, messages)
	req.MaxTokens = &tokenLimit
	req.Tools = buildTools(personDetection)

	resp, err := client.Chat(ctx, req)
	if err != nil {
		return "", err
	}
	if len(resp.Choices) == 0 {
		return "", fmt.Errorf("no response from vlm")
	}
	if err := ctx.Err(); err != nil {
		return "", err
	}

	initial := resp.Choices[0].Message
	text := ""
	if initial.Content != nil {
		text = *initial.Content
	}
	if len(initial.ToolCalls) == 0 {
		return text, nil
	}

	assistant := llm.AssistantMessage(text)
	assistant.ToolCalls = initial.ToolCalls
	messages = append(messages, assistant)
	results := make([]string, 0, len(initial.ToolCalls))
	for _, call := range initial.ToolCalls {
		result := executeToolCall(call, b)
		results = append(results, result)
		messages = append(messages, llm.ToolResultMessage(call.ID, result))
	}

	// Complete the narrative in a single follow-up without offering tools again.
	followUp := llm.NewChatRequest(model, messages)
	followUp.MaxTokens = &tokenLimit
	final, err := client.Chat(ctx, followUp)
	if err != nil {
		slog.Warn("vision: follow-up completion failed, using fallback narrative", "error", err)
	} else if content := final.Content(); content != nil && *content != "" {
		return *content, nil
	}
	return text + "\n\n【アクション実行結果】\n" + strings.Join(results, "\n"), nil
}

func buildTools(personDetection bool) []llm.Tool {
	tools := []llm.Tool{llm.FunctionTool(
		robotTool,
		"Send a natural language command to the robot to perform actions (move, turn, jump, stop, etc.)",
		json.RawMessage(`{
			"type": "object",
			"properties": {
				"command": {
					"type": "string",
					"description": "The command in natural language, e.g., 'jump', 'move forward 1 meter', 'turn right 90 degrees'"
				}
			},
			"required": ["command"]
		}`),
	)}
	if personDetection {
		tools = append(tools, llm.FunctionTool(
			personTool,
			"Record an observation about a person visible in the image, if any. Call this once per distinct person noticed.",
			json.RawMessage(`{
				"type": "object",
				"properties": {
					"name": {
						"type": "string",
						"description": "The person's name if identifiable from a name tag, caption, or prior context; empty string if unknown"
					},
					"appearance": {
						"type": "string",
						"description": "Short visual description, e.g. 'blue hoodie, short black hair'"
					},
					"note": {
						"type": "string",
						"description": "What this person is doing or any notable detail, if any"
					}
				},
				"required": ["appearance"]
			}`),
		))
	}
	return tools
}

func executeToolCall(call llm.ToolCall, b *bus.Bus) string {
	const errorPrefix = "Error executing tool: "
	switch call.Function.Name {
	case robotTool:
		args, err := toolArguments(call.Function.Arguments, "command", "command")
		if err != nil {
			return errorPrefix + err.Error()
		}
		// The action payload is the bare command string.
		b.Publish(bus.TopicAction, bus.MsgAction, args["command"])
		return "Successfully sent command: " + args["command"]
	case personTool:
		args, err := toolArguments(call.Function.Arguments, "appearance", "name", "appearance", "note")
		if err != nil {
			return errorPrefix + err.Error()
		}
		b.Publish(bus.TopicSense, bus.MsgPersonSeen, map[string]string{
			"name":       args["name"],
			"appearance": args["appearance"],
			"note":       args["note"],
			"source":     "vision",
		})
		return "Recorded observation of person: " + args["appearance"]
	default:
		return fmt.Sprintf("%sunknown tool '%s'", errorPrefix, call.Function.Name)
	}
}

// Read only the declared fields. Serde also accepts a positional array in
// declaration order when decoding these argument structs.
func toolArguments(raw, required string, fields ...string) (map[string]string, error) {
	decoder := json.NewDecoder(strings.NewReader(raw))
	start, err := decoder.Token()
	if err != nil {
		return nil, err
	}
	object := start == json.Delim('{')
	if !object && start != json.Delim('[') {
		return nil, fmt.Errorf("expected a JSON object or array")
	}
	values := make(map[string]string, len(fields))
	for i := 0; decoder.More(); i++ {
		var key string
		if object {
			token, err := decoder.Token()
			if err != nil {
				return nil, err
			}
			key = token.(string)
			if !slices.Contains(fields, key) {
				var ignored json.RawMessage
				if err := decoder.Decode(&ignored); err != nil {
					return nil, err
				}
				continue
			}
		} else {
			if i >= len(fields) {
				return nil, fmt.Errorf("too many positional arguments")
			}
			key = fields[i]
		}
		if _, exists := values[key]; exists {
			return nil, fmt.Errorf("duplicate field %q", key)
		}
		var value *string
		if err := decoder.Decode(&value); err != nil {
			return nil, fmt.Errorf("field %q: %w", key, err)
		}
		if value == nil {
			return nil, fmt.Errorf("field %q must be a string", key)
		}
		values[key] = *value
	}
	if _, err := decoder.Token(); err != nil {
		return nil, err
	}
	if _, err := decoder.Token(); err != io.EOF {
		return nil, fmt.Errorf("unexpected data after tool arguments")
	}
	if _, exists := values[required]; !exists {
		return nil, fmt.Errorf("missing field %q", required)
	}
	return values, nil
}
