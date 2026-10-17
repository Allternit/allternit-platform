package main

import (
	"tailscale.com/envknob"
	"tailscale.com/logtail"
)

// Same rule as package mesh (privacy.go): no log uploads to Tailscale's log
// service; this sidecar talks only to our own Headscale and DERP.
func init() {
	envknob.SetNoLogsNoSupport()
	logtail.Disable()
}
