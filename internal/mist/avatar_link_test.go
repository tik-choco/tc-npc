package mist

import (
	"fmt"
	"testing"

	"github.com/google/uuid"
)

func TestDecodesInputWithUnknownFields(t *testing.T) {
	data := []byte(`{"v":1,"t":"input","id":"i1","from":"h1","role":"host","ts":1,
		"text":"Hi","speaker":"viewer","to":"b1","source":"chat","future":true}`)
	host, ok := decodeHost(data)
	if !ok || host.id != "i1" || host.from != "h1" || host.kind != "input" || host.text != "Hi" ||
		host.speaker == nil || *host.speaker != "viewer" || host.to == nil || *host.to != "b1" {
		t.Fatalf("unexpected input: %+v, accepted = %v", host, ok)
	}
}

func TestIgnoresWrongVersionRoleAndUnknownType(t *testing.T) {
	for _, tc := range []struct {
		version    int
		role, kind string
	}{{2, "host", "bye"}, {1, "brain", "bye"}, {1, "host", "future"}} {
		data := fmt.Appendf(nil, `{"v":%d,"t":%q,"role":%q,"id":"i1","from":"h1","ts":1}`, tc.version, tc.kind, tc.role)
		if _, ok := decodeHost(data); ok {
			t.Errorf("accepted %s", data)
		}
	}
}

func TestEncodesSayWithCamelCaseFields(t *testing.T) {
	id := uuid.MustParse("00000000-0000-0000-0000-000000000001")
	lineID := uuid.MustParse("00000000-0000-0000-0000-000000000002")
	text, reply := "こんにちは<&>\u2028\u2029"+`\u2028`, "i1"
	data, err := encodeBrain("b1", id, 123, brainMessage{
		Kind: "say", LineID: &lineID, Text: &text, Audio: audioBrain, InReplyTo: &reply,
	})
	want := `{"v":1,"id":"00000000-0000-0000-0000-000000000001","from":"b1","role":"brain","ts":123,` +
		`"t":"say","lineId":"00000000-0000-0000-0000-000000000002","text":"こんにちは<&>` + "\u2028\u2029" +
		`\\u2028","audio":"brain","inReplyTo":"i1"}`
	if err != nil || string(data) != want {
		t.Fatalf("encode say = %q, %v\nwant %q", data, err, want)
	}
}

func TestOmitsAbsentOptionalFields(t *testing.T) {
	data, err := encodeBrain("b1", uuid.Nil, 123, brainMessage{Kind: "silent"})
	want := `{"v":1,"id":"00000000-0000-0000-0000-000000000000","from":"b1","role":"brain","ts":123,"t":"silent"}`
	if err != nil || string(data) != want {
		t.Fatalf("encode silent = %q, %v\nwant %q", data, err, want)
	}
}
