package mesh

import (
	"fmt"
	"net"
	"net/netip"
	"strconv"
	"strings"
	"sync"

	"tailscale.com/net/netmon"
)

var (
	ifaceMu      sync.Mutex
	ifaceList    []netmon.Interface
	ifaceHookSet bool
)

// SetInterfaces hands tsnet the device's network interfaces. Android 11+
// denies apps the netlink route dump Go's net.Interfaces relies on
// ("netlinkrib: permission denied"), so the app lists its interfaces with
// java.net.NetworkInterface and passes them here, the way Tailscale's own
// Android client does. Call it before Start and again on every network change.
//
// One interface per line:
//
//	name index mtu up loopback pointtopoint multicast | addr/prefix addr/prefix
func SetInterfaces(spec string) error {
	list, err := parseInterfaces(spec)
	if err != nil {
		return err
	}
	ifaceMu.Lock()
	defer ifaceMu.Unlock()
	ifaceList = list
	if !ifaceHookSet {
		netmon.RegisterInterfaceGetter(func() ([]netmon.Interface, error) {
			ifaceMu.Lock()
			defer ifaceMu.Unlock()
			return append([]netmon.Interface(nil), ifaceList...), nil
		})
		ifaceHookSet = true
	}
	return nil
}

func parseInterfaces(spec string) ([]netmon.Interface, error) {
	var out []netmon.Interface
	for _, line := range strings.Split(spec, "\n") {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}
		head, addrPart, _ := strings.Cut(line, "|")
		f := strings.Fields(head)
		if len(f) != 7 {
			return nil, fmt.Errorf("mesh: interface line needs 7 fields before '|': %q", line)
		}
		index, err1 := strconv.Atoi(f[1])
		mtu, err2 := strconv.Atoi(f[2])
		if err1 != nil || err2 != nil {
			return nil, fmt.Errorf("mesh: bad index or mtu in %q", line)
		}
		var flags net.Flags
		for i, flag := range []net.Flags{net.FlagUp, net.FlagLoopback, net.FlagPointToPoint, net.FlagMulticast} {
			if f[3+i] == "true" {
				flags |= flag
			}
		}
		if flags&net.FlagUp != 0 {
			flags |= net.FlagRunning
		}
		var addrs []net.Addr
		for _, a := range strings.Fields(addrPart) {
			p, err := netip.ParsePrefix(a)
			if err != nil {
				return nil, fmt.Errorf("mesh: bad address %q: %v", a, err)
			}
			addrs = append(addrs, &net.IPNet{IP: p.Addr().AsSlice(), Mask: net.CIDRMask(p.Bits(), p.Addr().BitLen())})
		}
		out = append(out, netmon.Interface{
			Interface: &net.Interface{Index: index, MTU: mtu, Name: f[0], Flags: flags},
			AltAddrs:  addrs,
		})
	}
	return out, nil
}
