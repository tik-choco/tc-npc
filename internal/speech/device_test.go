package speech

import (
	"encoding/json"
	"os"
	"strings"
	"testing"

	"github.com/tik-choco/tc-npc/internal/audio"
)

func TestAudioDevicesRESTJSON(t *testing.T) {
	input, defaultInput := deviceNames([]audio.DeviceInfo{{Name: "Mic", IsDefault: true}, {Name: "Headset"}}, nil)
	output, defaultOutput := deviceNames(nil, audio.ErrUnsupported)
	data, err := json.Marshal(AudioDevices{input, output, defaultInput, defaultOutput})
	if err != nil {
		t.Fatal(err)
	}
	if want := `{"input":["Mic","Headset"],"output":[],"default_input":"Mic","default_output":null}`; string(data) != want {
		t.Errorf("device JSON = %s, want %s", data, want)
	}
}

func TestHardwareListDevicesReturnsUniqueNamedDevices(t *testing.T) {
	if os.Getenv("TC_NPC_AUDIO_HW_TEST") != "1" {
		t.Skip("set TC_NPC_AUDIO_HW_TEST=1 to enumerate host audio devices")
	}
	devices := ListDevices()
	for _, names := range [][]string{devices.Input, devices.Output} {
		seen := make(map[string]bool)
		for _, name := range names {
			if strings.TrimSpace(name) == "" || seen[name] {
				t.Errorf("duplicate or blank device name in %q", names)
			}
			seen[name] = true
		}
	}
}
