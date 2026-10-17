package mesh

import (
	"tailscale.com/envknob"
	"tailscale.com/logtail"
)

// tsnet uploads its logs to Tailscale's log service (log.tailscale.io) by
// default. Allternit mesh nodes talk only to our own Headscale and DERP, so
// uploads are switched off for the whole process before any node starts.
func init() {
	envknob.SetNoLogsNoSupport()
	logtail.Disable()
}
