package action

import (
	"encoding/json"
	"strings"
	"testing"
)

func TestStaticPromptOmitsLanguageRuleWhenAuto(t *testing.T) {
	prompt := buildStaticSystemPrompt("auto")
	if !strings.Contains(prompt, "利用可能なアクション:") || strings.Contains(prompt, `"message" field`) {
		t.Fatalf("unexpected prompt: %s", prompt)
	}
}

func TestStaticPromptConstrainsOnlyMessageField(t *testing.T) {
	prompt := buildStaticSystemPrompt("zh")
	for _, line := range actionDescriptions {
		if !strings.Contains(prompt, line) {
			t.Errorf("missing action line: %s", line)
		}
	}
	if !strings.Contains(prompt, "Simplified Chinese") || !strings.Contains(prompt, "action names and JSON keys stay as written") {
		t.Fatalf("missing language rule: %s", prompt)
	}
}

func TestActionResponseSchema(t *testing.T) {
	for _, input := range []string{
		`{"message":"OK","actions":[{"name":"jump"}]}`,
		`["OK",[["jump",[]]]]`,
	} {
		var response actionResponse
		if err := json.Unmarshal([]byte(input), &response); err != nil {
			t.Fatal(err)
		}
		if response.Message != "OK" || len(response.Actions) != 1 || response.Actions[0].Name != "jump" || len(response.Actions[0].Args) != 0 {
			t.Fatalf("response = %+v", response)
		}
	}

	var response actionResponse
	if encoded, err := json.Marshal(actionItem{Name: "jump"}); err != nil || string(encoded) != `{"name":"jump","args":[]}` {
		t.Fatalf("encoded action = %s, error = %v", encoded, err)
	}
	if err := json.Unmarshal([]byte(`{}`), &response); err != nil || response.Message != "" || len(response.Actions) != 0 {
		t.Fatalf("default response = %+v, error = %v", response, err)
	}
	for _, input := range []string{
		`null`, `{"message":null}`, `{"actions":null}`, `{"actions":[{}]}`,
		`{"actions":[{"name":null}]}`, `{"actions":[{"name":"jump","args":null}]}`,
		`{"actions":[{"name":"jump","args":[null]}]}`, `{"actions":[{"name":"jump","args":[1]}]}`,
		`{"actions":[{"Name":"jump"}]}`, `{"actions":[{"name":"jump","name":"w"}]}`,
	} {
		if err := json.Unmarshal([]byte(input), &response); err == nil {
			t.Errorf("accepted malformed response %s", input)
		}
	}
}
