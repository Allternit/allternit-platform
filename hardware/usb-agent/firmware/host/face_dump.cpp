// Host check for the Allternit Link face renderer.
//
// Lives outside the sketch folder so arduino-cli does not compile it.
// Prints the 5000 bytes renderFace() makes as 10000 lowercase hex characters
// and a newline. The Desktop test (phone-usb-agent-face.test.ts) builds this
// with the system c++ and compares the output with the TypeScript renderer.
//
//   c++ -std=c++17 -o face_dump face_dump.cpp
//   ./face_dump <face> <prog 0-5> <armed 0|1>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../usb_agent/face.h"

int main(int argc, char **argv) {
  if (argc != 4) {
    fprintf(stderr, "usage: face_dump <face> <prog 0-5> <armed 0|1>\n");
    return 2;
  }
  FaceName face;
  if (!faceFromName(argv[1], &face)) {
    fprintf(stderr, "unknown face: %s\n", argv[1]);
    return 2;
  }
  char *end = nullptr;
  long prog = strtol(argv[2], &end, 10);
  if (*end != '\0' || prog < 0 || prog > 5) {
    fprintf(stderr, "prog must be 0 to 5\n");
    return 2;
  }
  if (strcmp(argv[3], "0") != 0 && strcmp(argv[3], "1") != 0) {
    fprintf(stderr, "armed must be 0 or 1\n");
    return 2;
  }
  static uint8_t out[FACE_BYTES];
  renderFace(out, face, (uint8_t)prog, argv[3][0] == '1');
  for (size_t i = 0; i < FACE_BYTES; i++) printf("%02x", out[i]);
  printf("\n");
  return 0;
}
