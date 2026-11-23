package vision

import (
	"bytes"
	"context"
	"encoding/base64"
	"fmt"
	"image"
	"image/png"
	"log/slog"
	"math"
	"os"
	"path/filepath"
	"strings"

	"github.com/kbinani/screenshot"
	"github.com/tik-choco/tc-npc/internal/config"
	"golang.org/x/image/draw"
)

// Limit multimodal payloads by scaling their longest edge to at most 1280 px.
const maxEdge = 1280

var lanczos3 = draw.Kernel{
	Support: 3,
	At: func(x float64) float64 {
		if x == 0 {
			return 1
		}
		x *= math.Pi
		return (math.Sin(x) / x) * (math.Sin(x/3) / (x / 3))
	},
}

func capture(ctx context.Context, cfg config.VisionConfig, dataDir string) (*image.RGBA, error) {
	var img *image.RGBA
	if cfg.CaptureMode == "active" && cfg.TargetWindowTitle != "" {
		var err error
		img, err = captureWindow(cfg.TargetWindowTitle)
		if err != nil {
			slog.Warn("vision: window capture failed, falling back to entire screen", "target", cfg.TargetWindowTitle, "error", err)
		}
	}
	if img == nil {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		bounds := primaryDisplayBounds()
		if bounds.Empty() {
			return nil, fmt.Errorf("no active displays found")
		}
		var err error
		img, err = screenshot.CaptureRect(bounds)
		if err != nil {
			return nil, fmt.Errorf("failed to capture monitor: %w", err)
		}
	}

	img = downscale(img)
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	if cfg.DebugSave {
		path := filepath.Join(dataDir, "debug_capture.png")
		if err := saveDebugPNG(path, img); err != nil {
			slog.Warn("vision: failed to save debug capture", "path", path, "error", err)
		} else {
			slog.Debug("vision: saved debug capture", "path", path)
		}
	}
	return img, nil
}

func downscale(img *image.RGBA) *image.RGBA {
	w, h := img.Bounds().Dx(), img.Bounds().Dy()
	longest := max(w, h)
	if longest <= maxEdge {
		return img
	}

	// The Rust image resize fits the original aspect ratio inside the rounded
	// target bounds. Reproduce that second fit when rounding goes downward.
	scale := float32(maxEdge) / float32(longest)
	targetW := max(1, math.Round(float64(float32(w)*scale)))
	targetH := max(1, math.Round(float64(float32(h)*scale)))
	ratio := min(targetW/float64(w), targetH/float64(h))
	newW := max(1, int(math.Round(float64(w)*ratio)))
	newH := max(1, int(math.Round(float64(h)*ratio)))
	dst := image.NewRGBA(image.Rect(0, 0, newW, newH))
	lanczos3.Scale(dst, dst.Bounds(), img, img.Bounds(), draw.Src, nil)
	return dst
}

// Keep the RGBA PNG color type even when the screen pixels are opaque.
type rgbaPNG struct{ *image.RGBA }

func (rgbaPNG) Opaque() bool { return false }

func encodeDataURL(img *image.RGBA) (string, error) {
	var buf bytes.Buffer
	if err := png.Encode(&buf, rgbaPNG{img}); err != nil {
		return "", err
	}
	return "data:image/png;base64," + base64.StdEncoding.EncodeToString(buf.Bytes()), nil
}

func saveDebugPNG(path string, img *image.RGBA) error {
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		return err
	}
	file, err := os.Create(path)
	if err != nil {
		return err
	}
	err = png.Encode(file, rgbaPNG{img})
	closeErr := file.Close()
	if err != nil {
		return err
	}
	return closeErr
}

func windowMatcher(target string) (matchApp bool, matches func(string) bool) {
	target = strings.ToLower(target)
	if stem, ok := strings.CutSuffix(target, ".exe"); ok {
		return true, func(app string) bool { return strings.Contains(strings.ToLower(app), stem) }
	}
	return false, func(title string) bool { return strings.Contains(strings.ToLower(title), target) }
}
