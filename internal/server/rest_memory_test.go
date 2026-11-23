package server

import (
	"errors"
	"fmt"
	"net/http"
	"path/filepath"
	"reflect"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
	"github.com/tik-choco/tc-npc/internal/memory"
	"github.com/tik-choco/tc-npc/internal/person"
)

func TestPersonInputAndOrdering(t *testing.T) {
	for _, tc := range []struct {
		err    string
		status int
	}{
		{`invalid person id: "../etc"`, http.StatusBadRequest},
		{"failed to parse person abc: unexpected EOF", http.StatusInternalServerError},
	} {
		if got := personErrorStatus(errors.New(tc.err)); got != tc.status {
			t.Errorf("status %d for %q, want %d", got, tc.err, tc.status)
		}
	}
	for _, blank := range []string{"", "   "} {
		if _, err := validatePersonName(blank); err == nil {
			t.Errorf("accepted blank person name %q", blank)
		}
	}
	if name, err := validatePersonName("  太郎  "); err != nil || name != "太郎" {
		t.Fatalf("name %q: %v", name, err)
	}
	a, b, c := person.NewPerson("太郎", "manual"), person.NewPerson("花子", "manual"), person.NewPerson("次郎", "manual")
	a.LastSeen, b.LastSeen, c.LastSeen = 100, 300, 200
	people := []person.Person{a, b, c}
	sortPeopleByLastSeenDesc(people)
	if !reflect.DeepEqual(people, []person.Person{b, c, a}) {
		t.Errorf("people sorted incorrectly: %v", people)
	}
}

func TestApplyPersonUpdate(t *testing.T) {
	p := person.NewPerson("太郎", "manual")
	p.Notes, p.Appearance, p.Aliases = "original notes", "original look", []string{"old-alias"}
	before := p
	applyPersonUpdate(&p, updatePersonRequest{})
	if !reflect.DeepEqual(p, before) {
		t.Fatal("empty patch changed the record")
	}
	applyPersonUpdate(&p, updatePersonRequest{Notes: pointer("updated notes")})
	if p.Name != "太郎" || p.Notes != "updated notes" || p.Appearance != "original look" {
		t.Fatalf("notes-only update: %+v", p)
	}
	replacement := []string{"new-alias-1", "new-alias-2"}
	applyPersonUpdate(&p, updatePersonRequest{Aliases: &replacement})
	if !reflect.DeepEqual(p.Aliases, replacement) {
		t.Fatalf("aliases after replacement: %v", p.Aliases)
	}
}

func TestPersonMemoryFiltersAndWireShape(t *testing.T) {
	record := func(doc string, ts int64, owner string) memory.LongTermRecord {
		out := memory.LongTermRecord{DocID: doc, Text: "text-" + doc, CreatedAt: ts}
		if owner != "" {
			out.Metadata = map[string]string{"person_id": owner}
		}
		return out
	}
	records := []memory.LongTermRecord{
		record("1", 300, "person-a"), record("2", 200, "person-b"),
		record("3", 100, "person-a"), record("4", 50, ""),
	}
	for _, tc := range []struct {
		id    string
		limit int
		want  []memory.LongTermRecord
	}{
		{"person-a", 50, []memory.LongTermRecord{records[0], records[2]}},
		{"person-a", 1, records[:1]},
		{"missing", 50, []memory.LongTermRecord{}},
	} {
		if got := filterPersonMemories(records, tc.id, tc.limit); !reflect.DeepEqual(got, tc.want) {
			t.Errorf("memories for %s/%d: %v, want %v", tc.id, tc.limit, got, tc.want)
		}
	}
	if wire := longTermRecordToWire(records[3]); wire["personId"] != "" || wire["docId"] != "4" {
		t.Fatalf("unattributed record: %v", wire)
	}
	if wire := longTermRecordToWire(records[0]); wire["personId"] != "person-a" || wire["createdAt"] != "1970-01-01T00:05:00Z" {
		t.Fatalf("attributed record: %v", wire)
	}
}

func TestChatHistoryWireOrderAndSpeaker(t *testing.T) {
	s := testState(t)
	h := s.routes()
	for _, entry := range []chatlog.ChatLogEntry{
		{Time: "2026-07-26T00:00:01Z", Speaker: pointer("太郎"), Input: "hi", Output: "hello"},
		{Time: "2026-07-26T00:00:00Z", Input: "hi", Output: "hello"},
	} {
		if err := chatlog.Append(s.mc.DataDir, &entry); err != nil {
			t.Fatal(err)
		}
	}
	response := request(t, h, http.MethodGet, "/api/chat/history", "", http.StatusOK)
	entries := decodeResponse[map[string][]object](t, response.Body.Bytes())["entries"]
	if len(entries) != 2 || entries[1]["speaker"] != "太郎" {
		t.Fatalf("chat entries: %v", entries)
	}
	if _, hasSpeaker := entries[0]["speaker"]; hasSpeaker || entries[0]["input"] != "hi" || entries[0]["output"] != "hello" {
		t.Fatalf("chat entry without a speaker: %v", entries[0])
	}
	response = request(t, h, http.MethodGet, "/api/chat/history?limit=1&limit=0", "", http.StatusOK)
	if response.Body.String() != `{"entries":[]}` {
		t.Fatalf("zero limit body: %s", response.Body)
	}
}

func TestPeopleAndMemoryRoutes(t *testing.T) {
	s := testState(t)
	h := s.routes()
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	created := request(t, h, http.MethodPost, "/api/people", `{"name":" 太郎 ","notes":"hi"}`, http.StatusOK)
	p := decodeResponse[map[string]object](t, created.Body.Bytes())["person"]
	id := p["id"].(string)
	path := "/api/people/" + id
	if p["name"] != "太郎" || p["source"] != "manual" || p["notes"] != "hi" {
		t.Fatalf("created person: %v", p)
	}
	if msg := receiveBus(t, sub); msg.Topic != bus.TopicUI || msg.Type != bus.MsgPersonUpdated || msg.Str("id") != id {
		t.Fatalf("unexpected person create broadcast: %v", msg)
	}
	duplicate := request(t, h, http.MethodPost, "/api/people", `{"name":"太郎さん"}`, http.StatusOK)
	if duplicate.Body.String() != created.Body.String() {
		t.Fatalf("duplicate name did not resolve to existing person: %s", duplicate.Body)
	}
	list := request(t, h, http.MethodGet, "/api/people", "", http.StatusOK)
	if people := decodeResponse[map[string][]object](t, list.Body.Bytes())["people"]; len(people) != 1 || people[0]["id"] != id {
		t.Fatalf("person list: %v", people)
	}
	data := fmt.Sprintf(`[{"doc_id":"1","text":"known","created_at":20,"metadata":{"person_id":%q},"embedding":[0.1]},{"doc_id":"2","text":"other","created_at":30}]`, id)
	putFile(t, filepath.Join(s.mc.DataDir, "memory-store.json"), []byte(data))
	get := request(t, h, http.MethodGet, path, "", http.StatusOK)
	memories := decodeResponse[object](t, get.Body.Bytes())["memories"].([]any)
	want := []any{object{"docId": "1", "text": "known", "createdAt": "1970-01-01T00:00:20Z"}}
	if !reflect.DeepEqual(memories, want) {
		t.Fatalf("person memories: %v", memories)
	}
	get = request(t, h, http.MethodGet, "/api/memory", "", http.StatusOK)
	entries := decodeResponse[object](t, get.Body.Bytes())["longTerm"].([]any)
	if len(entries) != 2 || entries[0].(map[string]any)["personId"] != "" ||
		entries[1].(map[string]any)["personId"] != id {
		t.Fatalf("long-term memories: %v", entries)
	}
	request(t, h, http.MethodPatch, path, `{"aliases":["alias"],"appearance":"new"}`, http.StatusOK)
	if msg := receiveBus(t, sub); msg.Type != bus.MsgPersonUpdated || msg.Str("appearance") != "new" {
		t.Fatalf("unexpected person patch broadcast: %v", msg)
	}
	request(t, h, http.MethodDelete, path, "", http.StatusOK)
	if msg := receiveBus(t, sub); msg.Type != bus.MsgPersonDeleted || msg.Str("id") != id {
		t.Fatalf("unexpected person deletion broadcast: %v", msg)
	}
	request(t, h, http.MethodGet, path, "", http.StatusNotFound)
	request(t, h, http.MethodGet, "/api/people/%2e%2e%2fescape", "", http.StatusBadRequest)
}
