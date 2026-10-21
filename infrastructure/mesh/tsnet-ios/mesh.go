// Package mesh wraps tailscale.com/tsnet behind a minimal, gomobile-friendly
// API so the Allternit iOS app can embed a userspace Tailscale node.
//
// gomobile constraints honored here: exported identifiers only, supported
// types only (string, bool, error), no channels, no generics.
package mesh

import (
	"context"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strconv"
	"sync"
	"time"

	"tailscale.com/tsnet"
)

// maxGetBody caps the response size returned by Get so a large tailnet
// response cannot blow up the app's memory.
const maxGetBody = 4 << 20 // 4 MiB

// Node is a single embedded tailnet node. Create it with NewNode, then call
// Start. A Node is not reusable after Close.
type Node struct {
	mu       sync.Mutex
	hostname string
	srv      *tsnet.Server
	proxy    *nodeProxy
}

// nodeProxy is the loopback TCP proxy started by StartProxy: it accepts on
// 127.0.0.1 and dials its fixed tailnet target through the node's tsnet
// server.
type nodeProxy struct {
	target string
	ln     net.Listener
}

// NewNode creates a Node that will join the tailnet under the given hostname.
// The node is not started yet; call Start.
func NewNode(hostname string) *Node {
	return &Node{hostname: hostname}
}

// Start brings the node up on the tailnet and blocks until it is online or
// the attempt fails.
//
// controlURL is the coordination server (leave empty for the hosted Tailscale
// control plane, or pass a Headscale URL). authKey is a pre-auth key (or a
// Headscale pre-auth key). dataDir must be a writable, persistent directory
// inside the app's sandbox (e.g. Application Support/mesh); tsnet stores its
// node key and state there.
func (n *Node) Start(controlURL, authKey, dataDir string) error {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.srv != nil {
		return errors.New("mesh: node already started")
	}
	if dataDir == "" {
		return errors.New("mesh: dataDir is required")
	}
	srv := &tsnet.Server{
		Dir:        dataDir,
		Hostname:   n.hostname,
		ControlURL: controlURL,
		AuthKey:    authKey,
		Ephemeral:  false,
		Logf:       func(string, ...any) {}, // stay quiet on iOS
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	if _, err := srv.Up(ctx); err != nil {
		return err
	}
	n.srv = srv
	return nil
}

// MeshIP returns the node's tailnet IPv4 address (100.64.0.0/10 range), or
// an empty string if the node is not up.
func (n *Node) MeshIP() string {
	n.mu.Lock()
	srv := n.srv
	n.mu.Unlock()
	if srv == nil {
		return ""
	}
	lc, err := srv.LocalClient()
	if err != nil {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := lc.Status(ctx)
	if err != nil || st.Self == nil {
		return ""
	}
	for _, ip := range st.Self.TailscaleIPs {
		if ip.Is4() {
			return ip.String()
		}
	}
	return ""
}

// Get fetches an HTTP URL through the tailnet using this node's identity and
// returns the response body (capped at 4 MiB). Use it to reach tailnet-only
// services (e.g. http://other-node:8080/...).
func (n *Node) Get(url string) (string, error) {
	n.mu.Lock()
	srv := n.srv
	n.mu.Unlock()
	if srv == nil {
		return "", errors.New("mesh: node not started")
	}
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return "", err
	}
	resp, err := srv.HTTPClient().Do(req)
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	body, err := io.ReadAll(io.LimitReader(resp.Body, maxGetBody))
	if err != nil {
		return "", err
	}
	return string(body), nil
}

// StartProxy starts a loopback TCP proxy that forwards connections from
// 127.0.0.1:<localPort> through the tailnet to targetHost:targetPort, and
// returns the chosen local port.
//
// iOS networking cannot reach tailnet (100.64.0.0/10) addresses directly —
// the mesh is userspace and in-process — so URLSession must go through this
// proxy to reach mesh-registered services (e.g. an instance URL like
// http://100.64.0.2:4096 becomes http://127.0.0.1:<localPort>). TCP-level
// forwarding carries HTTP and WebSocket traffic unchanged.
//
// One proxy per node: calling again with the same target returns the
// running proxy's port; calling with a different target replaces the
// running proxy. The proxy stops on StopProxy or Close.
func (n *Node) StartProxy(targetHost string, targetPort int) (int, error) {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.srv == nil {
		return 0, errors.New("mesh: node not started")
	}
	if targetHost == "" || targetPort <= 0 || targetPort > 65535 {
		return 0, fmt.Errorf("mesh: invalid proxy target %q port %d", targetHost, targetPort)
	}
	target := net.JoinHostPort(targetHost, strconv.Itoa(targetPort))
	if n.proxy != nil && n.proxy.target == target {
		return n.proxy.ln.Addr().(*net.TCPAddr).Port, nil
	}
	n.stopProxyLocked()

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return 0, err
	}
	p := &nodeProxy{target: target, ln: ln}
	n.proxy = p
	go n.serveProxy(p)
	return ln.Addr().(*net.TCPAddr).Port, nil
}

// serveProxy accepts loopback connections until the proxy's listener is
// closed (StopProxy/Close) and bridges each to the tailnet target.
func (n *Node) serveProxy(p *nodeProxy) {
	for {
		conn, err := p.ln.Accept()
		if err != nil {
			return // listener closed
		}
		go n.bridgeToTailnet(p, conn)
	}
}

// bridgeToTailnet dials the proxy's target through the tailnet and shuttles
// bytes between the loopback connection and the tailnet connection.
func (n *Node) bridgeToTailnet(p *nodeProxy, local net.Conn) {
	n.mu.Lock()
	srv := n.srv
	n.mu.Unlock()
	if srv == nil {
		local.Close()
		return
	}
	// 45s, not 15s: a freshly started node can need longer than 15s to reach
	// a peer the first time (mesh-node's reverse mode hit this 2026-09-30).
	ctx, cancel := context.WithTimeout(context.Background(), 45*time.Second)
	upstream, err := srv.Dial(ctx, "tcp", p.target)
	cancel()
	if err != nil {
		local.Close()
		return
	}
	proxyConns(local, upstream)
}

// StopProxy stops the loopback proxy started by StartProxy, if any.
func (n *Node) StopProxy() error {
	n.mu.Lock()
	defer n.mu.Unlock()
	n.stopProxyLocked()
	return nil
}

func (n *Node) stopProxyLocked() {
	if n.proxy != nil {
		n.proxy.ln.Close()
		n.proxy = nil
	}
}

// proxyConns shuttles bytes between a and b in both directions until both
// copies finish, then closes both. A half-close is propagated to the other
// side when the connection supports it (TCP), so a peer that signals EOF
// still receives the remaining response from the other direction. Mirrors
// proxyConn in cmd/mesh-node/proxy.go (kept as a copy: that one lives in
// package main and cannot be imported here).
func proxyConns(a, b net.Conn) {
	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		io.Copy(a, b)
		closeWrite(a)
	}()
	go func() {
		defer wg.Done()
		io.Copy(b, a)
		closeWrite(b)
	}()
	wg.Wait()
	a.Close()
	b.Close()
}

// closeWrite propagates EOF without closing the read side; a no-op for
// connection types without half-close support.
func closeWrite(c net.Conn) {
	if tc, ok := c.(*net.TCPConn); ok {
		_ = tc.CloseWrite()
	}
}

// Close shuts the node down and releases its resources. The Node must not be
// used afterwards; create a fresh one with NewNode to reconnect.
func (n *Node) Close() error {
	n.mu.Lock()
	defer n.mu.Unlock()
	n.stopProxyLocked()
	if n.srv == nil {
		return nil
	}
	err := n.srv.Close()
	n.srv = nil
	return err
}
