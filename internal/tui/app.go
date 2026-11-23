package tui

import (
	"strings"
	"time"
)

const (
	maxRows       = 500
	eventLogLimit = 200
	errorTTL      = 10 * time.Second
)

type connectionState uint8

const (
	connecting connectionState = iota
	connected
	reconnecting
	disconnected
)

func (s connectionState) label() string {
	return [...]string{"接続中...", "接続済み", "再接続中...", "切断"}[s]
}

type systemActivity uint8

const (
	activityOffline systemActivity = iota
	activityConnecting
	activityWaiting
	activityIdle
)

func (a systemActivity) label() string {
	return [...]string{"オフライン", "接続中", "応答待ち", "アイドル"}[a]
}

// The focus also identifies the screen, so a screen switch cannot leave
// keyboard focus on a hidden panel.
type focus uint8

const (
	focusInput focus = iota
	focusChat
	focusLog
	focusModules
	focusCharacters
)

type chatRow struct {
	role   string
	text   string
	silent bool
}

type logRow struct {
	label string
	text  string
}

type characterEntry struct {
	id     string
	name   string
	active bool
}

type timedError struct {
	message   string
	expiresAt time.Time
}

// app owns the terminal-independent state. Its zero value starts on Main
// with input focus while the client attempts its first connection.
type app struct {
	connection connectionState
	version    string
	modules    moduleFlags
	character  *characterRef
	focus      focus
	input      string
	pending    bool
	shouldQuit bool

	chat       []chatRow
	log        []logRow
	eventLog   []string
	chatScroll int // Rows above the latest visible page.
	logScroll  int

	characters      []characterEntry
	characterCursor int
	notice          string
	statusLine      string // Protocol errors persist until replaced.
	operatorError   *timedError
}

func pushCapped[T any](rows []T, row T, limit int) []T {
	if len(rows) == limit {
		copy(rows, rows[1:])
		rows[len(rows)-1] = row
		return rows
	}
	return append(rows, row)
}

func (a *app) applyServerMsg(msg serverMsg) {
	if msg.unknown {
		switch msg.kind {
		case "volume", "speakingLevel", "speaking", "affect", "position":
			// Continuous telemetry would evict useful log entries almost
			// immediately, even with a bounded log.
		default:
			a.addLog("unhandled", msg.kind)
		}
		return
	}

	switch msg.kind {
	case "hello":
		a.version, a.modules, a.character = msg.version, msg.modules, msg.character
	case "status":
		a.modules = msg.modules
	case "chat":
		a.chat = pushCapped(a.chat, chatRow{role: msg.role, text: *msg.text}, maxRows)
		if msg.role == "assistant" {
			a.pending = false
		}
	case "silent":
		a.chat = pushCapped(a.chat, chatRow{text: msg.reason, silent: true}, maxRows)
		a.pending = false
	case "sense", "memory":
		a.addLog(msg.kind+":"+msg.category, *msg.text)
	case "actionLog":
		a.addLog("action", *msg.text)
	case "error":
		a.pending = false
		a.statusLine = "error: " + *msg.message
		a.addLog("error", *msg.message)
	case "inputAccepted":
		a.pending = true
	case "response":
		a.pending = false
		if msg.status == "error" {
			a.statusLine = "error: unknown error"
			if msg.message != nil {
				a.statusLine = "error: " + *msg.message
			}
		} else if msg.text != nil {
			a.addLog("response", *msg.text)
		}
	}
}

func (a *app) addLog(label, message string) {
	a.log = pushCapped(a.log, logRow{label: label, text: message}, maxRows)
}

func (a *app) setConnection(state connectionState) {
	if a.connection != state {
		a.addEventLog("接続状態: " + state.label())
		a.connection = state
	}
}

func (a *app) systemActivity() systemActivity {
	switch a.connection {
	case disconnected:
		return activityOffline
	case connected:
		if a.pending {
			return activityWaiting
		}
		return activityIdle
	default:
		return activityConnecting
	}
}

func (a *app) inConfig() bool {
	return a.focus == focusModules || a.focus == focusCharacters
}

func (a *app) cycleFocus(step int) {
	if a.inConfig() {
		a.focus = focusModules + focus((int(a.focus-focusModules)+step+2)%2)
	} else {
		a.focus = focus((int(a.focus) + step + 3) % 3)
	}
}

func (a *app) toggleView() {
	if a.inConfig() {
		a.returnToMain()
		return
	}
	a.focus = focusModules
	a.setNotice("設定画面を表示しています")
	a.addEventLog("設定画面に切り替え")
}

func (a *app) returnToMain() {
	if a.inConfig() {
		a.focus = focusInput
		a.setNotice("メイン画面に戻りました")
		a.addEventLog("メイン画面に切り替え")
	}
}

func (a *app) addEventLog(message string) {
	if message = strings.TrimSpace(message); message != "" {
		a.eventLog = pushCapped(a.eventLog, message, eventLogLimit)
	}
}

func (a *app) setNotice(message string) { a.notice = message }

func (a *app) clearNotice() { a.notice = "" }

func (a *app) setError(message string, now time.Time) {
	a.operatorError = &timedError{message: message, expiresAt: now.Add(errorTTL)}
}

func (a *app) tick(now time.Time) {
	if a.operatorError != nil && !now.Before(a.operatorError.expiresAt) {
		a.operatorError = nil
	}
}

func (a *app) setCharacters(characters []characterEntry) {
	a.characters = characters
	a.moveCharacterCursor(0)
}

func (a *app) moveCharacterCursor(delta int) {
	a.characterCursor = max(0, min(a.characterCursor+delta, len(a.characters)-1))
}

func (a *app) selectedCharacter() *characterEntry {
	if a.characterCursor < len(a.characters) {
		return &a.characters[a.characterCursor]
	}
	return nil
}

type moduleState struct {
	name string
	on   bool
}

func (a *app) moduleStates() [7]moduleState {
	return [...]moduleState{
		{"talk", a.modules.Talk},
		{"memory", a.modules.Memory},
		{"speech", a.modules.Speech},
		{"vision", a.modules.Vision},
		{"action", a.modules.Action},
		{"scheduler", a.modules.Scheduler},
		{"translation", a.modules.Translation},
	}
}

func (a *app) takeInputForSend() string {
	text := strings.TrimSpace(a.input)
	a.input = ""
	return text
}
