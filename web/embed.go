// Package web contains the built web UI.
package web

import "embed"

// Dist holds the built web UI under dist/.
//
//go:embed all:dist
var Dist embed.FS
