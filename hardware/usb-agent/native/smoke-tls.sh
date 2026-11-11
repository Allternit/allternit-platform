#!/usr/bin/env bash
# Native board: TLS client smoke test (plan step 3).
#
# Makes a throwaway CA and server certificates in a temp dir (no key is kept),
# starts a local TLS server, and runs a tiny test main that links the real
# shim and runtime (NetworkClientSecure, sockets, world) and drives the board
# the way relay_link.h does: useBuiltinCACertBundle, setHandshakeTimeout,
# setTimeout, connect("relay.test", 443, timeout), then an HTTP/1.1 request.
# LINK_NATIVE_HOSTS points relay.test at the local server; LINK_NATIVE_CA_FILE
# is the only trust. Prints, in order:
#   tls ok: 200
#   tls refused: untrusted CA
#   tls refused: wrong host
# and exits non-zero if any case behaves differently.
#
#   bash hardware/usb-agent/native/smoke-tls.sh
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
CXX=${CXX:-c++}
OPENSSL_PC_PATH=/opt/homebrew/opt/openssl@3/lib/pkgconfig:/usr/local/opt/openssl@3/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}
OSSL_CFLAGS=$(PKG_CONFIG_PATH=$OPENSSL_PC_PATH pkg-config --cflags openssl 2>/dev/null || true)
OSSL_LIBS=$(PKG_CONFIG_PATH=$OPENSSL_PC_PATH pkg-config --libs openssl 2>/dev/null || echo "-lssl -lcrypto")
OSSL_BIN=$(PKG_CONFIG_PATH=$OPENSSL_PC_PATH pkg-config --variable=exec_prefix openssl 2>/dev/null || true)
OPENSSL=openssl
[ -n "$OSSL_BIN" ] && [ -x "$OSSL_BIN/bin/openssl" ] && OPENSSL="$OSSL_BIN/bin/openssl"

work=$(mktemp -d "${TMPDIR:-/tmp}/link-native-tls.XXXXXX")
srv=""
stop_server() {
  if [ -n "$srv" ] && kill -0 "$srv" 2>/dev/null; then
    kill "$srv" 2>/dev/null || true
    wait "$srv" 2>/dev/null || true
  fi
  srv=""
}
cleanup() {
  stop_server
  exec 9>&- 2>/dev/null || true
  rm -rf "$work"
}
trap cleanup EXIT
fail() {
  echo "smoke-tls FAILED: $*" >&2
  exit 1
}

# ── certificates ────────────────────────────────────────────────────────────
mkca() {  # name
  "$OPENSSL" req -x509 -newkey rsa:2048 -nodes -keyout "$work/$1.key" -out "$work/$1.pem" \
    -days 2 -subj "/CN=Throwaway $1" 2>/dev/null
}
mkcert() {  # name signing-ca dns-name
  "$OPENSSL" req -newkey rsa:2048 -nodes -keyout "$work/$1.key" -out "$work/$1.csr" -subj "/CN=$3" 2>/dev/null
  printf 'subjectAltName=DNS:%s\nextendedKeyUsage=serverAuth\n' "$3" >"$work/$1.ext"
  "$OPENSSL" x509 -req -in "$work/$1.csr" -CA "$work/$2.pem" -CAkey "$work/$2.key" -CAcreateserial \
    -out "$work/$1.pem" -days 2 -extfile "$work/$1.ext" 2>/dev/null
}
mkca ca-good
mkca ca-other
mkcert relay ca-good relay.test
mkcert wrong ca-good other.test

# ── the server: answers any request with a 200 and closes ───────────────────
cat >"$work/server.py" <<'PY'
import socket, ssl, sys, threading
cert, key, portfile = sys.argv[1:4]
ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.load_cert_chain(cert, key)
ls = socket.socket()
ls.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
ls.bind(("127.0.0.1", 0))
ls.listen(8)
open(portfile, "w").write(str(ls.getsockname()[1]))
def serve(c):
    try:
        c.settimeout(5)
        t = ctx.wrap_socket(c, server_side=True)
        buf = b""
        while b"\r\n\r\n" not in buf:
            d = t.recv(4096)
            if not d:
                break
            buf += d
        body = b"hello"
        t.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n" + body)
        t.unwrap()
    except Exception:
        pass
    finally:
        c.close()
while True:
    c, _ = ls.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
PY

start_server() {  # name
  rm -f "$work/port"
  python3 -I "$work/server.py" "$work/$1.pem" "$work/$1.key" "$work/port" &
  srv=$!
  for _ in $(seq 1 50); do
    [ -s "$work/port" ] && return 0
    sleep 0.1
  done
  fail "the test server did not start"
}

# ── the test main: the firmware's relay_link.h TLS calls, then GET / ────────
cat >"$work/tls_main.cpp" <<'CPP'
#include <Arduino.h>
#include <NetworkClientSecure.h>
#include <WiFi.h>
#include <stdio.h>
#include <stdlib.h>

void setup() {
  WiFi.begin("lab");
  for (int i = 0; i < 400 && WiFi.status() != WL_CONNECTED; i++) delay(20);
  if (WiFi.status() != WL_CONNECTED) {
    printf("TLSTEST no-wifi\n");
    fflush(stdout);
    exit(2);
  }
  NetworkClientSecure tls;
  tls.stop();
  tls.useBuiltinCACertBundle();
  tls.setHandshakeTimeout(8);
  tls.setTimeout(8000);
  int r = tls.connect("relay.test", 443, (int32_t)8000);
  if (r != 1) {
    printf("TLSTEST connect-failed %d\n", r);
    fflush(stdout);
    exit(0);
  }
  tls.print(String("GET / HTTP/1.1\r\nHost: relay.test\r\nConnection: close\r\n\r\n"));
  String line;
  uint32_t deadline = millis() + 8000;
  while ((int32_t)(deadline - millis()) > 0) {
    if (tls.available() <= 0) {
      if (!tls.connected()) break;
      delay(2);
      continue;
    }
    char c = (char)tls.read();
    if (c == '\n') break;
    line += c;
  }
  // read the rest until the server closes: connected() must turn false
  String rest;
  while ((int32_t)(deadline - millis()) > 0) {
    if (tls.available() > 0) { rest += (char)tls.read(); continue; }
    if (!tls.connected()) break;
    delay(2);
  }
  int status = line.startsWith("HTTP/1.1 ") ? line.substring(9, 12).toInt() : -1;
  bool closed = !tls.connected();
  printf("TLSTEST status %d closed %d body-tail %s\n", status, closed ? 1 : 0, rest.endsWith("hello") ? "hello" : "?");
  fflush(stdout);
  exit(0);
}
void loop() {}
CPP

# ── build the runtime objects and the test main ─────────────────────────────
RT=$(cd "$HERE" && ls runtime/*.cpp | sed 's|runtime/\(.*\)\.cpp|\1|')
targets=""
for n in $RT; do targets="$targets $work/b/obj/runtime/$n.o"; done
make -s -C "$HERE" OUT="$work/b" $targets >"$work/make.log" 2>&1 || { cat "$work/make.log" >&2; fail "runtime build"; }
"$CXX" -std=gnu++2b -funsigned-char -pthread -Wall -Wno-unused-parameter -Wno-unused-variable \
  -I"$HERE/shim" $OSSL_CFLAGS -O1 -g -c "$work/tls_main.cpp" -o "$work/tls_main.o" || fail "test main build"
"$CXX" "$work/tls_main.o" $(for n in $RT; do echo "$work/b/obj/runtime/$n.o"; done) $OSSL_LIBS -pthread \
  -o "$work/tls_board" || fail "test main link"

# The board quits when its control stdin closes: hold a fifo open read-write.
mkfifo "$work/ctl"
exec 9<>"$work/ctl"

ssid_hex=$(printf 'lab' | od -An -tx1 | tr -d ' \n')

# run_case <server cert name> <ca file>  -> prints the test main's TLSTEST line
run_case() {
  start_server "$1"
  local port out
  port=$(cat "$work/port")
  out=$(LINK_NATIVE_HOSTS="relay.test=127.0.0.1:$port" LINK_NATIVE_CA_FILE="$work/$2.pem" \
    LINK_NATIVE_STATE="$work/state" LINK_NATIVE_WORLD="wifi air $ssid_hex -" \
    timeout 30 "$work/tls_board" 2>"$work/err" <&9 | grep '^TLSTEST' || true)
  stop_server
  echo "$out"
}

# ── the three cases ─────────────────────────────────────────────────────────
r1=$(run_case relay ca-good)
[ "$r1" = "TLSTEST status 200 closed 1 body-tail hello" ] || fail "good case gave: $r1 / $(cat "$work/err" 2>/dev/null)"
echo "tls ok: 200"

r2=$(run_case relay ca-other)
case "$r2" in "TLSTEST connect-failed 0") ;; *) fail "untrusted CA gave: $r2" ;; esac
echo "tls refused: untrusted CA"

r3=$(run_case wrong ca-good)
case "$r3" in "TLSTEST connect-failed 0") ;; *) fail "wrong host gave: $r3" ;; esac
echo "tls refused: wrong host"

# no CA file at all: every connect fails
start_server relay
port=$(cat "$work/port")
r4=$(env -u LINK_NATIVE_CA_FILE LINK_NATIVE_HOSTS="relay.test=127.0.0.1:$port" \
  LINK_NATIVE_STATE="$work/state" LINK_NATIVE_WORLD="wifi air $ssid_hex -" \
  timeout 30 "$work/tls_board" 2>"$work/err" <&9 | grep '^TLSTEST' || true)
stop_server
case "$r4" in "TLSTEST connect-failed 0") ;; *) fail "no CA file gave: $r4" ;; esac
