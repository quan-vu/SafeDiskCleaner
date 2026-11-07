package main

import (
	"image"
	"image/color"
	"image/png"
	"math"
	"os"
)

func main() {
	const n = 1024
	img := image.NewNRGBA(image.Rect(0, 0, n, n))
	for y := range n {
		for x := range n {
			dx, dy := float64(x-512), float64(y-512)
			d := math.Hypot(dx, dy)
			c := color.NRGBA{R: 16, G: 24, B: 42, A: 255}
			if d < 470 { c = color.NRGBA{R: 23, G: 36, B: 58, A: 255} }
			if d > 348 && d < 408 { c = color.NRGBA{R: 38, G: 181, B: 118, A: 255} }
			if d < 298 { c = color.NRGBA{R: 13, G: 20, B: 37, A: 255} }
			// A small rising meter represents reclaimable space.
			if x > 348 && x < 430 && y > 565 && y < 700 { c = color.NRGBA{R: 76, G: 202, B: 139, A: 255} }
			if x > 472 && x < 554 && y > 470 && y < 700 { c = color.NRGBA{R: 76, G: 202, B: 139, A: 255} }
			if x > 596 && x < 678 && y > 370 && y < 700 { c = color.NRGBA{R: 76, G: 202, B: 139, A: 255} }
			img.SetNRGBA(x, y, c)
		}
	}
	file, err := os.Create("resources/icon.png")
	if err != nil { panic(err) }
	defer file.Close()
	if err := png.Encode(file, img); err != nil { panic(err) }
}
