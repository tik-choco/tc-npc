package memory

import (
	"fmt"
	"strings"
	"testing"

	"github.com/tik-choco/tc-npc/internal/person"
)

func TestObserveFromChatCreatesThenMatches(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	first := tracker.observeFromChat("太郎さん")
	if first == nil || first.Name != "太郎さん" || first.Source != "chat" || first.EncounterCount != 1 {
		t.Fatalf("first encounter = %+v", first)
	}
	id := first.ID
	again := tracker.observeFromChat("太郎くん")
	if again == nil || again.ID != id || again.EncounterCount != 2 || len(tracker.people) != 1 {
		t.Errorf("repeat encounter = %+v, roster size %d", again, len(tracker.people))
	}
}

func TestObserveFromChatIgnoresEmptyName(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	for _, name := range []string{"", "   "} {
		if got := tracker.observeFromChat(name); got != nil {
			t.Errorf("observeFromChat(%q) = %+v", name, got)
		}
	}
	if len(tracker.people) != 0 {
		t.Errorf("roster size = %d, want 0", len(tracker.people))
	}
}

func TestObserveFromVisionMatchesByNameAndUpdatesAppearance(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	created := tracker.observeFromVision("花子", "赤い帽子", "")
	if created == nil || created.Source != "vision" || created.Appearance != "赤い帽子" || created.EncounterCount != 1 {
		t.Fatalf("initial sighting = %+v", created)
	}
	id := created.ID
	again := tracker.observeFromVision("花子さん", "青い帽子", "")
	if again == nil || again.ID != id || again.EncounterCount != 2 || again.Appearance != "青い帽子" {
		t.Errorf("repeat sighting = %+v", again)
	}
}

func TestObserveFromVisionMatchesUnnamedPerson(t *testing.T) {
	for _, tc := range []struct{ name, appearance, note string }{
		{"exact appearance", "青いパーカーの男性", ""},
		{"reuse undescribed person", "", "手を振っている"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			tracker := loadPersonTracker(t.TempDir(), 20)
			first := tracker.observeFromVision("", tc.appearance, tc.note)
			if first == nil || first.Appearance != tc.appearance {
				t.Fatalf("initial sighting = %+v", first)
			}
			id := first.ID
			second := tracker.observeFromVision("", "青いパーカーの男性", "")
			if second == nil || second.ID != id || second.EncounterCount != 2 || second.Appearance != "青いパーカーの男性" || len(tracker.people) != 1 {
				t.Errorf("repeat sighting = %+v, roster size %d", second, len(tracker.people))
			}
		})
	}
}

func TestObserveFromVisionIgnoresBlankSighting(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	if got := tracker.observeFromVision("", "", ""); got != nil || len(tracker.people) != 0 {
		t.Errorf("empty sighting = %+v, roster size %d", got, len(tracker.people))
	}
}

func TestObserveFromVisionCapsUnnamedPeople(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	for i := range maxUnnamedVisionPeople {
		appearance := fmt.Sprintf("見た目-%d", i)
		if got := tracker.observeFromVision("", appearance, ""); got == nil || got.Appearance != appearance {
			t.Fatalf("sighting %d = %+v", i, got)
		}
	}
	if len(tracker.people) != maxUnnamedVisionPeople {
		t.Errorf("at cap, roster size = %d", len(tracker.people))
	}
	if got := tracker.observeFromVision("", "見た目-over-cap", ""); got != nil || len(tracker.people) != maxUnnamedVisionPeople {
		t.Errorf("over cap = %+v, roster size %d", got, len(tracker.people))
	}
	if got := tracker.observeFromVision("次郎", "見た目-named", ""); got == nil || got.Name != "次郎" || len(tracker.people) != maxUnnamedVisionPeople+1 {
		t.Errorf("named over cap = %+v, roster size %d", got, len(tracker.people))
	}
}

func TestAddFactsDedupsAndTrimsOldest(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 2)
	p := tracker.observeFromChat("太郎")
	tracker.addFacts(p.ID, []string{"コーヒーが好き"}, "chat")
	tracker.addFacts(p.ID, []string{"犬を飼っている"}, "chat")
	tracker.addFacts(p.ID, []string{"コーヒーが好き"}, "chat")
	if len(p.Facts) != 2 || p.Facts[0].Text != "コーヒーが好き" {
		t.Fatalf("deduped facts = %+v", p.Facts)
	}
	tracker.addFacts(p.ID, []string{"猫アレルギーがある"}, "chat")
	if len(p.Facts) != 2 || p.Facts[0].Text != "犬を飼っている" || p.Facts[1].Text != "猫アレルギーがある" {
		t.Errorf("trimmed facts = %+v", p.Facts)
	}
}

func TestAddFactsIgnoresBlankTextAndUnknownPerson(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	p := tracker.observeFromChat("太郎")
	tracker.addFacts(p.ID, []string{"  "}, "chat")
	tracker.addFacts("does-not-exist", []string{"x"}, "chat")
	if len(p.Facts) != 0 || len(tracker.people) != 1 {
		t.Errorf("facts = %+v, roster size %d", p.Facts, len(tracker.people))
	}
}

func TestObserveFromVisionNoteBecomesFact(t *testing.T) {
	tracker := loadPersonTracker(t.TempDir(), 20)
	p := tracker.observeFromVision("", "帽子の男性", "手を振っている")
	if p == nil || len(p.Facts) != 1 || p.Facts[0].Text != "手を振っている" || p.Facts[0].Source != "vision" {
		t.Errorf("person with vision fact = %+v", p)
	}
}

func TestProfileTextIncludesOnlyNonemptySections(t *testing.T) {
	p := person.NewPerson("太郎", "chat")
	p.EncounterCount = 12
	p.Facts = []person.PersonFact{{Text: "コーヒーが好き", Source: "chat"}, {Text: "犬を飼っている", Source: "chat"}}
	p.Appearance = "青いパーカー"
	want := "相手は「太郎」さんです(これまで12回会話)。\n覚えていること:\n- コーヒーが好き\n- 犬を飼っている\n見た目: 青いパーカー"
	if got := profileText(&p); got != want {
		t.Errorf("profileText = %q, want %q", got, want)
	}
}

func TestProfileTextHandlesUnnamedPerson(t *testing.T) {
	p := person.NewPerson("", "vision")
	const want = "名前不明の相手です(これまで1回接触)。"
	if got := profileText(&p); got != want {
		t.Errorf("profileText = %q, want %q", got, want)
	}
}

func TestProfileTextIncludesNotes(t *testing.T) {
	p := person.NewPerson("花子", "manual")
	p.Notes = "常連さん"
	if got := profileText(&p); !strings.HasSuffix(got, "メモ: 常連さん") {
		t.Errorf("profileText = %q, want notes included", got)
	}
}
