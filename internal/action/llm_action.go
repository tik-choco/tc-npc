package action

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
)

var actionDescriptions = [...]string{
	`"w" args: [] - 前進を開始する(停止するまで継続)`,
	`"s" args: [] - 後退を開始する(停止するまで継続)`,
	`"a" args: [] - 左移動を開始する(停止するまで継続)`,
	`"d" args: [] - 右移動を開始する(停止するまで継続)`,
	`"fw" args: ["seconds"] - 指定秒数(既定2秒)前進する`,
	`"tr" args: ["seconds"] - 指定秒数(既定1秒)右に旋回する`,
	`"tl" args: ["seconds"] - 指定秒数(既定1秒)左に旋回する`,
	`"jump" args: [] - ジャンプする`,
	`"mv" args: ["meters"] - 前方に指定メートル(既定1m)移動する`,
	`"mb" args: ["meters"] - 後方に指定メートル(既定1m)移動する`,
	`"rt" args: ["degrees"] - 右に指定角度(既定90°)回転する`,
	`"lt" args: ["degrees"] - 左に指定角度(既定90°)回転する`,
	`"mvt" args: ["meters", "seconds"] - 指定秒数(既定4秒)で指定メートル(既定2m)移動する`,
	`"rtt" args: ["degrees", "seconds"] - 指定秒数(既定3秒)で右に指定角度(既定180°)回転する`,
	`"ltt" args: ["degrees", "seconds"] - 指定秒数(既定3秒)で左に指定角度(既定180°)回転する`,
	`"stop" args: [] - すべての移動を停止する`,
	`"pos" args: [] - 現在位置を表示する`,
	`"reset" args: [] - 現在位置を原点にリセットする`,
	`"goto" args: ["x", "y", "heading", "seconds"] - 指定座標へ移動する(headingとsecondsは省略可、既定10秒)`,
	`"home" args: ["seconds"] - 原点に帰還する(既定10秒)`,
	`"locations" args: [] - 登録済みロケーション一覧を表示する`,
	`"go" args: ["location_name", "seconds"] - 登録済みロケーションへ移動する(既定10秒)`,
	`"routes" args: [] - 登録済みルート一覧を表示する`,
	`"route" args: ["route_name"] - 登録済みルートを実行する`,
}

const promptIntro = `あなたはVRChat内のアバターを制御するアシスタントです。
ユーザーの指示に従って移動や回転を行います。
必ずJSON形式で応答してください。

応答フォーマット:
{
  "message": "ユーザーへの返答メッセージ",
  "actions": [
    {"name": "アクション名1", "args": ["引数1", ...]},
    {"name": "アクション名2", "args": ["引数1", ...]}
  ]
}

利用可能なアクション:
`

func buildStaticSystemPrompt(language string) string {
	var prompt strings.Builder
	prompt.WriteString(promptIntro)
	for _, line := range actionDescriptions {
		prompt.WriteString("- ")
		prompt.WriteString(line)
		prompt.WriteByte('\n')
	}
	if instruction, ok := config.LanguageInstruction(language); ok {
		fmt.Fprintf(&prompt, "\nThe \"message\" field must follow this rule (action names and JSON keys stay as written above): %s\n", instruction)
	}
	return prompt.String()
}

// Rebuild the map and pose portion on every turn so recent edits and movement
// are reflected in the plan.
func (s *actionState) dynamicSystemPrompt() string {
	data := s.mapSnapshot()
	var prompt strings.Builder
	prompt.WriteString(s.staticPrompt)
	if len(data.locations) > 0 {
		prompt.WriteString("\n登録済みロケーション:\n")
		for _, loc := range data.locations {
			fmt.Fprintf(&prompt, "- %s (x=%.1f, y=%.1f, heading=%.1f°)\n", loc.Name, loc.X, loc.Y, loc.Heading)
		}
	}
	if len(data.routes) > 0 {
		prompt.WriteString("\n登録済みルート:\n")
		for _, route := range data.routes {
			fmt.Fprintf(&prompt, "- %s: %s\n", route.Name, routePath(route))
		}
	}
	fmt.Fprintf(&prompt, "\n\n現在のステータス:\n- %s", s.navigator.pos.get())
	return prompt.String()
}

func trimHistory(history []llm.ChatMessage, historySize uint32) []llm.ChatMessage {
	if uint64(len(history)) <= uint64(historySize) {
		return history
	}
	start := max(1, len(history)-int(historySize))
	return append([]llm.ChatMessage{history[0]}, history[start:]...)
}

func (s *actionState) chatWithAction(query string) {
	if !s.chatBusy.CompareAndSwap(false, true) {
		s.log(fmt.Sprintf("[Chat] 別の指示を処理中のため、このメッセージは破棄されました: %s", query))
		return
	}

	response, err := s.planActions(query)
	s.chatBusy.Store(false)
	if s.shutdown.Err() != nil {
		return
	}
	if err != nil {
		s.log(fmt.Sprintf("Error: %v", err))
		return
	}
	s.log(response.Message)
	if len(response.Actions) != 0 {
		s.executeActions(response.Actions)
	}
}

func (s *actionState) planActions(query string) (actionResponse, error) {
	s.history[0] = llm.SystemMessage(s.dynamicSystemPrompt())
	s.history = trimHistory(append(s.history, llm.UserMessage(query)), max(s.historySize, 1))

	request := llm.NewChatRequest(s.model, s.history)
	format := llm.JSONObjectFormat()
	request.ResponseFormat = &format
	response, err := s.llm.Chat(s.shutdown, request)
	if err != nil {
		return actionResponse{}, fmt.Errorf("chat error: %w", err)
	}

	var content string
	if text := response.Content(); text != nil {
		content = *text
	}
	s.history = append(s.history, llm.AssistantMessage(content))

	// A malformed reply still gets shown to the operator as plain text.
	var plan actionResponse
	if err := json.Unmarshal([]byte(content), &plan); err != nil {
		return actionResponse{Message: content}, nil
	}
	return plan, nil
}
