package mesh

import (
	"bufio"
	"io"
	"net"
	"strconv"
	"testing"
	"time"
)

// serveTestNode returns a Node whose listen seam binds loopback, so ServeLocal
// can be exercised with no control server and no tailnet.
func serveTestNode(t *testing.T) *Node {
	t.Helper()
	n := NewNode("test")
	n.listen = func(network, addr string) (net.Listener, error) {
		return net.Listen("tcp", "127.0.0.1:0")
	}
	return n
}

// startEchoServer runs a TCP echo server until the test ends and returns its
// address as host:port.
func startEchoServer(t *testing.T) string {
	t.Helper()
	ln, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { ln.Close() })
	go func() {
		for {
			conn, err := ln.Accept()
			if err != nil {
				return
			}
			go func() {
				io.Copy(conn, conn)
				conn.Close()
			}()
		}
	}()
	return ln.Addr().String()
}

// TestServeLocalForwards: a connection accepted on the (seamed) tailnet
// listener lands on 127.0.0.1:localPort and bytes flow both ways, exactly as
// a mesh peer dialing <meshIP>:8200 would reach the phone's local server.
func TestServeLocalForwards(t *testing.T) {
	target := startEchoServer(t)

	n := serveTestNode(t)
	if err := n.ServeLocal(8200, mustPort(t, target)); err != nil {
		t.Fatal(err)
	}

	ln := currentServeListener(t, n)
	client, err := net.Dial("tcp", ln.Addr().String())
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
		t.Fatalf("reading echo through ServeLocal: %v", err)
	}
	if line != "ping\n" {
		t.Fatalf("echo mismatch: got %q", line)
	}
}

// TestServeLocalIdempotent: a second ServeLocal with the same port is a no-op;
// a different port is an error until StopServe.
func TestServeLocalIdempotent(t *testing.T) {
	target := startEchoServer(t)
	n := serveTestNode(t)
	if err := n.ServeLocal(8200, mustPort(t, target)); err != nil {
		t.Fatal(err)
	}
	if err := n.ServeLocal(8200, mustPort(t, target)); err != nil {
		t.Fatalf("same-port ServeLocal should be a no-op, got %v", err)
	}
	if err := n.ServeLocal(8201, mustPort(t, target)); err == nil {
		t.Fatal("different-port ServeLocal should fail while serving")
	}
	if err := n.StopServe(); err != nil {
		t.Fatal(err)
	}
	if err := n.ServeLocal(8201, mustPort(t, target)); err != nil {
		t.Fatalf("ServeLocal after StopServe should work, got %v", err)
	}
	n.StopServe()
}

// TestServeLocalRequiresStart: without a started node (and no test seam) the
// call fails fast instead of panicking or binding.
func TestServeLocalRequiresStart(t *testing.T) {
	n := NewNode("test")
	if err := n.ServeLocal(8200, 8080); err == nil {
		t.Fatal("ServeLocal on an unstarted node should fail")
	}
}

// TestServeLocalRejectsBadPorts: both ports are validated up front.
func TestServeLocalRejectsBadPorts(t *testing.T) {
	n := serveTestNode(t)
	for _, ports := range [][2]int{{0, 8080}, {8200, 0}, {70000, 8080}, {8200, -1}} {
		if err := n.ServeLocal(ports[0], ports[1]); err == nil {
			t.Fatalf("ServeLocal(%d, %d) should be rejected", ports[0], ports[1])
		}
	}
}

// TestServeLocalDialFailureClosesConn: when the loopback target is not
// listening, the accepted connection is closed (EOF) rather than hung.
func TestServeLocalDialFailureClosesConn(t *testing.T) {
	n := serveTestNode(t)
	if err := n.ServeLocal(8200, 1); err != nil { // port 1: nothing listens
		t.Fatal(err)
	}
	ln := currentServeListener(t, n)
	client, err := net.Dial("tcp", ln.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	client.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := client.Read(make([]byte, 1)); err != io.EOF {
		t.Fatalf("expected EOF when target dial fails, got %v", err)
	}
	n.StopServe()
}

// TestCloseStopsServe: Close tears down a serving listener too.
func TestCloseStopsServe(t *testing.T) {
	n := serveTestNode(t)
	if err := n.ServeLocal(8200, 8080); err != nil {
		t.Fatal(err)
	}
	if err := n.Close(); err != nil {
		t.Fatal(err)
	}
	n.mu.Lock()
	serving := n.srvLn != nil
	n.mu.Unlock()
	if serving {
		t.Fatal("listener still live after Close")
	}
}

func mustPort(t *testing.T, hostPort string) int {
	t.Helper()
	_, portStr, err := net.SplitHostPort(hostPort)
	if err != nil {
		t.Fatal(err)
	}
	port, err := strconv.Atoi(portStr)
	if err != nil {
		t.Fatal(err)
	}
	return port
}

func currentServeListener(t *testing.T, n *Node) net.Listener {
	t.Helper()
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.srvLn == nil {
		t.Fatal("node is not serving")
	}
	return n.srvLn
}
