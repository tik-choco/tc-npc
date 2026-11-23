package person

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func TestSaveAndLoadRoundTrip(t *testing.T) {
	dir := t.TempDir()
	p := NewPerson("太郎", "manual")
	p.Aliases = append(p.Aliases, "たろちゃん")
	p.Facts = append(p.Facts, PersonFact{Text: "犬を飼っている", Source: "chat", CreatedAt: 12345})
	if err := SavePerson(dir, &p); err != nil {
		t.Fatal(err)
	}
	got, err := LoadPerson(dir, p.ID)
	if err != nil || !reflect.DeepEqual(got, &p) {
		t.Errorf("loaded %+v, %v; want %+v", got, err, p)
	}
}

func TestLoadMissingPersonReturnsNil(t *testing.T) {
	if p, err := LoadPerson(t.TempDir(), "does-not-exist"); p != nil || err != nil {
		t.Errorf("got %+v, %v for missing person", p, err)
	}
}

func TestListPeopleOnMissingDirIsEmpty(t *testing.T) {
	if people, err := ListPeople(t.TempDir()); len(people) != 0 || err != nil {
		t.Errorf("got %+v, %v for missing dir", people, err)
	}
}

func TestListPeopleSkipsCorruptFilesAndKeepsRest(t *testing.T) {
	dir := t.TempDir()
	p := NewPerson("花子", "manual")
	if err := SavePerson(dir, &p); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(PeopleDir(dir), "broken.json"), []byte("{ not json"), 0o644); err != nil {
		t.Fatal(err)
	}
	people, err := ListPeople(dir)
	if err != nil || len(people) != 1 || people[0].ID != p.ID {
		t.Errorf("listed %+v, %v", people, err)
	}
}

func TestDeletePersonRemovesFileAndReportsExistence(t *testing.T) {
	dir := t.TempDir()
	p := NewPerson("次郎", "manual")
	if err := SavePerson(dir, &p); err != nil {
		t.Fatal(err)
	}
	if removed, err := DeletePerson(dir, p.ID); !removed || err != nil {
		t.Errorf("deleted %v, %v", removed, err)
	}
	if loaded, err := LoadPerson(dir, p.ID); loaded != nil || err != nil {
		t.Errorf("loaded after delete %+v, %v", loaded, err)
	}
	if removed, err := DeletePerson(dir, p.ID); removed || err != nil {
		t.Errorf("deleted again %v, %v", removed, err)
	}
}

func TestRejectsInvalidIDsToPreventPathTraversal(t *testing.T) {
	dir := t.TempDir()
	for _, id := range []string{"../../etc/passwd", "", "some/slash", "bad id with spaces", "日本語", "a_b"} {
		if _, err := LoadPerson(dir, id); err == nil {
			t.Errorf("load accepted %q", id)
		}
		if _, err := DeletePerson(dir, id); err == nil {
			t.Errorf("delete accepted %q", id)
		}
		if err := SavePerson(dir, &Person{ID: id}); err == nil {
			t.Errorf("save accepted %q", id)
		}
	}
}

func TestNormalizePersonNameStripsHonorificsAndFoldsCase(t *testing.T) {
	for _, tt := range []struct{ name, want string }{
		{"太郎さん", "太郎"}, {"花子ちゃん", "花子"}, {"次郎くん", "次郎"},
		{"鈴木様", "鈴木"}, {"田中氏", "田中"}, {"山田先生", "山田"},
		{"鈴木せんせい", "鈴木"}, {"　太郎　", "太郎"}, {"  Alice  ", "alice"},
		{"太郎", "太郎"}, {" 山田　太郎　さん　", "山田 太郎"}, {"太郎先生さん", "太郎先生"},
		{"İΟΣ", "i\u0307ος"},
	} {
		if got := NormalizePersonName(tt.name); got != tt.want {
			t.Errorf("NormalizePersonName(%q) = %q; want %q", tt.name, got, tt.want)
		}
	}
}

func TestFindPersonMatchesByNameOrAlias(t *testing.T) {
	a := NewPerson("太郎", "manual")
	a.Aliases = append(a.Aliases, "たろ")
	people := []Person{a, NewPerson("花子", "manual")}
	for _, name := range []string{"太郎さん", "たろ"} {
		if got := FindPerson(people, name); got != &people[0] {
			t.Errorf("FindPerson(%q) = %+v", name, got)
		}
	}
	for _, name := range []string{"見知らぬ人", ""} {
		if got := FindPerson(people, name); got != nil {
			t.Errorf("FindPerson(%q) = %+v", name, got)
		}
	}
}

func TestPersonToWireUsesCamelCaseKeys(t *testing.T) {
	p := NewPerson("太郎", "chat")
	p.Facts = append(p.Facts, PersonFact{Text: "コーヒーが好き", Source: "chat", CreatedAt: 999})
	want := map[string]any{
		"id": p.ID, "name": "太郎", "aliases": []string{},
		"firstSeen": p.FirstSeen, "lastSeen": p.LastSeen, "encounterCount": uint32(1),
		"appearance": "", "facts": []map[string]any{{"text": "コーヒーが好き", "source": "chat", "createdAt": int64(999)}},
		"familiarity": float64(0), "source": "chat", "notes": "",
	}
	if wire := PersonToWire(&p); !reflect.DeepEqual(wire, want) {
		t.Errorf("wire = %+v; want %+v", wire, want)
	}
}

func TestPersonJSONUsesRustDefaultsAndEmptyArrays(t *testing.T) {
	var p Person
	if err := json.Unmarshal([]byte(`{"id":"legacy"}`), &p); err != nil {
		t.Fatal(err)
	}
	if p.Aliases == nil || p.Facts == nil {
		t.Errorf("missing fields did not default to empty arrays: %+v", p)
	}
	data, err := json.Marshal(Person{ID: "legacy"})
	const want = `{"id":"legacy","name":"","aliases":[],"first_seen":0,"last_seen":0,"encounter_count":0,"appearance":"","facts":[],"familiarity":0,"source":"","notes":""}`
	if err != nil || string(data) != want {
		t.Errorf("encoded %s, %v; want %s", data, err, want)
	}
}
