// Command mesh-node is a small tsnet sidecar for `gizzi serve --mesh`. It
// joins the tailnet in pure userspace mode (no root, no TUN, no system
// tailscaled) and listens on the tailnet, forwarding each accepted connection
// to the local gizzi port. Unlike a userspace tailscaled — which gives the
// host no routable 100.x interface — tsnet's Listen accepts inbound tailnet
// connections directly, so the node's mesh IP is actually reachable from
// other tailnet nodes (e.g. the iOS app with its embedded tsnet).
//
// Reverse mode (`--reverse host:port`) is the mirror image for tailnet
// CLIENTS (e.g. the Allternit desktop app): the node listens on 127.0.0.1
// with an ephemeral port and dials the fixed tailnet target through the
// tsnet server per connection, so local apps that cannot route 100.x can
// reach a mesh-registered instance via the loopback URL. Mirrors StartProxy
// in infrastructure/mesh/tsnet-ios/mesh.go (the iOS loopback proxy).
//
// Contract with the parent process (gizzi-code's mesh.ts, or the desktop
// app's mesh-manager.ts): on success `MESH_READY ip=<100.x addr>` is printed
// to stdout once the node is up and its listener is bound; in reverse mode a
// second line, `PROXY_READY port=<n>`, follows with the chosen loopback port.
// These are the only stdout output. Any failure prints
// `MESH_ERROR reason=...` to stderr and exits non-zero. SIGTERM/SIGINT shut
// down cleanly (exit 0).
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net"
	"net/netip"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"tailscale.com/tailcfg"
	"tailscale.com/tsnet"
)

// keepWarmInterval paces reverse-mode keepalive pings. An idle tailnet peer
// is trimmed after a few minutes, and the next dial then spends ~10s
// rediscovering the path (measured 2026-09-30 to the paired Mail VPS: 10.6s
// cold vs <1s warm), which a person opening a computer's screen sees as a
// hung viewer. A TSMP ping every 25s keeps the WireGuard session alive.
const keepWarmInterval = 25 * time.Second

const defaultControlURL = "https://allternit-headscale.fly.dev"

func defaultDataDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return filepath.Join(os.TempDir(), "gizzi-mesh-node")
	}
	return filepath.Join(home, ".local", "share", "gizzi-code", "mesh-node")
}

func main() {
	hostname := flag.String("hostname", "gizzi-node", "hostname to join the tailnet under")
	controlURL := flag.String("control-url", defaultControlURL, "coordination server (Headscale) URL")
	authKey := flag.String("auth-key", os.Getenv("MESH_AUTH_KEY"), "pre-auth key (or env MESH_AUTH_KEY)")
	dataDir := flag.String("data-dir", defaultDataDir(), "tsnet state directory (node key lives here)")
	forwardPort := flag.Int("forward", 4096, "local gizzi port to proxy tailnet connections to")
	listenPort := flag.Int("listen", 0, "tailnet-side listen port (default: same as --forward)")
	reverseTarget := flag.String("reverse", "", "reverse mode: listen on 127.0.0.1:0 and dial this tailnet target (host:port) per connection (ignores --forward/--listen)")
	upTimeout := flag.Duration("up-timeout", 60*time.Second, "how long to wait for the tailnet join")
	debug := flag.Bool("debug", false, "log tsnet and connection events to stderr")
	flag.Parse()

	listen := *listenPort
	if listen == 0 {
		listen = *forwardPort
	}

	// Stay quiet by default: stdout is reserved for the MESH_READY/PROXY_READY
	// contract lines and stderr for MESH_ERROR; anything else only with --debug.
	logf := func(string, ...any) {}
	if *debug {
		logf = func(format string, args ...any) {
			fmt.Fprintf(os.Stderr, "mesh-node: "+format+"\n", args...)
		}
	}

	if err := run(*hostname, *controlURL, *authKey, *dataDir, *forwardPort, listen, *reverseTarget, *upTimeout, logf); err != nil {
		fmt.Fprintf(os.Stderr, "MESH_ERROR reason=%s\n", err)
		os.Exit(1)
	}
}

func run(hostname, controlURL, authKey, dataDir string, forwardPort, listenPort int, reverseTarget string, upTimeout time.Duration, logf func(string, ...any)) error {
	if reverseTarget != "" {
		if _, _, err := net.SplitHostPort(reverseTarget); err != nil {
			return fmt.Errorf("invalid --reverse target %q: %w", reverseTarget, err)
		}
	}
	if err := os.MkdirAll(dataDir, 0o700); err != nil {
		return fmt.Errorf("creating data dir: %w", err)
	}
	srv := &tsnet.Server{
		Dir:        dataDir,
		Hostname:   hostname,
		ControlURL: controlURL,
		AuthKey:    authKey,
		Ephemeral:  false,
		Logf:       logf,
	}
	ctx, cancel := context.WithTimeout(context.Background(), upTimeout)
	defer cancel()
	st, err := srv.Up(ctx)
	if err != nil {
		return fmt.Errorf("joining tailnet (check the auth key and control URL): %w", err)
	}
	var ip string
	if st.Self != nil {
		for _, addr := range st.Self.TailscaleIPs {
			if addr.Is4() {
				ip = addr.String()
				break
			}
		}
	}
	if ip == "" {
		return errors.New("tailnet joined but no tailnet IPv4 address was assigned")
	}

	done := make(chan struct{})
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGTERM, syscall.SIGINT)
	go func() {
		<-sigCh
		close(done)
	}()

	if reverseTarget != "" {
		return serveReverse(srv, ip, reverseTarget, done, logf)
	}
	return serveForward(srv, ip, forwardPort, listenPort, done, logf)
}

// serveForward is the default mode: listen on the tailnet and proxy each
// accepted connection to the local gizzi port.
func serveForward(srv *tsnet.Server, ip string, forwardPort, listenPort int, done <-chan struct{}, logf func(string, ...any)) error {
	ln, err := srv.Listen("tcp", fmt.Sprintf(":%d", listenPort))
	if err != nil {
		return fmt.Errorf("listening on tailnet port %d: %w", listenPort, err)
	}

	// The parent scrapes exactly this line; keep stdout to the contract lines.
	fmt.Printf("MESH_READY ip=%s\n", ip)

	go func() {
		<-done
		ln.Close()
	}()

	for {
		conn, err := ln.Accept()
		if err != nil {
			select {
			case <-done:
				// SIGTERM path: close listener, close server, exit 0.
				return srv.Close()
			default:
				return fmt.Errorf("accepting on tailnet listener: %w", err)
			}
		}
		logf("accepted tailnet connection from %s", conn.RemoteAddr())
		go forward(conn, forwardPort, logf)
	}
}

// serveReverse is the tailnet-client mode: listen on loopback with an
// ephemeral port and dial the fixed tailnet target through the tsnet server
// per connection. Local apps reach the instance via 127.0.0.1:<port>; TCP
// forwarding carries HTTP and WebSocket traffic unchanged.
func serveReverse(srv *tsnet.Server, ip, target string, done <-chan struct{}, logf func(string, ...any)) error {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return fmt.Errorf("listening on loopback: %w", err)
	}

	// Contract lines the parent scrapes; keep them the only stdout output.
	fmt.Printf("MESH_READY ip=%s\n", ip)
	fmt.Printf("PROXY_READY port=%d\n", ln.Addr().(*net.TCPAddr).Port)

	dial := func() (net.Conn, error) {
		ctx, cancel := context.WithTimeout(context.Background(), 15*time.Second)
		defer cancel()
		return srv.Dial(ctx, "tcp", target)
	}
	if host, _, err := net.SplitHostPort(target); err == nil {
		if addr, err := netip.ParseAddr(host); err == nil {
			if lc, err := srv.LocalClient(); err == nil {
				ping := func(ctx context.Context) error {
					_, err := lc.Ping(ctx, addr, tailcfg.PingTSMP)
					return err
				}
				go keepWarm(ping, keepWarmInterval, done, logf)
			} else {
				logf("keepalive disabled: %v", err)
			}
		}
	}
	if err := serveLoopback(ln, dial, done, logf); err != nil {
		return err
	}
	return srv.Close()
}

// keepWarm pings the reverse target every interval until done is closed, so
// the tailnet path stays up between connections. Failures are logged and
// retried on the next tick; they never stop the proxy.
func keepWarm(ping func(context.Context) error, interval time.Duration, done <-chan struct{}, logf func(string, ...any)) {
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	for {
		ctx, cancel := context.WithTimeout(context.Background(), interval)
		if err := ping(ctx); err != nil {
			logf("keepalive ping: %v", err)
		}
		cancel()
		select {
		case <-done:
			return
		case <-ticker.C:
		}
	}
}

// serveLoopback accepts on ln and bridges each connection to the target
// obtained from dial, until done is closed (which closes ln). It returns nil
// on the shutdown path so the caller can close the tsnet server cleanly.
func serveLoopback(ln net.Listener, dial func() (net.Conn, error), done <-chan struct{}, logf func(string, ...any)) error {
	go func() {
		<-done
		ln.Close()
	}()
	for {
		conn, err := ln.Accept()
		if err != nil {
			select {
			case <-done:
				return nil
			default:
				return fmt.Errorf("accepting on loopback listener: %w", err)
			}
		}
		logf("accepted loopback connection from %s", conn.RemoteAddr())
		go func() {
			upstream, err := dial()
			if err != nil {
				logf("dialing reverse target: %v", err)
				conn.Close()
				return
			}
			proxyConn(conn, upstream)
		}()
	}
}

// forward bridges one tailnet-side connection to the local gizzi port.
func forward(tailnetConn net.Conn, forwardPort int, logf func(string, ...any)) {
	local, err := net.DialTimeout("tcp", fmt.Sprintf("127.0.0.1:%d", forwardPort), 10*time.Second)
	if err != nil {
		logf("dialing 127.0.0.1:%d: %v", forwardPort, err)
		tailnetConn.Close()
		return
	}
	proxyConn(local, tailnetConn)
}
