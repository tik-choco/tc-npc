//go:build desktop && production

package desktop

import (
	"context"
	"fmt"
	"net/http"
	"net/url"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
)

const healthcheckTimeout = 15 * time.Second

// RunApp opens the main window and its managed mascot against an already
// started server. Call it on the main goroutine. It blocks until the UI exits,
// then calls shutdown once. A startup error also triggers shutdown.
func RunApp(serverURL string, shutdown func()) error {
	if shutdown != nil {
		defer shutdown()
	}
	base, err := serverBaseURL(serverURL)
	if err != nil {
		return err
	}
	if err := waitForServer(base); err != nil {
		return err
	}
	child, err := startMascot(base.String())
	if err != nil {
		return err
	}
	defer child.stop()
	return runWindow(base, config.DataDir(), windowMode{
		title: "tc-npc", stateKey: "main", legacyLabel: "main", width: 1024, height: 768,
	}, child)
}

func waitForServer(base url.URL) error {
	healthURL := base
	healthURL.Path = "/healthz"
	ctx, cancel := context.WithTimeout(context.Background(), healthcheckTimeout)
	defer cancel()
	client := &http.Client{
		Timeout:       time.Second,
		CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
	}
	retry := time.NewTicker(100 * time.Millisecond)
	defer retry.Stop()
	for {
		req, err := http.NewRequestWithContext(ctx, http.MethodGet, healthURL.String(), nil)
		if err != nil {
			return fmt.Errorf("create desktop health check: %w", err)
		}
		res, err := client.Do(req)
		if err == nil {
			res.Body.Close()
			if res.StatusCode == http.StatusOK {
				return nil
			}
		}
		select {
		case <-ctx.Done():
			return fmt.Errorf("timed out waiting for tc-npc server at %s to become ready: %w", base.String(), ctx.Err())
		case <-retry.C:
		}
	}
}
