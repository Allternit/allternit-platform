// Native board: IPAddress, written for this project. Signatures from
// arduino-esp32 3.3.12 IPAddress.h. fromString() tries the IPv4 dotted form
// with the core's rules (exactly three dots, each part 0 to 255, digits
// only) and then IPv6, as the core does; IPv6 parsing here uses inet_pton,
// the core uses lwIP's ip6addr_aton (they can differ on zone suffixes).
#pragma once

#include <stdint.h>

#include "WString.h"

enum IPType {
  IPv4,
  IPv6
};

class IPAddress {
 public:
  IPAddress();
  IPAddress(uint8_t first_octet, uint8_t second_octet, uint8_t third_octet, uint8_t fourth_octet);
  IPAddress(uint32_t address);
  IPAddress(const char *address);
  IPAddress(const IPAddress &address);

  bool fromString(const char *address);
  bool fromString(const String &address) {
    return fromString(address.c_str());
  }
  String toString(bool includeZone = false) const;

  IPType type() const {
    return _type;
  }
  const uint8_t *v4bytes() const {
    return _bytes + 12;
  }
  const uint8_t *v6bytes() const {
    return _bytes;
  }
  bool operator==(const IPAddress &addr) const;
  IPAddress &operator=(const IPAddress &address);

 private:
  uint8_t _bytes[16] = {0};  // IPv4 in the last four bytes, as in the core
  IPType _type = IPv4;
  bool fromString4(const char *address);
  bool fromString6(const char *address);
};
