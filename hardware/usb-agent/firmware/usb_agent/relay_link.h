// Allternit Link: the board's path through the cloud relay
// (contract hardware/link/README.md section 4.3).
//
//   1. POST https://<relay>/api/v1/link-boards/<boardId>/socket-ticket
//      with header x-allternit-link-board-token and body {}.
//   2. Open wss://<relay><socketPath>.
//   3. Wait for the one text frame {"type":"allternit_socket_ready"} and
//      discard it. Nothing is sent before it.
//   4. Then one text frame = one USBA line, no line ending, text frames only.
//
// The relay sees USBA lines but cannot forge them: commands, replies and
// state lines carry tags under the session key, and typed text goes as
// `typex`. TLS uses the arduino-esp32 core's built-in CA bundle.
//
// The WebSocket client is our own and small: the opening handshake, masked
// text frames up to 512 bytes, ping and pong, close. No WebSocket library.
// It has never been run on a board.
#pragma once

#include <Arduino.h>
#include <NetworkClientSecure.h>
#include <esp_random.h>
#include <esp_task_wdt.h>
#include <mbedtls/base64.h>
#include <mbedtls/sha1.h>

class RelayLink {
 public:
  static const size_t MAX_FRAME = 512;
  // Each step (TCP connect, TLS handshake, response) is bounded so that one
  // step stays well under the 8 s watchdog; the watchdog is fed between steps.
  static const uint32_t IO_TIMEOUT_MS = 3000;

  // Steps 1 and 2. Blocks for the two TLS handshakes (a few seconds, each
  // step bounded by IO_TIMEOUT_MS) and feeds the watchdog in between. The board
  // only does this while it has no Desktop session, so nothing is moving.
  bool open(const String &host, const String &boardId, const String &tokenHex) {
    stop();
    String path;
    if (!requestTicket(host, boardId, tokenHex, &path)) return false;
    esp_task_wdt_reset();
    if (!upgrade(host, path)) {
      stop();
      return false;
    }
    open_ = true;
    lastTx_ = millis();
    return true;
  }

  bool connected() { return open_ && tls_.connected(); }
  // True once the relay's allternit_socket_ready frame has arrived.
  bool ready() const { return ready_; }

  // 1: a text frame (one USBA line) is in `line`. 0: nothing complete yet.
  // -1: the link must be dropped (close, binary frame, oversized or
  // fragmented frame, a line with a control character, or a bad ready frame).
  int poll(String &line) {
    if (!connected()) return -1;
    if (millis() - lastTx_ > 25000) {  // keep NATs and the relay from timing out an idle socket
      if (!sendFrame(0x9, nullptr, 0)) return -1;
    }
    while (tls_.available() > 0) {
      int c = tls_.read();
      if (c < 0) break;
      int r = feed((uint8_t)c, line);
      if (r != 0) return r;
    }
    return 0;
  }

  bool sendText(const String &text) {
    if (!connected() || !ready_ || text.length() > MAX_FRAME) return false;
    return sendFrame(0x1, (const uint8_t *)text.c_str(), text.length());
  }

  void stop() {
    if (open_ && tls_.connected()) {
      uint8_t code[2] = {0x03, 0xE8};  // 1000, normal close
      sendFrame(0x8, code, 2);
    }
    tls_.stop();
    open_ = false;
    ready_ = false;
    resetFrame();
  }

 private:
  NetworkClientSecure tls_;
  bool open_ = false;
  bool ready_ = false;
  uint32_t lastTx_ = 0;
  // Incoming frame parser.
  uint8_t hdr_[14];
  size_t hdrLen_ = 0, hdrNeed_ = 2;
  uint8_t opcode_ = 0;
  bool masked_ = false;
  size_t payloadLen_ = 0, payloadGot_ = 0;
  uint8_t payload_[MAX_FRAME];

  void resetFrame() {
    hdrLen_ = 0;
    hdrNeed_ = 2;
    payloadLen_ = payloadGot_ = 0;
  }

  bool connectTls(const String &host) {
    esp_task_wdt_reset();
    tls_.stop();
    tls_.useBuiltinCACertBundle();
    tls_.setHandshakeTimeout(IO_TIMEOUT_MS / 1000);
    tls_.setTimeout(IO_TIMEOUT_MS);
    return tls_.connect(host.c_str(), 443, (int32_t)IO_TIMEOUT_MS) == 1;
  }

  // Reads one CRLF-terminated line (without the CRLF), at most 1023 bytes.
  bool readLine(String *out, uint32_t deadline) {
    out->clear();
    while ((int32_t)(deadline - millis()) > 0) {
      esp_task_wdt_reset();
      if (tls_.available() <= 0) {
        if (!tls_.connected()) return false;
        delay(2);
        continue;
      }
      char c = (char)tls_.read();
      if (c == '\n') {
        if (out->endsWith("\r")) out->remove(out->length() - 1);
        return true;
      }
      if (out->length() >= 1023) return false;
      *out += c;
    }
    return false;
  }

  // Status line and headers. Returns the status code, or -1.
  int readHead(String *wantHeader, String *headerValue, long *contentLength, uint32_t deadline) {
    String line;
    if (!readLine(&line, deadline) || !line.startsWith("HTTP/1.1 ") || line.length() < 12) return -1;
    int status = line.substring(9, 12).toInt();
    *contentLength = -1;
    while (true) {
      if (!readLine(&line, deadline)) return -1;
      if (line.length() == 0) return status;
      int colon = line.indexOf(':');
      if (colon <= 0) continue;
      String name = line.substring(0, colon);
      name.toLowerCase();
      String value = line.substring(colon + 1);
      value.trim();
      if (name == "content-length") *contentLength = value.toInt();
      if (wantHeader && name == *wantHeader) *headerValue = value;
    }
  }

  bool requestTicket(const String &host, const String &boardId, const String &tokenHex, String *path) {
    if (!connectTls(host)) return false;
    esp_task_wdt_reset();
    tls_.print(String("POST /api/v1/link-boards/") + boardId + "/socket-ticket HTTP/1.1\r\n" +
               "Host: " + host + "\r\n" +
               "x-allternit-link-board-token: " + tokenHex + "\r\n" +
               "Content-Type: application/json\r\n" +
               "Content-Length: 2\r\n" +
               "Connection: close\r\n\r\n{}");
    uint32_t deadline = millis() + IO_TIMEOUT_MS;
    long len;
    int status = readHead(nullptr, nullptr, &len, deadline);
    if (status != 200) {
      tls_.stop();
      return false;
    }
    String body;
    while ((int32_t)(deadline - millis()) > 0 && body.length() < 1024 && (len < 0 || (long)body.length() < len)) {
      esp_task_wdt_reset();
      if (tls_.available() > 0) {
        body += (char)tls_.read();
      } else if (!tls_.connected()) {
        break;
      } else {
        delay(2);
      }
    }
    tls_.stop();
    // {"ticket":"…","expiresInSeconds":30,"runtimeDeviceId":"…","socketPath":"/api/v1/runtime-devices/<id>/socket?ticket=…"}
    const char *key = "\"socketPath\":\"";
    int at = body.indexOf(key);
    if (at < 0) return false;
    at += strlen(key);
    int end = body.indexOf('"', at);
    if (end < 0) return false;
    String p = body.substring(at, end);
    p.replace("\\/", "/");
    if (!p.startsWith("/api/v1/runtime-devices/") || p.length() > 400) return false;
    for (size_t i = 0; i < p.length(); i++) {
      if (p[i] <= ' ' || p[i] > '~' || p[i] == '\\') return false;
    }
    *path = p;
    return true;
  }

  bool upgrade(const String &host, const String &path) {
    if (!connectTls(host)) return false;
    esp_task_wdt_reset();
    uint8_t nonce[16];
    esp_fill_random(nonce, sizeof nonce);
    char key[32];
    size_t keyLen = 0;
    if (mbedtls_base64_encode((unsigned char *)key, sizeof key, &keyLen, nonce, sizeof nonce) != 0) return false;
    key[keyLen] = '\0';
    tls_.print(String("GET ") + path + " HTTP/1.1\r\n" +
               "Host: " + host + "\r\n" +
               "Upgrade: websocket\r\n" +
               "Connection: Upgrade\r\n" +
               "Sec-WebSocket-Key: " + key + "\r\n" +
               "Sec-WebSocket-Version: 13\r\n\r\n");
    String want = "sec-websocket-accept", accept;
    long len;
    if (readHead(&want, &accept, &len, millis() + IO_TIMEOUT_MS) != 101) return false;
    // Accept = base64(SHA-1(key + the RFC 6455 GUID)).
    String joined = String(key) + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    uint8_t digest[20];
    if (mbedtls_sha1((const unsigned char *)joined.c_str(), joined.length(), digest) != 0) return false;
    char expect[32];
    size_t expectLen = 0;
    if (mbedtls_base64_encode((unsigned char *)expect, sizeof expect, &expectLen, digest, sizeof digest) != 0) return false;
    expect[expectLen] = '\0';
    return accept == expect;
  }

  bool sendFrame(uint8_t opcode, const uint8_t *data, size_t len) {
    if (len > MAX_FRAME) return false;
    uint8_t frame[2 + 2 + 4 + MAX_FRAME];
    size_t at = 0;
    frame[at++] = 0x80 | opcode;  // FIN, never fragmented
    if (len < 126) {
      frame[at++] = 0x80 | (uint8_t)len;  // client frames are always masked
    } else {
      frame[at++] = 0x80 | 126;
      frame[at++] = (uint8_t)(len >> 8);
      frame[at++] = (uint8_t)len;
    }
    uint8_t mask[4];
    esp_fill_random(mask, sizeof mask);
    memcpy(frame + at, mask, 4);
    at += 4;
    for (size_t i = 0; i < len; i++) frame[at++] = data[i] ^ mask[i & 3];
    lastTx_ = millis();
    return tls_.write(frame, at) == at;
  }

  // Feeds one byte to the frame parser. Same return values as poll().
  int feed(uint8_t b, String &line) {
    if (hdrLen_ < hdrNeed_) {
      hdr_[hdrLen_++] = b;
      if (hdrLen_ == 2) {
        bool fin = hdr_[0] & 0x80;
        opcode_ = hdr_[0] & 0x0F;
        masked_ = hdr_[1] & 0x80;
        uint8_t len7 = hdr_[1] & 0x7F;
        if (!fin || opcode_ == 0x0 || (hdr_[0] & 0x70)) return -1;  // fragments and extensions: not used by the relay
        if (len7 == 127) return -1;                                  // far over 512 bytes
        hdrNeed_ = 2 + (len7 == 126 ? 2 : 0) + (masked_ ? 4 : 0);
        payloadLen_ = len7;
      }
      if (hdrLen_ < hdrNeed_) return 0;
      if (hdr_[1] & 0x7F) {
        if ((hdr_[1] & 0x7F) == 126) payloadLen_ = ((size_t)hdr_[2] << 8) | hdr_[3];
      }
      if (payloadLen_ > MAX_FRAME) return -1;
      payloadGot_ = 0;
      if (payloadLen_ > 0) return 0;
    } else {
      payload_[payloadGot_++] = b;
      if (payloadGot_ < payloadLen_) return 0;
    }
    // A whole frame is in.
    if (masked_) {
      const uint8_t *mask = hdr_ + hdrNeed_ - 4;
      for (size_t i = 0; i < payloadLen_; i++) payload_[i] ^= mask[i & 3];
    }
    uint8_t op = opcode_;
    size_t len = payloadLen_;
    resetFrame();
    switch (op) {
      case 0x1: {  // text
        line.clear();
        line.reserve(len);
        for (size_t i = 0; i < len; i++) {
          if (payload_[i] < 0x20 || payload_[i] > 0x7e) return -1;
          line += (char)payload_[i];
        }
        if (!ready_) {
          if (line.indexOf("\"allternit_socket_ready\"") < 0) return -1;
          ready_ = true;  // discarded
          return 0;
        }
        return len ? 1 : 0;
      }
      case 0x8:  // close: answer and drop
        return -1;
      case 0x9:  // ping: answer with the same payload
        return sendFrame(0xA, payload_, len) ? 0 : -1;
      case 0xA:  // pong
        return 0;
      default:  // binary or reserved
        return -1;
    }
  }
};
