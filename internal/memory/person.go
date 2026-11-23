package memory

import (
	"fmt"
	"log/slog"
	"slices"
	"strings"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/person"
)

// Appearance text varies with lighting, pose, and wording. Cap the number
// of unmatched anonymous records a busy scene can create.
const maxUnnamedVisionPeople = 32

// The event loop owns the roster and decides when to persist a mutation.
type personTracker struct {
	dataDir  string
	people   map[string]*person.Person
	maxFacts uint32
}

func loadPersonTracker(dataDir string, maxFacts uint32) *personTracker {
	people, err := person.ListPeople(dataDir)
	if err != nil {
		slog.Warn("npc-memory: failed to list people, starting with an empty roster", "error", err)
	}
	t := &personTracker{dataDir: dataDir, people: make(map[string]*person.Person, len(people)), maxFacts: maxFacts}
	for i := range people {
		t.people[people[i].ID] = &people[i]
	}
	return t
}

func (t *personTracker) findByName(name string) *person.Person {
	roster := make([]person.Person, 0, len(t.people))
	for _, p := range t.people {
		roster = append(roster, *p)
	}
	if match := person.FindPerson(roster, name); match != nil {
		return t.people[match.ID]
	}
	return nil
}

func (t *personTracker) create(name, source string) *person.Person {
	p := person.NewPerson(name, source)
	t.people[p.ID] = &p
	return &p
}

func (t *personTracker) observeFromChat(name string) *person.Person {
	if person.NormalizePersonName(name) == "" {
		return nil
	}
	p := t.findByName(name)
	if p == nil {
		return t.create(name, "chat")
	}
	p.LastSeen = time.Now().Unix()
	p.EncounterCount++
	return p
}

func (t *personTracker) observeFromVision(name, appearance, note string) *person.Person {
	hasAppearance := strings.TrimSpace(appearance) != ""
	hasNote := strings.TrimSpace(note) != ""
	if strings.TrimSpace(name) == "" && !hasAppearance && !hasNote {
		return nil
	}
	p, created := t.resolveVisionPerson(name, appearance, hasAppearance)
	if p == nil {
		return nil
	}
	p.LastSeen = time.Now().Unix()
	// A fresh record already counts this sighting as its first encounter.
	if !created {
		p.EncounterCount++
	}
	if hasAppearance {
		p.Appearance = appearance
	}
	if hasNote {
		t.addFacts(p.ID, []string{note}, "vision")
	}
	return p
}

func (t *personTracker) resolveVisionPerson(name, appearance string, hasAppearance bool) (*person.Person, bool) {
	if person.NormalizePersonName(name) != "" {
		if p := t.findByName(name); p != nil {
			return p, false
		}
		return t.create(name, "vision"), true
	}

	unnamed := 0
	var undescribed *person.Person
	for _, p := range t.people {
		if hasAppearance && p.Appearance == appearance {
			return p, false
		}
		if p.Name == "" && p.Source == "vision" {
			unnamed++
			if p.Appearance == "" {
				undescribed = p
			}
		}
	}
	// Reuse an undescribed anonymous record after checking exact matches.
	if undescribed != nil {
		return undescribed, false
	}
	if unnamed >= maxUnnamedVisionPeople {
		slog.Warn("npc-memory: unnamed vision person cap reached, dropping unmatched sighting", "limit", maxUnnamedVisionPeople)
		return nil, false
	}
	return t.create("", "vision"), true
}

func (t *personTracker) addFacts(id string, facts []string, source string) {
	p := t.people[id]
	if p == nil {
		return
	}
	for _, text := range facts {
		text = strings.TrimSpace(text)
		if text == "" || slices.ContainsFunc(p.Facts, func(f person.PersonFact) bool { return f.Text == text }) {
			continue
		}
		p.Facts = append(p.Facts, person.PersonFact{Text: text, Source: source, CreatedAt: time.Now().Unix()})
	}
	if uint64(len(p.Facts)) > uint64(t.maxFacts) {
		p.Facts = slices.Delete(p.Facts, 0, len(p.Facts)-int(t.maxFacts))
	}
}

func (t *personTracker) persistAndPublish(b *bus.Bus, p *person.Person) {
	if err := person.SavePerson(t.dataDir, p); err != nil {
		slog.Warn("npc-memory: failed to save person record", "person_id", p.ID, "error", err)
	}
	b.Publish(bus.TopicUI, bus.MsgPersonUpdated, person.PersonToWire(p))
}

// This fixed Japanese profile is part of npc-talk's prompt context. The
// talk prompt already carries the language instruction for the reply.
func profileText(p *person.Person) string {
	var text strings.Builder
	if p.Name == "" {
		fmt.Fprintf(&text, "名前不明の相手です(これまで%d回接触)。", p.EncounterCount)
	} else {
		fmt.Fprintf(&text, "相手は「%s」さんです(これまで%d回会話)。", p.Name, p.EncounterCount)
	}
	if len(p.Facts) > 0 {
		text.WriteString("\n覚えていること:")
		for _, fact := range p.Facts {
			text.WriteString("\n- " + fact.Text)
		}
	}
	if p.Appearance != "" {
		text.WriteString("\n見た目: " + p.Appearance)
	}
	if p.Notes != "" {
		text.WriteString("\nメモ: " + p.Notes)
	}
	return text.String()
}
