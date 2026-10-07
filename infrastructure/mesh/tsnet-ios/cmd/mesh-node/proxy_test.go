package main

import (
	"bufio"
	"context"
	"errors"
	"io"
	"net"
	"strings"
	"sync"
	"testing"
	"time"
)

// TestProxyConn bridges a client to a plain TCP echo server through proxyConn
// and verifies bidirectional flow plus half-close propagation: after the
// client CloseWrites, the echo server sees EOF and closes, and the client
// still reads the echoed reply followed by EOF.
func TestProxyConn(t *testing.T) {
	// Echo server: copies back until EOF, then closes.
	echoLn, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer echoLn.Close()
	go func() {
		conn, err := echoLn.Accept()
		if err != nil {
			return
		}
		io.Copy(conn, conn)
		conn.Close()
	}()

	// Tailnet-side stand-in: a plain listener whose accepted conn is proxied
	// to the echo server, exactly like forward() does.
	proxyLn, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer proxyLn.Close()
	go func() {
		conn, err := proxyLn.Accept()
		if err != nil {
			return
		}
		upstream, err := net.Dial("tcp", echoLn.Addr().String())
		if err != nil {
			conn.Close()
			return
		}
		proxyConn(upstream, conn)
	}()

	client, err := net.Dial("tcp", proxyLn.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	client.SetDeadline(time.Now().Add(5 * time.Second))

	if _, err := client.Write([]byte("ping\n")); err != nil {
		t.Fatal(err)
	}
	line, err := bufio.NewReader(client).ReadString('\n')
	if err != nil {
		t.Fatalf("reading echo: %v", err)
	}
	if line != "ping\n" {
		t.Fatalf("echo mismatch: got %q", line)
	}

	// Half-close: the echo server must see EOF and close, which propagates
	// back as EOF on the client without the proxy hanging.
	if err := client.(*net.TCPConn).CloseWrite(); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Read(make([]byte, 1)); err != io.EOF {
		t.Fatalf("expected EOF after half-close, got %v", err)
	}
}

// TestServeLoopback exercises the reverse-mode accept loop: connections on a
// loopback listener are bridged to the target obtained from the dial func
// (a stand-in for srv.Dial into the tailnet), and closing `done` shuts the
// loop down cleanly.
func TestServeLoopback(t *testing.T) {
	// Echo server as the reverse target.
	echoLn, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer echoLn.Close()
	go func() {
		for {
			conn, err := echoLn.Accept()
			if err != nil {
				return
			}
			go func() {
				io.Copy(conn, conn)
				conn.Close()
			}()
		}
	}()

	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	serveDone := make(chan error, 1)
	go func() {
		serveDone <- serveLoopback(ln, func() (net.Conn, error) {
			return net.Dial("tcp", echoLn.Addr().String())
		}, done, func(string, ...any) {})
	}()

	client, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	client.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := client.Write([]byte("ping\n")); err != nil {
		t.Fatal(err)
	}
	line, err := bufio.NewReader(client).ReadString('\n')
	if err != nil {
		t.Fatalf("reading echo through reverse proxy: %v", err)
	}
	if line != "ping\n" {
		t.Fatalf("echo mismatch: got %q", line)
	}
	client.Close()

	// Shutdown: closing done must end the serve loop without error.
	close(done)
	select {
	case err := <-serveDone:
		if err != nil {
			t.Fatalf("serveLoopback returned error on shutdown: %v", err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("serveLoopback did not stop after done was closed")
	}
}

// TestServeLoopbackDialFailure: a failed target dial must close the loopback
// connection (EOF for the client) instead of hanging or crashing the loop.
func TestServeLoopbackDialFailure(t *testing.T) {
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	done := make(chan struct{})
	defer close(done)
	go func() {
		serveLoopback(ln, func() (net.Conn, error) {
			return nil, io.ErrClosedPipe
		}, done, func(string, ...any) {})
	}()

	client, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	client.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := client.Read(make([]byte, 1)); err != io.EOF {
		t.Fatalf("expected EOF after failed target dial, got %v", err)
	}
}

// TestRunRejectsInvalidReverseTarget: an unparseable --reverse target fails
// fast with a clear error before any tailnet join is attempted.
func TestRunRejectsInvalidReverseTarget(t *testing.T) {
	err := run("test-node", defaultControlURL, "", t.TempDir(), 0, 0, nil, "not-a-host-port", time.Second, func(string, ...any) {})
	if err == nil {
		t.Fatal("expected run to reject an invalid --reverse target")
	}
	if got := err.Error(); !strings.Contains(got, "--reverse") {
		t.Fatalf("error should mention --reverse, got %q", got)
	}
}

func TestKeepWarmPingsUntilDone(t *testing.T) {
	var mu sync.Mutex
	pings := 0
	ping := func(context.Context) error {
		mu.Lock()
		defer mu.Unlock()
		pings++
		if pings == 2 {
			return errors.New("transient")
		}
		return nil
	}
	done := make(chan struct{})
	finished := make(chan struct{})
	go func() {
		keepWarm(ping, 10*time.Millisecond, done, func(string, ...any) {})
		close(finished)
	}()
	time.Sleep(55 * time.Millisecond)
	close(done)
	select {
	case <-finished:
	case <-time.After(time.Second):
		t.Fatal("keepWarm did not stop after done")
	}
	mu.Lock()
	defer mu.Unlock()
	if pings < 3 {
		t.Fatalf("expected repeated pings despite a failure, got %d", pings)
	}
}

// TestAlsoFlagTakesPortNumbers: --also is repeatable and only takes ports.
func TestAlsoFlagTakesPortNumbers(t *testing.T) {
	var p portList
	if err := p.Set("3019"); err != nil {
		t.Fatal(err)
	}
	if err := p.Set("5901"); err != nil {
		t.Fatal(err)
	}
	if len(p) != 2 || p[0] != 3019 || p[1] != 5901 {
		t.Fatalf("got %v", p)
	}
	for _, bad := range []string{"0", "70000", "vnc"} {
		if err := p.Set(bad); err == nil {
			t.Fatalf("--also %s should be refused", bad)
		}
	}
}
