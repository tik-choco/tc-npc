//go:build !desktop

package desktop

import "testing"

func TestHeadlessDesktopAPI(t *testing.T) {
	want := "this build has no desktop UI; rebuild with -tags desktop, or use `tc-npc serve`"
	for _, err := range []error{RunApp("", nil), RunMascot("")} {
		if err == nil || err.Error() != want {
			t.Errorf("headless desktop error = %v, want %q", err, want)
		}
	}
}
