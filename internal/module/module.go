// Package module defines the Module interface every subsystem (talk,
// memory, speech, vision, action, scheduler, translate, server) implements.
// Each subsystem package exposes
//
//	func New(mc module.Ctx) (module.Module, error)
//
// and main drives Run until the context is cancelled. A constructor or Run
// error is logged by the caller and does not take the process down.
package module

import (
	"context"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

// Ctx is the shared context handed to every module.
type Ctx struct {
	Bus *bus.Bus
	// Config is the startup snapshot. Modules that hot-reload watch
	// bus.TopicConfig / bus.MsgConfigUpdated instead of re-reading this.
	Config *config.Config
	// ConfigPath is the resolved path of the loaded (or default) config
	// file; modules that persist settings write here.
	ConfigPath string
	// DataDir is the app data directory (~/.tc-npc/).
	DataDir string
}

// Module is a long-running subsystem. Run should return once ctx is
// cancelled (or on unrecoverable error).
type Module interface {
	Name() string
	Run(ctx context.Context, mc Ctx) error
}
