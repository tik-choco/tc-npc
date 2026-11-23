package server

import (
	"errors"
	"net/http"
	"net/url"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
	"github.com/tik-choco/tc-npc/internal/memory"
	"github.com/tik-choco/tc-npc/internal/person"
)

const (
	maxLongTermDocs         = 200
	maxPersonMemories       = 50
	chatHistoryDefaultLimit = 200
	chatHistoryMaxLimit     = 1000
)

func (s *appState) apiMemory(w http.ResponseWriter, _ *http.Request) {
	s.cacheMu.Lock()
	shortTerm := s.shortTerm
	s.cacheMu.Unlock()

	records := memory.ReadLongTermRecords(s.mc.DataDir)
	records = records[:min(len(records), maxLongTermDocs)]
	out := make([]object, 0, len(records))
	for _, record := range records {
		out = append(out, longTermRecordToWire(record))
	}
	writeJSON(w, object{"shortTerm": shortTerm, "longTerm": out})
}

func formatCreatedAt(ts int64) string {
	t := time.Unix(ts, 0).UTC()
	year := t.Year()
	// Chrono's DateTime bounds apply to old or corrupt store records.
	if year < -262143 || year > 262142 {
		return ""
	}
	if year > 9999 {
		return "+" + t.Format(time.RFC3339)
	}
	return t.Format(time.RFC3339)
}

func memoryRecordFields(record memory.LongTermRecord) object {
	return object{"docId": record.DocID, "text": record.Text, "createdAt": formatCreatedAt(record.CreatedAt)}
}

func longTermRecordToWire(record memory.LongTermRecord) object {
	out := memoryRecordFields(record)
	out["personId"] = record.Metadata["person_id"]
	return out
}

func filterPersonMemories(records []memory.LongTermRecord, id string, limit int) []memory.LongTermRecord {
	out := []memory.LongTermRecord{}
	for _, record := range records {
		if len(out) >= limit {
			break
		}
		if personID, ok := record.Metadata["person_id"]; ok && personID == id {
			out = append(out, record)
		}
	}
	return out
}

func queryLimit(r *http.Request, fallback, maximum int) int {
	// Query<HashMap> uses the final occurrence of a key.
	limit := fallback
	for pair := range strings.SplitSeq(r.URL.RawQuery, "&") {
		key, value, _ := strings.Cut(pair, "=")
		if key, _ := url.QueryUnescape(key); key != "limit" {
			continue
		}
		value, _ = url.QueryUnescape(value)
		n, err := strconv.ParseUint(strings.TrimPrefix(value, "+"), 10, strconv.IntSize)
		limit = fallback
		if err == nil {
			limit = int(min(n, uint64(maximum)))
		}
	}
	return min(limit, maximum)
}

func recentAffectEntries(history []affectEntry, limit int) []affectEntry {
	return append([]affectEntry{}, history[len(history)-min(len(history), limit):]...)
}

func (s *appState) apiAffectHistory(w http.ResponseWriter, r *http.Request) {
	s.cacheMu.Lock()
	entries := recentAffectEntries(s.affectHistory, queryLimit(r, affectHistoryCap, affectHistoryCap))
	s.cacheMu.Unlock()
	writeJSON(w, object{"entries": entries})
}

func (s *appState) apiChatHistory(w http.ResponseWriter, r *http.Request) {
	entries := chatlog.LoadRecent(s.mc.DataDir, queryLimit(r, chatHistoryDefaultLimit, chatHistoryMaxLimit))
	sort.SliceStable(entries, func(i, j int) bool { return entries[i].Time < entries[j].Time })
	writeJSON(w, object{"entries": entries})
}

func personErrorStatus(err error) int {
	if strings.Contains(err.Error(), "invalid person id") {
		return http.StatusBadRequest
	}
	return http.StatusInternalServerError
}

func sortPeopleByLastSeenDesc(people []person.Person) {
	sort.SliceStable(people, func(i, j int) bool { return people[i].LastSeen > people[j].LastSeen })
}

func (s *appState) apiListPeople(w http.ResponseWriter, _ *http.Request) {
	people, err := person.ListPeople(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	sortPeopleByLastSeenDesc(people)
	out := make([]object, 0, len(people))
	for i := range people {
		out = append(out, person.PersonToWire(&people[i]))
	}
	writeJSON(w, object{"people": out})
}

type createPersonRequest struct {
	Name  string  `json:"name" required:"true"`
	Notes *string `json:"notes"`
}

func (req *createPersonRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func validatePersonName(name string) (string, error) {
	name = strings.TrimSpace(name)
	if name == "" {
		return "", errors.New("name is required")
	}
	return name, nil
}

func (s *appState) apiCreatePerson(w http.ResponseWriter, r *http.Request) {
	var req createPersonRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return
	}
	name, err := validatePersonName(req.Name)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}

	s.peopleMu.Lock()
	defer s.peopleMu.Unlock()
	people, err := person.ListPeople(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	if existing := person.FindPerson(people, name); existing != nil {
		writeJSON(w, object{"person": person.PersonToWire(existing)})
		return
	}
	p := person.NewPerson(name, "manual")
	p.Notes = valueOr(req.Notes, p.Notes)
	s.savePerson(w, &p)
}

func (s *appState) findPerson(w http.ResponseWriter, id string) *person.Person {
	p, err := person.LoadPerson(s.mc.DataDir, id)
	if err != nil {
		writeError(w, personErrorStatus(err), err.Error())
		return nil
	}
	if p == nil {
		writeError(w, http.StatusNotFound, "person not found: "+id)
	}
	return p
}

func (s *appState) apiGetPerson(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	p := s.findPerson(w, id)
	if p == nil {
		return
	}
	records := filterPersonMemories(memory.ReadLongTermRecords(s.mc.DataDir), id, maxPersonMemories)
	memories := make([]object, 0, len(records))
	for _, record := range records {
		memories = append(memories, memoryRecordFields(record))
	}
	writeJSON(w, object{"person": person.PersonToWire(p), "memories": memories})
}

type updatePersonRequest struct {
	Name       *string   `json:"name"`
	Aliases    *[]string `json:"aliases"`
	Notes      *string   `json:"notes"`
	Appearance *string   `json:"appearance"`
}

func (req *updatePersonRequest) UnmarshalJSON(data []byte) error {
	type fields struct {
		Name       *string    `json:"name"`
		Aliases    *[]*string `json:"aliases"`
		Notes      *string    `json:"notes"`
		Appearance *string    `json:"appearance"`
	}
	var wire fields
	if err := decodeFields(data, &wire, true); err != nil {
		return err
	}
	req.Name, req.Notes, req.Appearance = wire.Name, wire.Notes, wire.Appearance
	if wire.Aliases != nil {
		aliases := make([]string, 0, len(*wire.Aliases))
		for _, alias := range *wire.Aliases {
			if alias == nil {
				return errors.New("invalid type: null, expected a string in aliases")
			}
			aliases = append(aliases, *alias)
		}
		req.Aliases = &aliases
	}
	return nil
}

func applyPersonUpdate(p *person.Person, req updatePersonRequest) {
	p.Name = valueOr(req.Name, p.Name)
	p.Aliases = valueOr(req.Aliases, p.Aliases)
	p.Notes = valueOr(req.Notes, p.Notes)
	p.Appearance = valueOr(req.Appearance, p.Appearance)
}

func (s *appState) apiUpdatePerson(w http.ResponseWriter, r *http.Request) {
	var req updatePersonRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return
	}
	s.peopleMu.Lock()
	defer s.peopleMu.Unlock()
	if p := s.findPerson(w, r.PathValue("id")); p != nil {
		applyPersonUpdate(p, req)
		s.savePerson(w, p)
	}
}

func (s *appState) savePerson(w http.ResponseWriter, p *person.Person) {
	if err := person.SavePerson(s.mc.DataDir, p); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	wire := person.PersonToWire(p)
	s.mc.Bus.Publish(bus.TopicUI, bus.MsgPersonUpdated, wire)
	writeJSON(w, object{"person": wire})
}

func (s *appState) apiDeletePerson(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	s.peopleMu.Lock()
	defer s.peopleMu.Unlock()
	deleted, err := person.DeletePerson(s.mc.DataDir, id)
	switch {
	case err != nil:
		writeError(w, personErrorStatus(err), err.Error())
	case !deleted:
		writeError(w, http.StatusNotFound, "person not found: "+id)
	default:
		s.mc.Bus.Publish(bus.TopicUI, bus.MsgPersonDeleted, object{"id": id})
		writeJSON(w, object{"ok": true})
	}
}
