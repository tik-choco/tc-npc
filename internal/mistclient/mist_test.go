package mistclient

import (
	"encoding/json"
	"reflect"
	"slices"
	"testing"

	"github.com/tik-choco/tc-npc/internal/config"
)

func TestBuildsOnlyUsableSharesAndPreservesResolutionOrder(t *testing.T) {
	cfg := readConfig(t, `{"providers":[
		{"id":"z","base_url":"http://z","api_key":"secret"},
		{"id":"a","base_url":"https://a"},
		{"id":"off","base_url":"http://off","enabled":false},
		{"id":"unused","base_url":"http://unused"},
		{"id":"r","base_url":"mist-network://z-room","provide":true,"shared":[
			{"provider_id":"z","model":"same"},{"provider_id":"a","model":"same"},
			{"provider_id":"off","model":"bad"},{"provider_id":"r","model":"loop"},
			{"provider_id":"missing","model":"bad"},{"provider_id":"z","model":"same"}]},
		{"id":"consumer","base_url":"mist-network://a-room"},
		{"id":"disabled-room","base_url":"mist-network://off-room","enabled":false,"provide":true}
	]}`)
	built := BuildRegistration(&cfg)
	if len(built.Providers) != 2 || built.Providers[0].ID != "a" || built.Providers[1].ID != "z" {
		t.Fatal("expected sorted providers a and z")
	}
	if built.Providers[1].APIKey != "secret" {
		t.Fatal("expected the real API key in the payload")
	}
	if len(built.Rooms) != 2 || built.Rooms[0].Room != "a-room" || built.Rooms[1].Room != "z-room" {
		t.Fatalf("expected sorted rooms a-room and z-room, got %+v", built.Rooms)
	}
	if !built.Rooms[0].Consume || built.Rooms[0].Provide || !built.Rooms[1].Provide {
		t.Fatalf("unexpected room participation: %+v", built.Rooms)
	}
	want := []config.ModelRef{{ProviderID: "z", Model: "same"}, {ProviderID: "a", Model: "same"}}
	if !slices.Equal(built.Rooms[1].Shared, want) {
		t.Fatalf("shares = %+v, want %+v", built.Rooms[1].Shared, want)
	}
	slices.Reverse(cfg.Providers)
	if !reflect.DeepEqual(BuildRegistration(&cfg), built) {
		t.Fatal("reversing providers changed the registration")
	}
	cfg.Providers = []config.ProviderConfig{config.DefaultProviderConfig()}
	if len(BuildRegistration(&cfg).Rooms) != 0 {
		t.Fatal("expected no rooms from a default provider")
	}
}

func TestEncodesRoomSegment(t *testing.T) {
	const want = "http://localhost/v1/rooms/a%2Fb%20%25%E6%97%A5%E6%9C%AC"
	if got := RoomBaseURL("http://localhost/v1/", "a/b %日本"); got != want {
		t.Fatalf("room URL = %q, want %q", got, want)
	}
}

func TestRegistrationJSONPreservesLineSeparators(t *testing.T) {
	registration := Registration{Owner: Owner, Rooms: []ExternalRoom{{Room: `\u2028` + "\u2028\u2029"}}}
	data, err := registration.MarshalJSON()
	const want = `{"owner":"tc-npc","label":"","providers":[],"rooms":[{"room":"\\u2028` + "\u2028\u2029" + `","consume":false,"provide":false,"shared":[]}]}`
	if err != nil || string(data) != want {
		t.Fatalf("registration JSON = %q, %v, want %q", data, err, want)
	}
}

func TestExternalGetJSON(t *testing.T) {
	const want = `{"registrations":[{"owner":"tc-npc","rooms":[],"updated_at":null,"status":{"rooms":[]}}]}`
	for _, input := range []string{
		`{"registrations":[{"owner":"tc-npc","providers":[{"api_key":"secret"}]}]}`,
		`[[["tc-npc"]]]`,
	} {
		var got ExternalGet
		if err := json.Unmarshal([]byte(input), &got); err != nil {
			t.Fatal(err)
		}
		data, err := json.Marshal(got)
		if err != nil || string(data) != want {
			t.Fatalf("get JSON = %s, %v, want %s", data, err, want)
		}
	}
	for _, input := range []string{
		`{}`, `null`, `{"registrations":null}`, `{"registrations":[null]}`,
		`{"registrations":[{"Owner":"tc-npc"}]}`,
		`{"registrations":[{"owner":"tc-npc","owner":"tc-npc"}]}`,
		`{"registrations":[{"owner":"tc-npc","status":null}]}`,
		`{"registrations":[{"owner":"tc-npc","status":{"rooms":[{"room":"a","joined":false,"providing":false,"peers":0,"models":[null]}]}}]}`,
	} {
		var got ExternalGet
		if err := json.Unmarshal([]byte(input), &got); err == nil {
			t.Errorf("accepted invalid response %s", input)
		}
	}
}

func readConfig(t *testing.T, input string) config.Config {
	t.Helper()
	var cfg config.Config
	if err := json.Unmarshal([]byte(input), &cfg); err != nil {
		t.Fatal(err)
	}
	return cfg
}
