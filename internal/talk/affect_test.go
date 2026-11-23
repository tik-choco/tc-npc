package talk

import (
	"fmt"
	"strings"
	"testing"
	"time"
)

func TestInitialPromptMarksPartnerUnknown(t *testing.T) {
	state := NewAffectState()
	if !strings.Contains(state.ToPrompt(), "相手がまだ誰か分からない") {
		t.Fatal(state.ToPrompt())
	}
}

func TestDriveLevelsStayInRange(t *testing.T) {
	state := NewAffectState()
	for turn, input := range []string{"おやすみ", "うるさい！", "一緒に行こう", "なぜそう思うの?", "疲れた、眠い", "ありがとう、嬉しい", "また明日"} {
		state.Update(input, uint32(turn), false)
		for key := DriveKey(0); key < driveCount; key++ {
			if level := state.Level(key); !(level >= 0 && level <= 1) {
				t.Errorf("turn %d: %s out of range: %g", turn, key, level)
			}
		}
	}
}

func TestGoodnightClosesAndQuestionReengages(t *testing.T) {
	state := NewAffectState()
	state.Update("おやすみ", 3, false)
	if reply, ok := state.ForcedClosureReply("おやすみ"); !state.IsClosing() || !ok || reply != "おやすみなさい。" {
		t.Fatalf("closing = %v, forced reply = %q, %v", state.IsClosing(), reply, ok)
	}
	state.Update("明日どこか一緒に行く?", 4, false)
	if reply, ok := state.ForcedClosureReply("明日どこか一緒に行く?"); state.IsClosing() || ok {
		t.Fatalf("closing = %v, forced reply = %q, %v", state.IsClosing(), reply, ok)
	}
}

func TestStrangerInvitationRaisesInviteCaution(t *testing.T) {
	state := NewAffectState()
	if state.Familiarity() != 0 {
		t.Fatalf("initial familiarity = %g", state.Familiarity())
	}
	state.Update("一緒に行こう", 0, false)
	if !strings.Contains(state.ToPrompt(), "どこかへ行こう") {
		t.Fatal(state.ToPrompt())
	}
}

func TestUtterancesMoveDrives(t *testing.T) {
	for _, tc := range []struct {
		name  string
		input string
		above []DriveKey
		below []DriveKey
		level []DriveKey
	}{
		{name: "conflict raises cortisol", input: "うるさい", above: []DriveKey{Cortisol}},
		{name: "inflected praise raises endorphin", input: "褒められて嬉しかった", above: []DriveKey{Endorphin}},
		{name: "declarative conflict raises adrenaline", input: "最悪だよ", above: []DriveKey{Adrenaline}},
		{
			name: "pain moves sad drives and leaves adrenaline alone", input: "寂しくて悲しい",
			above: []DriveKey{Cortisol, Dynorphin, SubstanceP}, below: []DriveKey{Serotonin}, level: []DriveKey{Adrenaline},
		},
		{name: "brief surprise raises acetylcholine", input: "えっ、うそでしょ", above: []DriveKey{Acetylcholine}},
		{name: "exclaimed praise leaves adrenaline alone", input: "今日の発表、すごく褒められたんだ!", level: []DriveKey{Adrenaline}},
		{name: "plain praise leaves adrenaline alone", input: "今日の発表、すごく褒められたんだ", level: []DriveKey{Adrenaline}},
		{name: "comfort raises enkephalin", input: "大丈夫だよ、ゆっくりでいいから", above: []DriveKey{Enkephalin}},
		{name: "positivity alone raises anandamide", input: "助かった、本当にありがとう", above: []DriveKey{Anandamide}},
		{name: "calm alone leaves anandamide alone", input: "静かでのんびりした時間だった", level: []DriveKey{Anandamide}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			state := NewAffectState()
			state.Update(tc.input, 0, false)
			for _, key := range tc.above {
				if got, base := state.Level(key), drives[key].base; got <= base {
					t.Errorf("%s = %g, want above %g", key, got, base)
				}
			}
			for _, key := range tc.below {
				if got, base := state.Level(key), drives[key].base; got >= base {
					t.Errorf("%s = %g, want below %g", key, got, base)
				}
			}
			for _, key := range tc.level {
				if got, base := state.Level(key), drives[key].base; got != base {
					t.Errorf("%s = %g, want %g", key, got, base)
				}
			}
		})
	}
}

func TestPainBarelyMovesCckComparedToConflict(t *testing.T) {
	pain, conflict := NewAffectState(), NewAffectState()
	pain.Update("寂しくて悲しい", 0, false)
	conflict.Update("うるさい、最悪だよ", 0, false)
	base := drives[Cck].base
	painDelta, conflictDelta := pain.Level(Cck)-base, conflict.Level(Cck)-base
	if painDelta <= 0 || conflictDelta <= painDelta*2 {
		t.Fatalf("cck deviations: pain %g, conflict %g", painDelta, conflictDelta)
	}
}

func TestSnapshotDriveContract(t *testing.T) {
	keys := [...]string{
		"dopamine", "serotonin", "oxytocin", "endorphin", "cortisol", "noradrenaline",
		"adrenaline", "acetylcholine", "glutamate", "gaba", "glycine", "melatonin",
		"orexin", "histamine", "dynorphin", "dhea", "enkephalin", "anandamide",
		"substance_p", "npy", "cck", "bdnf",
	}
	state := NewAffectState()
	snapshot := state.Snapshot()
	if len(snapshot.Drives) != len(keys) {
		t.Fatalf("drive count = %d, want %d", len(snapshot.Drives), len(keys))
	}
	for i, key := range keys {
		want := DriveSnapshot{key, state.Level(DriveKey(i)), drives[i].base}
		if got := snapshot.Drives[i]; got != want || got.Key != DriveKey(i).String() {
			t.Errorf("drive %d = %+v, want %+v", i, got, want)
		}
	}
}

func TestSnapshotReflectsFamiliarityClosingAndCaution(t *testing.T) {
	state := NewAffectState()
	if s := state.Snapshot(); s.Familiarity != 0 || s.Closing || s.InviteCaution {
		t.Fatalf("initial snapshot = %+v", s)
	}
	state.Update("一緒に行こう", 0, false)
	if s := state.Snapshot(); !s.InviteCaution || s.Closing || s.Familiarity != state.Familiarity() {
		t.Fatalf("invitation snapshot = %+v", s)
	}
	state.Update("じゃあね", 1, false)
	if !state.Snapshot().Closing {
		t.Fatal("farewell did not set closing")
	}
}

func warmUp(partners *PartnerAffect, now time.Time) {
	for range 3 {
		partners.Update("ありがとう、嬉しいです", now)
	}
}

func TestUnnamedSpeakerKeepsCurrentPartner(t *testing.T) {
	partners := NewPartnerAffect()
	if partners.Observe(new("太郎")) == nil {
		t.Fatal("expected first switch")
	}
	warmUp(&partners, time.Now())
	familiarity := partners.state().Familiarity()
	if partners.Observe(nil) != nil || partners.Observe(new("   ")) != nil {
		t.Fatal("unnamed utterance changed the partner")
	}
	if s := partners.Snapshot(); stringOrEmpty(s.Partner) != "太郎" || s.Familiarity != familiarity {
		t.Fatalf("unnamed utterance changed the current state: %+v", s)
	}
}

func TestPartnerSwitchKeepsSeparateAffect(t *testing.T) {
	partners := NewPartnerAffect()
	now := time.Now()
	partners.Observe(new("太郎"))
	warmUp(&partners, now)
	familiarity := partners.state().Familiarity()
	if familiarity <= 0 {
		t.Fatal("partner did not warm up")
	}
	change := partners.Observe(new("花子"))
	if change == nil || stringOrEmpty(change.From) != "太郎" || change.To != "花子" || change.Returning {
		t.Fatalf("switch to a new partner = %+v", change)
	}
	if s := partners.Snapshot(); s.Familiarity != 0 || s.PartnerKnown {
		t.Fatalf("new partner inherited affect: %+v", s)
	}
	partners.Update("こんにちは", now)
	change = partners.Observe(new("太郎"))
	if s := partners.Snapshot(); change == nil || !change.Returning || !s.PartnerKnown || s.Familiarity != familiarity {
		t.Fatalf("returning switch = %+v, snapshot = %+v", change, s)
	}
}

func TestPartnerSwitchNormalizesHonorificsAndSpacing(t *testing.T) {
	partners := NewPartnerAffect()
	partners.Observe(new("太郎"))
	for _, name := range []string{"太郎さん", "　太郎　"} {
		if change := partners.Observe(&name); change != nil {
			t.Errorf("%q caused switch %+v", name, change)
		}
	}
}

func TestPartnerSwitchedFlagOnlyMarksFirstTurn(t *testing.T) {
	partners := NewPartnerAffect()
	if partners.Snapshot().PartnerSwitched {
		t.Fatal("initial snapshot marked switched")
	}
	partners.Observe(new("太郎"))
	if !partners.Snapshot().PartnerSwitched {
		t.Fatal("new partner did not set switched")
	}
	partners.Observe(new("太郎"))
	if partners.Snapshot().PartnerSwitched {
		t.Fatal("same partner marked switched")
	}
}

func TestPartnerTrackingEvictsLeastRecent(t *testing.T) {
	partners := NewPartnerAffect()
	now := time.Now()
	partners.Observe(new("最初"))
	warmUp(&partners, now)
	for i := range maxTrackedPartners {
		name := fmt.Sprintf("人%d", i)
		partners.Observe(&name)
		partners.Update("こんにちは", now)
	}
	if change := partners.Observe(new("最初")); change == nil || change.Returning || partners.state().Familiarity() != 0 {
		t.Fatalf("evicted partner still known: %+v", change)
	}
}

const testAbsenceTimeout = 300 * time.Second

func TestPartnerAbsenceSettlesAffect(t *testing.T) {
	partners := NewPartnerAffect()
	start := time.Now()
	partners.Observe(new("太郎"))
	partners.Update("一緒に行こう", start)
	if s := partners.Snapshot(); !s.InviteCaution || s.PartnerAway {
		t.Fatalf("before absence: %+v", s)
	}
	if absence := partners.CheckAbsence(start.Add(299*time.Second), testAbsenceTimeout); absence != nil || partners.Snapshot().PartnerAway {
		t.Fatalf("reported absence too early: %+v", absence)
	}
	absence := partners.CheckAbsence(start.Add(301*time.Second), testAbsenceTimeout)
	if absence == nil || stringOrEmpty(absence.Who) != "太郎" || absence.Idle != 301*time.Second {
		t.Fatalf("absence = %+v", absence)
	}
	s := partners.Snapshot()
	if !s.PartnerAway || s.InviteCaution || s.Closing || stringOrEmpty(s.Partner) != "太郎" ||
		partners.state().Level(Cortisol) != drives[Cortisol].base {
		t.Fatalf("after absence: %+v", s)
	}
}

func TestFamiliaritySurvivesAbsence(t *testing.T) {
	partners := NewPartnerAffect()
	start := time.Now()
	partners.Observe(new("太郎"))
	warmUp(&partners, start)
	familiarity := partners.state().Familiarity()
	if familiarity <= 0 {
		t.Fatal("partner did not warm up")
	}
	partners.CheckAbsence(start.Add(600*time.Second), testAbsenceTimeout)
	if got := partners.state().Familiarity(); got != familiarity {
		t.Fatalf("familiarity after absence = %g, want %g", got, familiarity)
	}
}

func TestAbsenceReportedOnceAndSpeakingAgainClearsAway(t *testing.T) {
	partners := NewPartnerAffect()
	start := time.Now()
	partners.Observe(new("太郎"))
	partners.Update("こんにちは", start)
	if partners.CheckAbsence(start.Add(400*time.Second), testAbsenceTimeout) == nil || !partners.Snapshot().PartnerAway {
		t.Fatal("expected first absence")
	}
	for _, seconds := range []time.Duration{500, 9000} {
		if absence := partners.CheckAbsence(start.Add(seconds*time.Second), testAbsenceTimeout); absence != nil {
			t.Fatalf("reported absence twice: %+v", absence)
		}
	}
	resumed := start.Add(500 * time.Second)
	partners.Observe(new("太郎"))
	partners.Update("ただいま", resumed)
	if partners.Snapshot().PartnerAway || partners.CheckAbsence(resumed.Add(100*time.Second), testAbsenceTimeout) != nil {
		t.Fatal("speaking did not reset absence")
	}
}

func TestAbsenceDisabledUntilSpeechOrWithZeroTimeout(t *testing.T) {
	partners := NewPartnerAffect()
	start := time.Now()
	if absence := partners.CheckAbsence(start.Add(9000*time.Second), testAbsenceTimeout); absence != nil {
		t.Fatalf("absence reported before first speech: %+v", absence)
	}
	partners.Observe(new("太郎"))
	partners.Update("こんにちは", start)
	if absence := partners.CheckAbsence(start.Add(24*time.Hour), 0); absence != nil || partners.Snapshot().PartnerAway {
		t.Fatalf("zero timeout triggered absence: %+v", absence)
	}
}

func TestBondRampCountsTurnsPerPartner(t *testing.T) {
	partners := NewPartnerAffect()
	now := time.Now()
	partners.Observe(new("太郎"))
	for range 10 {
		partners.Update("ありがとう、嬉しいです", now)
	}
	taro := partners.state().Level(Oxytocin)
	partners.Observe(new("花子"))
	partners.Update("ありがとう、嬉しいです", now)
	if hanako := partners.state().Level(Oxytocin); hanako >= taro {
		t.Fatalf("fresh partner's oxytocin %g >= familiar partner's %g", hanako, taro)
	}
}

func TestBoundedIntegratorOverOrdinaryConversation(t *testing.T) {
	state := NewAffectState()
	const plain = "今日はとても天気が良くて気持ちいいですね"
	var prompts []string
	for turn := range uint32(30) {
		state.Update(plain, turn, false)
		prompts = append(prompts, state.ToPrompt())
	}
	for key := DriveKey(0); key < driveCount; key++ {
		if level := state.Level(key); !(level > 0 && level < 1) {
			t.Errorf("%s pinned to a rail: %g", key, level)
		}
	}
	plateau := state.Level(Dopamine)
	if !(plateau > 0.6 && plateau < 1) {
		t.Errorf("dopamine plateau = %g, want within (0.6, 1)", plateau)
	}
	state.Update("うん", 30, false)
	if got := state.Level(Dopamine); got >= plateau {
		t.Errorf("dopamine remained at plateau: %g >= %g", got, plateau)
	}
	if prompts[0] == prompts[len(prompts)-1] {
		t.Error("prompt did not change over the conversation")
	}
}

func TestMinimalAckAfterClosingStaysSilent(t *testing.T) {
	state := NewAffectState()
	state.Update("じゃあね", 5, false)
	if !state.IsClosing() {
		t.Fatal("farewell did not set closing")
	}
	state.Update("うん", 6, false)
	if reply, ok := state.ForcedClosureReply("うん"); !state.IsClosing() || !ok || reply != "……" {
		t.Fatalf("closing = %v, forced reply = %q, %v", state.IsClosing(), reply, ok)
	}
}
