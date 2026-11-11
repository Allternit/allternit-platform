// Allternit Link face renderer (contract hardware/link/README.md section 3).
//
// Plain C++, no Arduino includes, so the host check
// (../host/face_dump.cpp) can compile it with the system compiler. The same
// function is written in TypeScript in
// surfaces/allternit-desktop/src/main/phone-usb-agent-face.ts with the same
// integer rules; a Desktop test compares the two byte for byte.
//
// Canvas: 200 x 200, 1 bit per pixel, row-major, 25 bytes per row, most
// significant bit first, 1 = black, 0 = white. Gizzi: two solid square eyes,
// no nose, no mouth. The coral A:// is printed on the window, so the keep-out
// area (x 60..139, y 118..152) is always white.
#pragma once

#include <stddef.h>
#include <stdint.h>
#include <string.h>

enum FaceName : uint8_t { FACE_DEFAULT, FACE_WINK, FACE_WORKING, FACE_NEEDS_YOU, FACE_STOPPED, FACE_COUNT };
const uint16_t FACE_W = 200, FACE_H = 200;
const size_t FACE_BYTES = 5000;

namespace face_detail {

struct Rect {
  int16_t x, y, w, h;
};

const char *const NAMES[FACE_COUNT] = {"default", "wink", "working", "needs_you", "stopped"};

// Rectangles per face, from contract section 3.2. A zero-width entry ends a list.
const Rect RECTS[FACE_COUNT][4] = {
  {{46, 62, 36, 36}, {118, 62, 36, 36}, {0, 0, 0, 0}, {0, 0, 0, 0}},     // default
  {{46, 62, 36, 36}, {118, 76, 36, 8}, {0, 0, 0, 0}, {0, 0, 0, 0}},      // wink
  {{46, 72, 36, 16}, {118, 72, 36, 16}, {0, 0, 0, 0}, {0, 0, 0, 0}},     // working
  {{46, 62, 36, 36}, {118, 62, 36, 36}, {96, 10, 8, 28}, {96, 42, 8, 8}}, // needs_you
  {{46, 77, 36, 6}, {118, 77, 36, 6}, {0, 0, 0, 0}, {0, 0, 0, 0}},       // stopped
};

const int16_t PROG_X[5] = {26, 58, 90, 122, 154};
const int16_t PROG_Y = 164, PROG_SIZE = 20;
const Rect ARMED_MARK = {184, 8, 8, 8};
const Rect KEEP_OUT = {60, 118, 80, 35};

inline void paint(uint8_t *out, Rect r, bool black) {
  int x0 = r.x < 0 ? 0 : r.x;
  int y0 = r.y < 0 ? 0 : r.y;
  int x1 = r.x + r.w > FACE_W ? FACE_W : r.x + r.w;
  int y1 = r.y + r.h > FACE_H ? FACE_H : r.y + r.h;
  for (int y = y0; y < y1; y++) {
    for (int x = x0; x < x1; x++) {
      size_t i = (size_t)y * (FACE_W / 8) + (size_t)(x >> 3);
      uint8_t mask = (uint8_t)(0x80 >> (x & 7));
      if (black) out[i] |= mask; else out[i] &= (uint8_t)~mask;
    }
  }
}

}  // namespace face_detail

inline bool faceFromName(const char *name, FaceName *out) {
  if (!name) return false;
  for (uint8_t i = 0; i < FACE_COUNT; i++) {
    if (strcmp(name, face_detail::NAMES[i]) == 0) {
      *out = (FaceName)i;
      return true;
    }
  }
  return false;
}

inline const char *faceName(FaceName f) {
  return f < FACE_COUNT ? face_detail::NAMES[f] : face_detail::NAMES[FACE_DEFAULT];
}

inline void renderFace(uint8_t out[FACE_BYTES], FaceName face, uint8_t prog, bool armed) {
  using namespace face_detail;
  memset(out, 0, FACE_BYTES);
  if (face >= FACE_COUNT) face = FACE_DEFAULT;
  for (int i = 0; i < 4 && RECTS[face][i].w > 0; i++) paint(out, RECTS[face][i], true);
  uint8_t p = prog > 5 ? 5 : prog;
  if (p > 0 || face == FACE_WORKING) {
    for (int i = 0; i < 5; i++) {
      int16_t x = PROG_X[i], y = PROG_Y, s = PROG_SIZE;
      if (i + 1 <= p) {
        paint(out, {x, y, s, s}, true);
      } else {
        paint(out, {x, y, s, 2}, true);
        paint(out, {x, (int16_t)(y + s - 2), s, 2}, true);
        paint(out, {x, y, 2, s}, true);
        paint(out, {(int16_t)(x + s - 2), y, 2, s}, true);
      }
    }
  }
  if (armed) paint(out, ARMED_MARK, true);
  paint(out, KEEP_OUT, false);
}
