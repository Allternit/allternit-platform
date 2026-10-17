package mesh

import (
	"net"
	"testing"
)

func TestParseInterfaces(t *testing.T) {
	list, err := parseInterfaces("lo 1 65536 true true false false | 127.0.0.1/8 ::1/128\nwlan0 30 1500 true false false true | 10.0.0.11/24 fe80::1/64\n\nrmnet_data0 12 1500 false false true false |\n")
	if err != nil {
		t.Fatal(err)
	}
	if len(list) != 3 {
		t.Fatalf("got %d interfaces, want 3", len(list))
	}
	w := list[1]
	if w.Name != "wlan0" || w.Index != 30 || w.MTU != 1500 || w.Flags&net.FlagUp == 0 || w.Flags&net.FlagMulticast == 0 || w.Flags&net.FlagLoopback != 0 {
		t.Fatalf("wlan0 parsed wrong: %+v", w.Interface)
	}
	if len(w.AltAddrs) != 2 || w.AltAddrs[0].String() != "10.0.0.11/24" {
		t.Fatalf("wlan0 addrs: %v", w.AltAddrs)
	}
	if list[2].Flags&net.FlagUp != 0 || list[2].Flags&net.FlagPointToPoint == 0 || len(list[2].AltAddrs) != 0 {
		t.Fatalf("rmnet parsed wrong: %+v %v", list[2].Interface, list[2].AltAddrs)
	}
}

func TestParseInterfacesRejectsBadLines(t *testing.T) {
	for _, bad := range []string{"wlan0 30 | 10.0.0.1/24", "wlan0 x 1500 true false false true |", "wlan0 3 1500 true false false true | 10.0.0.1"} {
		if _, err := parseInterfaces(bad); err == nil {
			t.Errorf("accepted %q", bad)
		}
	}
}
