package llm

import (
	"bytes"
	"encoding/json"
	"fmt"
	"strings"
)

// ChatMessage is a text, multimodal, or tool-result message.
type ChatMessage struct {
	Role       string      `json:"role"`
	Content    ChatContent `json:"content"`
	ToolCalls  []ToolCall  `json:"tool_calls,omitzero"`
	ToolCallID *string     `json:"tool_call_id,omitempty"`
}

func SystemMessage(text string) ChatMessage    { return TextMessage("system", text) }
func UserMessage(text string) ChatMessage      { return TextMessage("user", text) }
func AssistantMessage(text string) ChatMessage { return TextMessage("assistant", text) }

func TextMessage(role, text string) ChatMessage {
	return ChatMessage{Role: role, Content: ChatContent{Text: text}}
}

func UserPartsMessage(parts []ContentPart) ChatMessage {
	if parts == nil {
		parts = []ContentPart{}
	}
	return ChatMessage{Role: "user", Content: ChatContent{Parts: parts}}
}

func ToolResultMessage(toolCallID, text string) ChatMessage {
	message := TextMessage("tool", text)
	message.ToolCallID = &toolCallID
	return message
}

func (m *ChatMessage) UnmarshalJSON(data []byte) error {
	var decoded ChatMessage
	if err := decodeObject(data,
		jsonField{"role", &decoded.Role, required},
		jsonField{"content", &decoded.Content, required},
		jsonField{"tool_calls", &decoded.ToolCalls, optional},
		jsonField{"tool_call_id", &decoded.ToolCallID, optional},
	); err != nil {
		return err
	}
	*m = decoded
	return nil
}

// ChatContent encodes as Text when Parts is nil, or as a JSON array otherwise.
type ChatContent struct {
	Text  string
	Parts []ContentPart
}

// AsText returns the text or joins the text parts with newlines.
func (c ChatContent) AsText() string {
	if c.Parts == nil {
		return c.Text
	}
	var text []string
	for _, part := range c.Parts {
		if part.Type == "text" {
			text = append(text, part.Text)
		}
	}
	return strings.Join(text, "\n")
}

func (c ChatContent) MarshalJSON() ([]byte, error) {
	if c.Parts != nil {
		return json.Marshal(c.Parts)
	}
	return json.Marshal(c.Text)
}

func (c *ChatContent) UnmarshalJSON(data []byte) error {
	data = bytes.TrimSpace(data)
	var decoded ChatContent
	var err error
	switch {
	case len(data) > 0 && data[0] == '"':
		err = json.Unmarshal(data, &decoded.Text)
	case len(data) > 0 && data[0] == '[':
		err = json.Unmarshal(data, &decoded.Parts)
	default:
		return fmt.Errorf("chat content must be a string or an array of parts")
	}
	if err == nil {
		*c = decoded
	}
	return err
}

// ContentPart encodes the field selected by Type, either "text" or "image_url".
type ContentPart struct {
	Type     string
	Text     string
	ImageURL ImageURL
}

func TextPart(text string) ContentPart {
	return ContentPart{Type: "text", Text: text}
}

// ImageDataURLPart constructs an image part from a data URL.
func ImageDataURLPart(dataURL string) ContentPart {
	return ContentPart{Type: "image_url", ImageURL: ImageURL{URL: dataURL}}
}

func (p ContentPart) MarshalJSON() ([]byte, error) {
	switch p.Type {
	case "text":
		return json.Marshal(struct {
			Type string `json:"type"`
			Text string `json:"text"`
		}{p.Type, p.Text})
	case "image_url":
		return json.Marshal(struct {
			Type     string   `json:"type"`
			ImageURL ImageURL `json:"image_url"`
		}{p.Type, p.ImageURL})
	default:
		return nil, fmt.Errorf("unknown content part type %q", p.Type)
	}
}

func (p *ContentPart) UnmarshalJSON(data []byte) error {
	var kind string
	if err := decodeObject(data, jsonField{"type", &kind, required}); err != nil {
		return err
	}
	decoded := ContentPart{Type: kind}
	var field jsonField
	switch kind {
	case "text":
		field = jsonField{"text", &decoded.Text, required}
	case "image_url":
		field = jsonField{"image_url", &decoded.ImageURL, required}
	default:
		return fmt.Errorf("unknown content part type %q", kind)
	}
	if err := decodeObject(data, field); err != nil {
		return err
	}
	*p = decoded
	return nil
}

type ImageURL struct {
	URL string `json:"url"`
}

func (u *ImageURL) UnmarshalJSON(data []byte) error {
	var decoded ImageURL
	if err := decodeObject(data, jsonField{"url", &decoded.URL, required}); err != nil {
		return err
	}
	*u = decoded
	return nil
}

const functionType = "function"

// Tool is an OpenAI function-calling tool definition.
type Tool struct {
	Type     string      `json:"type"`
	Function FunctionDef `json:"function"`
}

func FunctionTool(name, description string, parameters json.RawMessage) Tool {
	return Tool{Type: functionType, Function: FunctionDef{
		Name: name, Description: &description, Parameters: parameters,
	}}
}

func (t *Tool) UnmarshalJSON(data []byte) error {
	decoded := Tool{Type: functionType}
	if err := decodeObject(data,
		jsonField{"type", &decoded.Type, defaulted},
		jsonField{"function", &decoded.Function, required},
	); err != nil {
		return err
	}
	*t = decoded
	return nil
}

type FunctionDef struct {
	Name        string          `json:"name"`
	Description *string         `json:"description,omitempty"`
	Parameters  json.RawMessage `json:"parameters"`
}

func (f *FunctionDef) UnmarshalJSON(data []byte) error {
	var decoded FunctionDef
	if err := decodeObject(data,
		jsonField{"name", &decoded.Name, required},
		jsonField{"description", &decoded.Description, optional},
		jsonField{"parameters", &decoded.Parameters, requiredNullable},
	); err != nil {
		return err
	}
	*f = decoded
	return nil
}

// ToolCall is a complete call requested by the assistant.
type ToolCall struct {
	ID       string       `json:"id"`
	Type     string       `json:"type"`
	Function FunctionCall `json:"function"`
}

func (t *ToolCall) UnmarshalJSON(data []byte) error {
	decoded := ToolCall{Type: functionType}
	if err := decodeObject(data,
		jsonField{"id", &decoded.ID, defaulted},
		jsonField{"type", &decoded.Type, defaulted},
		jsonField{"function", &decoded.Function, required},
	); err != nil {
		return err
	}
	*t = decoded
	return nil
}

type FunctionCall struct {
	Name      string `json:"name"`
	Arguments string `json:"arguments"` // JSON-encoded arguments returned by the API.
}

func (f *FunctionCall) UnmarshalJSON(data []byte) error {
	var decoded FunctionCall
	if err := decodeObject(data,
		jsonField{"name", &decoded.Name, required},
		jsonField{"arguments", &decoded.Arguments, required},
	); err != nil {
		return err
	}
	*f = decoded
	return nil
}

// ResponseFormat specifies the response format for JSON-mode requests.
type ResponseFormat struct {
	Type string `json:"type"`
}

func JSONObjectFormat() ResponseFormat { return ResponseFormat{Type: "json_object"} }

func (f *ResponseFormat) UnmarshalJSON(data []byte) error {
	var decoded ResponseFormat
	if err := decodeObject(data, jsonField{"type", &decoded.Type, required}); err != nil {
		return err
	}
	*f = decoded
	return nil
}

type ChatRequest struct {
	Model           string          `json:"model"`
	Messages        []ChatMessage   `json:"messages"`
	Tools           []Tool          `json:"tools,omitzero"`
	ResponseFormat  *ResponseFormat `json:"response_format,omitempty"`
	ReasoningEffort *string         `json:"reasoning_effort,omitempty"`
	MaxTokens       *uint32         `json:"max_tokens,omitempty"`
	Stream          *bool           `json:"stream,omitempty"`
}

func NewChatRequest(model string, messages []ChatMessage) ChatRequest {
	return ChatRequest{Model: model, Messages: messages}
}

func (r ChatRequest) MarshalJSON() ([]byte, error) {
	if r.Messages == nil {
		r.Messages = []ChatMessage{}
	}
	type wire ChatRequest
	return json.Marshal(wire(r))
}

type ChatResponse struct {
	ID      *string      `json:"id"`
	Choices []ChatChoice `json:"choices"`
}

func (r *ChatResponse) UnmarshalJSON(data []byte) error {
	decoded := ChatResponse{Choices: []ChatChoice{}}
	if err := decodeObject(data,
		jsonField{"id", &decoded.ID, optional},
		jsonField{"choices", &decoded.Choices, defaulted},
	); err != nil {
		return err
	}
	*r = decoded
	return nil
}

// Content returns the first choice's message content, if present.
func (r ChatResponse) Content() *string {
	if len(r.Choices) == 0 {
		return nil
	}
	return r.Choices[0].Message.Content
}

// ToolCalls returns the first choice's requested tool calls.
func (r ChatResponse) ToolCalls() []ToolCall {
	if len(r.Choices) == 0 {
		return nil
	}
	return r.Choices[0].Message.ToolCalls
}

type ChatChoice struct {
	Index        uint32          `json:"index"`
	Message      ResponseMessage `json:"message"`
	FinishReason *string         `json:"finish_reason"`
}

func (c *ChatChoice) UnmarshalJSON(data []byte) error {
	var decoded ChatChoice
	if err := decodeObject(data,
		jsonField{"index", &decoded.Index, defaulted},
		jsonField{"message", &decoded.Message, required},
		jsonField{"finish_reason", &decoded.FinishReason, optional},
	); err != nil {
		return err
	}
	*c = decoded
	return nil
}

type ResponseMessage struct {
	Role      string     `json:"role"`
	Content   *string    `json:"content"`
	ToolCalls []ToolCall `json:"tool_calls"`
}

func (m *ResponseMessage) UnmarshalJSON(data []byte) error {
	var decoded ResponseMessage
	if err := decodeObject(data,
		jsonField{"role", &decoded.Role, defaulted},
		jsonField{"content", &decoded.Content, optional},
		jsonField{"tool_calls", &decoded.ToolCalls, optional},
	); err != nil {
		return err
	}
	*m = decoded
	return nil
}
