package speech

import "github.com/tik-choco/tc-npc/internal/audio"

// AudioDevices is the response for GET /api/audio/devices. A nil default
// means the OS reported no named default endpoint for that direction.
type AudioDevices struct {
	Input         []string `json:"input"`
	Output        []string `json:"output"`
	DefaultInput  *string  `json:"default_input"`
	DefaultOutput *string  `json:"default_output"`
}

// ListDevices lists the host's selectable endpoints without opening streams.
// An unavailable direction is returned as an empty array with a null default.
func ListDevices() AudioDevices {
	input, defaultInput := deviceNames(audio.ListInputDevices())
	output, defaultOutput := deviceNames(audio.ListOutputDevices())
	return AudioDevices{Input: input, Output: output, DefaultInput: defaultInput, DefaultOutput: defaultOutput}
}

func deviceNames(devices []audio.DeviceInfo, err error) ([]string, *string) {
	names := make([]string, 0, len(devices))
	var defaultName *string
	if err == nil {
		for _, device := range devices {
			names = append(names, device.Name)
			if device.IsDefault {
				defaultName = &device.Name
			}
		}
	}
	return names, defaultName
}
