package sprite

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"image"
	"image/color"
	"image/draw"
	_ "image/jpeg"
	"image/png"
	"math"
	"strings"
)

const (
	generatedCellPx  = 128
	generatedColumns = 8
)

// SheetFromImageDataURL converts a tc-town PNG or JPEG portrait to one row
// of identical, cover-fitted cells. The renderer cycles eight columns,
// and a still portrait has no speaking animation to put in a second row.
func SheetFromImageDataURL(dataURL string) ([]byte, error) {
	_, encoded, ok := strings.Cut(dataURL, "base64,")
	if !ok {
		return nil, fmt.Errorf("avatar image is not a base64 data URL")
	}
	encoded = strings.TrimSpace(encoded)
	// Go's base64 decoder ignores internal newlines. Rust's standard
	// decoder accepts only the encoded content after trimming its edges.
	if strings.ContainsAny(encoded, "\r\n") {
		return nil, fmt.Errorf("avatar image is not valid base64")
	}
	data, err := base64.StdEncoding.Strict().DecodeString(encoded)
	if err != nil {
		return nil, fmt.Errorf("avatar image is not valid base64: %w", err)
	}
	source, format, err := image.Decode(bytes.NewReader(data))
	if err != nil {
		return nil, fmt.Errorf("avatar image could not be decoded: %w", err)
	}
	if format != "png" && format != "jpeg" {
		return nil, fmt.Errorf("avatar image could not be decoded: unsupported format %s", format)
	}

	cell := coverFitSquare(source, generatedCellPx)
	sheet := image.NewNRGBA(image.Rect(0, 0, generatedColumns*generatedCellPx, generatedCellPx))
	for column := range generatedColumns {
		offset := image.Pt(column*generatedCellPx, 0)
		draw.Draw(sheet, cell.Bounds().Add(offset), cell, image.Point{}, draw.Src)
	}

	var out bytes.Buffer
	if err := png.Encode(&out, sheet); err != nil {
		return nil, fmt.Errorf("failed to encode sprite sheet: %w", err)
	}
	return out.Bytes(), nil
}

// coverFitSquare scales the picture to fill the cell and crops the overflow
// from the center. Only the crop is sampled, so a very wide or tall image
// doesn't require allocating the entire scaled picture.
func coverFitSquare(source image.Image, size int) *image.NRGBA {
	bounds := source.Bounds()
	width, height := bounds.Dx(), bounds.Dy()
	cell := image.NewNRGBA(image.Rect(0, 0, size, size))
	if width == 0 || height == 0 {
		return cell
	}

	scale := max(float64(size)/float64(width), float64(size)/float64(height))
	scaledW := max(int(math.Ceil(float64(width)*scale)), size)
	scaledH := max(int(math.Ceil(float64(height)*scale)), size)
	if scaledW == width && scaledH == height && sampleMax(source) == 255 {
		draw.Draw(cell, cell.Bounds(), source, bounds.Min, draw.Src)
		return cell
	}

	xWeights := make([]sampleWeights, size)
	for x := range xWeights {
		xWeights[x] = lanczosWeights(width, scaledW, (scaledW-size)/2+x)
	}
	firstX := xWeights[0].first
	lastSample := xWeights[size-1]
	row := make([][4]float32, lastSample.first+len(lastSample.weights)-firstX)
	maxValue := sampleMax(source)
	for y := range size {
		yWeights := lanczosWeights(height, scaledH, (scaledH-size)/2+y)
		// Match the Rust image filter's vertical pass followed by its
		// horizontal pass. Keep the intermediate channels unrounded.
		clear(row)
		for i, weight := range yWeights.weights {
			sy := bounds.Min.Y + yWeights.first + i
			for x := range row {
				pixel := source.At(bounds.Min.X+firstX+x, sy)
				addWeighted(&row[x], pixelChannels(pixel, maxValue), weight)
			}
		}
		for x, weights := range xWeights {
			var pixel [4]float32
			for i, weight := range weights.weights {
				addWeighted(&pixel, row[weights.first-firstX+i], weight)
			}
			offset := cell.PixOffset(x, y)
			for channel, value := range pixel {
				value = min(max(value, 0), maxValue)
				rounded := int(math.Round(float64(value)))
				if maxValue == 65535 {
					rounded = (rounded + 128) / 257
				}
				cell.Pix[offset+channel] = byte(rounded)
			}
		}
	}
	return cell
}

func sampleMax(source image.Image) float32 {
	switch source.(type) {
	case *image.Gray16, *image.NRGBA64, *image.RGBA64:
		return 65535
	default:
		return 255
	}
}

func pixelChannels(pixel color.Color, maxValue float32) [4]float32 {
	if maxValue == 65535 {
		p := color.NRGBA64Model.Convert(pixel).(color.NRGBA64)
		return [4]float32{float32(p.R), float32(p.G), float32(p.B), float32(p.A)}
	}
	p := color.NRGBAModel.Convert(pixel).(color.NRGBA)
	return [4]float32{float32(p.R), float32(p.G), float32(p.B), float32(p.A)}
}

func addWeighted(sum *[4]float32, pixel [4]float32, weight float32) {
	for i, channel := range pixel {
		sum[i] += channel * weight
	}
}

type sampleWeights struct {
	first   int
	weights []float32
}

func lanczosWeights(sourceSize, scaledSize, index int) sampleWeights {
	ratio := float32(sourceSize) / float32(scaledSize)
	stretch := max(ratio, 1)
	support := 3 * stretch
	center := (float32(index) + 0.5) * ratio
	first := min(max(int(math.Floor(float64(center-support))), 0), sourceSize-1)
	end := min(max(int(math.Ceil(float64(center+support))), first+1), sourceSize)
	center -= 0.5

	weights := make([]float32, end-first)
	var sum float32
	for i := range weights {
		d := (float32(first+i) - center) / stretch
		if d > -3 && d < 3 {
			weights[i] = sinc(d) * sinc(d/3)
			sum += weights[i]
		}
	}
	for i := range weights {
		weights[i] /= sum
	}
	return sampleWeights{first: first, weights: weights}
}

func sinc(d float32) float32 {
	if d == 0 {
		return 1
	}
	x := d * math.Pi
	return float32(math.Sin(float64(x))) / x
}
