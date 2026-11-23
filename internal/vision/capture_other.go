//go:build !windows && !linux

package vision

import (
	"fmt"
	"image"

	"github.com/kbinani/screenshot"
)

func primaryDisplayBounds() image.Rectangle {
	if screenshot.NumActiveDisplays() == 0 {
		return image.Rectangle{}
	}
	return screenshot.GetDisplayBounds(0)
}

func captureWindow(_ string) (*image.RGBA, error) {
	return nil, fmt.Errorf("window capture is not supported on this platform")
}
