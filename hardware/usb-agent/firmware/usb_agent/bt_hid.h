// Bluetooth LE keyboard + mouse for the Allternit USB Agent ("Allternit Keyboard").
//
// Bluetooth mode sends the same keyboard/mouse reports as the Android app and the
// Mac tool, from the same HID report map, so Desktop's shared core treats every
// sender alike. Report IDs: 1 keyboard (modifiers, reserved, 6 keys), 2 mouse
// (3 buttons, X, Y, wheel, each -127..127). Over GATT the report ID is not part
// of the value; it rides in each report's Report Reference descriptor.
//
// Uses the BLE library bundled with the arduino-esp32 core (Apache-2.0).
#pragma once
#include <Arduino.h>

// off: not in Bluetooth mode. pairing: no bonded iPhone. bonded: bonded, not connected.
// connected: bonded and encrypted, reports can flow.
enum BtState { BT_OFF, BT_PAIRING, BT_BONDED, BT_CONNECTED };

void btStart(const char *name);          // call once, in Bluetooth mode only
BtState btState();
const char *btStateName();
bool btConnected();
void btOpenPairing(uint32_t ms);         // let a NEW iPhone bond (single host: it replaces the old bond)
void btForget();                         // remove every bond and disconnect
void btService();                        // call every loop: pairing window, advertising restart

void btMouseMove(int dx, int dy, int wheel);   // each clamped to -127..127
void btMouseButtons(uint8_t buttons);          // bit 0 left, bit 1 right
void btKeys(uint8_t modifiers, const uint8_t *keys, size_t n);  // up to 6 usages
void btReleaseAll();
// US-layout ASCII to HID usage; false when the character has no key.
bool btAsciiToHid(char c, uint8_t *modifier, uint8_t *usage);
