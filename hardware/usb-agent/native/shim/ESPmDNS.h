// Native board: ESPmDNS.h (decision 11: mDNS is a table).
//
// The table comes from `mdns set <ip> <port> <deskId>` (one entry per desk id)
// and `mdns clear`. queryService() blocks like the core's
// mdns_query_ptr(…, 3000 ms, 20 results): 3000 ms of real time, or the
// `mdns stall <ms>` value once. It does not feed the watchdog, as on the board.
// The answer is empty without Wi-Fi or before begin().
#pragma once

#include <Arduino.h>

class MDNSResponder {
 public:
  bool begin(const String &hostName);
  bool begin(const char *hostName) {
    return begin(String(hostName));
  };
  int queryService(char *service, char *proto);
  int queryService(const char *service, const char *proto) {
    return queryService((char *)service, (char *)proto);
  }
  IPAddress address(int idx);
  uint16_t port(int idx);
  String txt(int idx, const char *key);
};
extern MDNSResponder MDNS;
