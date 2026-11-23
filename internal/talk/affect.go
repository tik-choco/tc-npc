package talk

import (
	"bytes"
	"encoding/json"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/person"
)

// DriveSnapshot includes the resting level so consumers can measure deviation.
type DriveSnapshot struct {
	Key   string  `json:"key"`
	Level float32 `json:"level"`
	Base  float32 `json:"base"`
}

// AffectSnapshot is the UI and emotion evaluation wire shape. Drives are
// always in DriveKey order. The UI transport supplies its own timestamp.
type AffectSnapshot struct {
	Drives          []DriveSnapshot `json:"drives"`
	Familiarity     float32         `json:"familiarity"`
	Closing         bool            `json:"closing"`
	InviteCaution   bool            `json:"inviteCaution"`
	Partner         *string         `json:"partner"`
	PartnerKnown    bool            `json:"partnerKnown"`
	PartnerSwitched bool            `json:"partnerSwitched"`
	PartnerAway     bool            `json:"partnerAway"`
}

// Use serde's decimal spelling for whole-valued f32 readings so the live
// payload and replay trace have the same representation as the Rust model.
func (s AffectSnapshot) MarshalJSON() ([]byte, error) {
	type snapshot AffectSnapshot
	wire := struct {
		Drives      []DriveSnapshot `json:"drives"`
		Familiarity affectJSONFloat `json:"familiarity"`
		snapshot
	}{s.Drives, affectJSONFloat(s.Familiarity), snapshot(s)}
	var out bytes.Buffer
	encoder := json.NewEncoder(&out)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(wire); err != nil {
		return nil, err
	}
	return bytes.TrimSuffix(out.Bytes(), []byte("\n")), nil
}

func (d DriveSnapshot) MarshalJSON() ([]byte, error) {
	return json.Marshal(struct {
		Key   string          `json:"key"`
		Level affectJSONFloat `json:"level"`
		Base  affectJSONFloat `json:"base"`
	}{d.Key, affectJSONFloat(d.Level), affectJSONFloat(d.Base)})
}

type affectJSONFloat float32

func (f affectJSONFloat) MarshalJSON() ([]byte, error) {
	if f == 0 || f == 1 {
		return strconv.AppendFloat(nil, float64(f), 'f', 1, 32), nil
	}
	return json.Marshal(float32(f))
}

// AffectState tracks the drives and closing state of a single conversation.
// Create it with NewAffectState to start the drives at their resting levels.
type AffectState struct {
	levels             [driveCount]float32
	familiarity        float32
	inviteCautionTurns uint32
	closing            bool
	closingTurns       uint32
}

func NewAffectState() AffectState {
	var state AffectState
	state.Relax()
	return state
}

func (a *AffectState) Level(key DriveKey) float32 { return a.levels[key] }
func (a *AffectState) Familiarity() float32       { return a.familiarity }
func (a *AffectState) IsClosing() bool            { return a.closing }

func (a *AffectState) Snapshot() AffectSnapshot {
	readings := make([]DriveSnapshot, driveCount)
	for key, info := range drives {
		readings[key] = DriveSnapshot{Key: DriveKey(key).String(), Level: a.levels[key], Base: info.base}
	}
	return AffectSnapshot{
		Drives:        readings,
		Familiarity:   a.familiarity,
		Closing:       a.closing,
		InviteCaution: a.inviteCautionTurns > 0,
	}
}

// Relax resets momentary affect when a partner leaves, preserving their familiarity.
func (a *AffectState) Relax() {
	for key, info := range drives {
		a.levels[key] = info.base
	}
	a.inviteCautionTurns = 0
	a.closing = false
	a.closingTurns = 0
}

// Update folds in one utterance. ownTurns is the turn count with this partner.
func (a *AffectState) Update(partnerText string, ownTurns uint32, partnerKnown bool) {
	text := strings.TrimSpace(partnerText)
	question := strings.ContainsAny(text, "?？")
	charCount := utf8.RuneCountInString(text)
	novelty := charCount >= 12
	terse := charCount <= 2

	conflict := includesAny(text, conflictWords[:])
	positive := includesAny(text, positiveWords[:])
	bond := includesAny(text, bondWords[:])
	humor := includesAny(text, humorWords[:])
	tired := includesAny(text, tiredWords[:])
	night := includesAny(text, nightWords[:])
	pain := includesAny(text, painWords[:])
	calm := includesAny(text, calmWords[:])
	urgent := includesAny(text, urgentWords[:])
	learn := includesAny(text, learnWords[:])
	comfort := includesAny(text, comfortWords[:])
	recover := includesAny(text, recoverWords[:])
	lure := includesAny(text, lureWords[:])
	surprise := includesAny(text, surpriseWords[:])

	var delta [driveCount]float32
	if partnerKnown {
		a.familiarity = max(a.familiarity, 0.7)
	}

	// Pain takes precedence over length so a long, sad turn lowers dopamine.
	switch {
	case pain:
		delta[Dopamine] = -0.15
	case question || novelty:
		delta[Dopamine] = 0.18
	case terse:
		delta[Dopamine] = -0.12
	default:
		delta[Dopamine] = -0.03
	}
	delta[Serotonin] += stimulus(positive, 0.06) - stimulus(conflict, 0.15) - stimulus(pain && !conflict, 0.1)
	delta[Oxytocin] += stimulus(bond, 0.15) + float32(0.04*float32(min(ownTurns, 4)))/4.0
	// Recovery adds signal without increasing the weight of ordinary acknowledgments.
	delta[Endorphin] += stimulus(humor, 0.18) + stimulus(positive, 0.05) + stimulus(recover, 0.08)
	delta[Cortisol] += stimulus(conflict || urgent || pain, 0.18) - stimulus(positive || calm, 0.06)
	delta[Noradrenaline] += stimulus(question || urgent, 0.16) - stimulus(terse, 0.06)
	// Declarative conflict raises adrenaline too. Pain alone uses the sad-voting drives.
	delta[Adrenaline] += stimulus(urgent || conflict, 0.22)
	delta[Acetylcholine] += stimulus(learn || charCount >= 16, 0.13)
	// Surprise has its own addend so even a short reaction reaches the face.
	delta[Acetylcholine] += stimulus(surprise, 0.4)
	delta[Glutamate] += stimulus(learn, 0.13)
	delta[Gaba] += stimulus(calm, 0.12) - stimulus(urgent || conflict, 0.1)
	delta[Glycine] += stimulus(calm || night, 0.13)
	delta[Melatonin] += stimulus(night || tired, 0.2) - stimulus(urgent, 0.08)
	delta[Orexin] += stimulus(question || novelty || positive, 0.12) - stimulus(tired || night, 0.16)
	delta[Orexin] += stimulus(surprise, 0.35)
	delta[Histamine] += stimulus(question || urgent, 0.12) - stimulus(tired || night, 0.1)
	delta[Histamine] += stimulus(surprise, 0.35)
	// Values above 0.2 tipped mildly painful but otherwise neutral turns into sad.
	delta[Dynorphin] += stimulus(pain || conflict, 0.2)
	delta[Dhea] += stimulus(recover || positive, 0.12)
	// Comfort, calm and recovery can each release tension.
	delta[Enkephalin] += stimulus(comfort, 0.18) + stimulus(calm, 0.08) + stimulus(recover, 0.06)
	// Calm already feeds relaxed drives. Positive words, humor and recovery reinforce happy.
	delta[Anandamide] += stimulus(positive, 0.075) + stimulus(humor, 0.115) + stimulus(recover, 0.095)
	delta[SubstanceP] += stimulus(pain, 0.2)
	delta[Npy] += stimulus(calm, 0.1) + stimulus(recover, 0.06)
	// Cck votes for angry, so pain alone should only move it slightly.
	delta[Cck] += stimulus(conflict, 0.16) + stimulus(pain && !conflict, 0.03)
	delta[Bdnf] += stimulus(learn, 0.11)

	farewell := isFarewell(text)
	reengaged := !farewell && (question || novelty || includesAny(text, greetingWords[:]))
	if a.closing && reengaged {
		a.closing = false
		a.closingTurns = 0
	}
	if farewell {
		a.closing = true
	}
	if a.closing {
		a.closingTurns++
	}

	wariness := max(0.45-a.familiarity, 0)
	delta[Cortisol] += float32(wariness * 0.5)
	delta[Noradrenaline] += float32(wariness * 0.5)
	delta[Oxytocin] -= float32(wariness * 0.3)

	if lure && a.familiarity < 0.6 {
		a.inviteCautionTurns = 3
	} else if a.inviteCautionTurns > 0 {
		a.inviteCautionTurns--
	}
	if a.inviteCautionTurns > 0 {
		delta[Cortisol] += 0.25
		delta[Cck] += 0.2
		delta[Noradrenaline] += 0.15
		delta[Oxytocin] -= 0.12
	}
	a.familiarity = clamp(a.familiarity + 0.1 + stimulus(bond, 0.1))

	// Decay and stimulus both start at the previous level. Scaling the
	// stimulus by the remaining headroom keeps repeated ordinary turns
	// below the ceiling and lets stronger stimuli still move the drive.
	// Explicit rounding prevents fused operations from changing Rust's f32 results.
	for i, info := range drives {
		pull := float32(0.08)
		if info.fast {
			pull = 0.2
		} else if isPhasic(DriveKey(i)) {
			pull = 0.15
		}
		level := a.levels[i]
		decayed := float32(level + float32((info.base-level)*pull))
		headroom := level
		if delta[i] > 0 {
			headroom = 1 - level
		}
		effectiveDelta := float32(delta[i] * headroom)
		a.levels[i] = clamp(decayed + effectiveDelta)
	}
}

func isPhasic(key DriveKey) bool {
	switch key {
	case Dopamine, Endorphin, Noradrenaline, Histamine, Glutamate, Acetylcholine, Orexin:
		return true
	default:
		return false
	}
}

// ToPrompt describes closing, caution, familiarity, and the three most
// deviant drives. Ties retain drive order.
func (a *AffectState) ToPrompt() string {
	type reading struct {
		key       DriveKey
		deviation float32
	}
	var salient []reading
	for i, info := range drives {
		deviation := a.levels[i] - info.base
		if abs(deviation) >= 0.2 {
			salient = append(salient, reading{DriveKey(i), deviation})
		}
	}
	sort.SliceStable(salient, func(i, j int) bool {
		return abs(salient[i].deviation) > abs(salient[j].deviation)
	})
	lines := []string{"今のあなたの内的状態（強く動いている感情だけ。セリフに直接書かず、態度に滲ませる）:"}
	if a.closing {
		if a.closingTurns <= 1 {
			lines = append(lines, "・相手が別れの挨拶をした。ここで会話は終わり。短い別れの言葉を一度だけ返し、新しい話題・お願い・気遣い・詩的な余韻を付け足さない。")
		} else {
			lines = append(lines, "・会話はもう終わっている。これ以上言葉を重ねない。「……」か「はい」「ええ」程度の最小限だけにする。")
		}
	}
	if a.inviteCautionTurns > 0 {
		lines = append(lines, "・まだよく知らない相手に、どこかへ行こう／一緒に来てと誘われている。軽々しく応じず、理由や相手の素性をそっと確かめる。")
	}
	if a.familiarity < 0.4 {
		lines = append(lines, "・相手がまだ誰か分からない。いきなり話しかけられて少し身構えている。すぐ打ち解けず様子をうかがい、必要なら「どなたですか」とそっと尋ねる。")
	}
	if len(salient) == 0 {
		lines = append(lines, "・内的状態は落ち着いて安定している。")
	}
	for _, reading := range salient[:min(len(salient), 3)] {
		info := drives[reading.key]
		wording := info.low
		if reading.deviation > 0 {
			wording = info.high
		}
		lines = append(lines, "・"+wording)
	}
	lines = append(lines,
		"温かさは感嘆詞や過剰な称賛では出さない。相手の言葉から具体を拾い、覚えていることに触れて示す。",
		"関心があっても質問攻めにしない。問いは連続させず、まず相手の発話を受け止める。",
	)
	return strings.Join(lines, "\n")
}

// ForcedClosureReply returns a fixed closing reply when the turn can skip the LLM.
func (a *AffectState) ForcedClosureReply(partnerText string) (string, bool) {
	if !a.closing {
		return "", false
	}
	text := strings.TrimSpace(partnerText)
	if isFarewell(text) {
		switch {
		case strings.Contains(text, "おやすみ"):
			return "おやすみなさい。", true
		case strings.Contains(text, "いってきます") || strings.Contains(text, "行ってきます"):
			return "いってらっしゃい。", true
		case strings.Contains(text, "また") || strings.Contains(text, "じゃあ"):
			return "またね。", true
		default:
			return "ばいばい。", true
		}
	}
	if a.closingTurns > 1 && isMinimalAck(text) {
		return "……", true
	}
	return "", false
}

// Bound the number of retained identities in a long-running session.
const maxTrackedPartners = 16

type PartnerAbsence struct {
	Who  *string
	Idle time.Duration
}

type PartnerSwitch struct {
	From      *string
	To        string
	Returning bool
}

// PartnerAffect follows the current speaker while retaining recent speakers'
// individual drives. A speakerless utterance keeps the current partner.
type PartnerAffect struct {
	currentKey   string
	currentName  *string
	currentKnown bool
	switched     bool
	away         bool
	lastActivity *time.Time
	states       map[string]*partnerEntry
	recency      []string // least recently current first
}

type partnerEntry struct {
	state    AffectState
	ownTurns uint32
}

func NewPartnerAffect() PartnerAffect {
	return PartnerAffect{
		states:  map[string]*partnerEntry{"": {state: NewAffectState()}},
		recency: []string{""},
	}
}

func (a *PartnerAffect) Observe(speaker *string) *PartnerSwitch {
	a.switched = false
	a.away = false
	if speaker == nil {
		return nil
	}
	name := strings.TrimSpace(*speaker)
	key := person.NormalizePersonName(name)
	if key == "" || key == a.currentKey {
		return nil
	}
	_, returning := a.states[key]
	if !returning {
		a.states[key] = &partnerEntry{state: NewAffectState()}
	}
	from := a.currentName
	a.currentKey = key
	a.currentName = &name
	a.currentKnown = returning
	a.switched = true
	a.touch(key)
	return &PartnerSwitch{From: copyString(from), To: name, Returning: returning}
}

func (a *PartnerAffect) Update(partnerText string, now time.Time) {
	a.lastActivity = &now
	a.away = false
	entry := a.states[a.currentKey]
	entry.state.Update(partnerText, entry.ownTurns, a.currentKnown)
	if entry.ownTurns < ^uint32(0) {
		entry.ownTurns++
	}
}

// CheckAbsence reports at most once per silence and resets the drives while
// retaining familiarity. A zero timeout disables absence checks.
func (a *PartnerAffect) CheckAbsence(now time.Time, timeout time.Duration) *PartnerAbsence {
	if timeout == 0 || a.lastActivity == nil {
		return nil
	}
	idle := max(now.Sub(*a.lastActivity), 0)
	if idle < timeout {
		return nil
	}
	a.lastActivity = nil
	a.away = true
	if entry := a.states[a.currentKey]; entry != nil {
		entry.state.Relax()
	}
	return &PartnerAbsence{Who: copyString(a.currentName), Idle: idle}
}

func (a *PartnerAffect) state() *AffectState {
	if entry := a.states[a.currentKey]; entry != nil {
		return &entry.state
	}
	fallback := NewAffectState()
	return &fallback
}

func (a *PartnerAffect) Snapshot() AffectSnapshot {
	snapshot := a.state().Snapshot()
	snapshot.Partner = copyString(a.currentName)
	snapshot.PartnerKnown = a.currentKnown
	snapshot.PartnerSwitched = a.switched
	snapshot.PartnerAway = a.away
	return snapshot
}

func (a *PartnerAffect) touch(key string) {
	a.recency = slices.DeleteFunc(a.recency, func(other string) bool { return key == other })
	a.recency = append(a.recency, key)
	if len(a.recency) > maxTrackedPartners {
		delete(a.states, a.recency[0])
		a.recency = slices.Delete(a.recency, 0, 1)
	}
}

func copyString(value *string) *string {
	if value == nil {
		return nil
	}
	copy := *value
	return &copy
}

func stimulus(condition bool, value float32) float32 {
	if condition {
		return value
	}
	return 0
}

func includesAny(text string, markers []string) bool {
	for _, marker := range markers {
		if strings.Contains(text, marker) {
			return true
		}
	}
	return false
}

func abs(value float32) float32 {
	if value < 0 {
		return -value
	}
	return value
}

func clamp(value float32) float32 { return min(max(value, 0), 1) }

const trailingPunctuation = "。.!！？?、"

func isFarewell(text string) bool {
	text = strings.TrimSpace(text)
	stripped := strings.TrimRight(text, trailingPunctuation)
	// A mid-sentence じゃあ often connects to the next topic.
	if strings.Contains(text, "じゃあ") && !strings.Contains(text, "また") &&
		!strings.HasSuffix(stripped, "じゃあ") && !strings.HasSuffix(stripped, "じゃあね") {
		return false
	}
	return includesAny(text, farewellWords[:])
}

func isMinimalAck(text string) bool {
	stripped := strings.Trim(strings.TrimSpace(text), " 　"+trailingPunctuation)
	return slices.Contains(minimalAckWords[:], stripped) || utf8.RuneCountInString(stripped) <= 2
}
