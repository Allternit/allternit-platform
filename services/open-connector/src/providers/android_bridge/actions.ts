import type { ActionDefinition } from "../../core/types.ts";
import { s } from "../../core/json-schema.ts";

const phoneNumberField = s.string("Phone number in E.164 or local format.", { minLength: 1 });
const messageBodyField = s.string("SMS message body.", { minLength: 1 });
const limitField = s.integer("Maximum number of messages to return.", { minimum: 1, maximum: 100 });

export const androidBridgeActions: readonly ActionDefinition[] = [
  {
    id: "android_bridge.send_sms",
    service: "android_bridge",
    name: "send_sms",
    description: "Send an SMS message from the bridged Android device.",
    requiredScopes: [],
    providerPermissions: [],
    inputSchema: s.object(
      "SMS send request.",
      {
        to: phoneNumberField,
        body: messageBodyField,
      },
    ),
    outputSchema: s.object(
      "SMS send result.",
      {
        sent: s.boolean("Whether the message was dispatched to the device."),
        to: phoneNumberField,
      },
    ),
  },
  {
    id: "android_bridge.list_messages",
    service: "android_bridge",
    name: "list_messages",
    description: "List recent SMS messages from the device inbox.",
    requiredScopes: [],
    providerPermissions: [],
    inputSchema: s.object(
      "SMS list parameters.",
      {
        limit: limitField,
      },
      { optional: ["limit"] },
    ),
    outputSchema: s.object(
      "SMS list result.",
      {
        messages: s.array(
          "SMS messages.",
          s.object("An SMS message.", {
            address: s.string("Sender/recipient phone number."),
            date: s.string("Message timestamp."),
            body: s.string("Message body."),
          }),
        ),
      },
    ),
  },
  {
    id: "android_bridge.screenshot",
    service: "android_bridge",
    name: "screenshot",
    description: "Capture a screenshot from the Android device.",
    requiredScopes: [],
    providerPermissions: [],
    inputSchema: s.object("No input is required.", {}),
    outputSchema: s.object(
      "Screenshot result.",
      {
        image: s.string("Base64-encoded PNG image."),
        format: s.string("Image format."),
      },
    ),
  },
  {
    id: "android_bridge.tap",
    service: "android_bridge",
    name: "tap",
    description: "Tap text on the screen or tap absolute coordinates.",
    requiredScopes: [],
    providerPermissions: [],
    inputSchema: s.object(
      "Tap request.",
      {
        text: s.string("Text to tap (uses OCR)."),
        x: s.integer("X coordinate."),
        y: s.integer("Y coordinate."),
      },
      { optional: ["text", "x", "y"] },
    ),
    outputSchema: s.object(
      "Tap result.",
      {
        tapped: s.boolean("Whether the tap was performed."),
        text: s.string("Tapped text, if matched."),
        x: s.integer("X coordinate."),
        y: s.integer("Y coordinate."),
      },
      { optional: ["text", "x", "y"] },
    ),
  },
  {
    id: "android_bridge.type",
    service: "android_bridge",
    name: "type",
    description: "Type text into the currently focused input field.",
    requiredScopes: [],
    providerPermissions: [],
    inputSchema: s.object(
      "Type request.",
      {
        text: s.string("Text to type.", { minLength: 1 }),
      },
    ),
    outputSchema: s.object(
      "Type result.",
      {
        typed: s.boolean("Whether the text was typed."),
        length: s.integer("Number of characters typed."),
      },
    ),
  },
  {
    id: "android_bridge.press_key",
    service: "android_bridge",
    name: "press_key",
    description: "Press a hardware-style key on the device.",
    requiredScopes: [],
    providerPermissions: [],
    inputSchema: s.object(
      "Press key request.",
      {
        key: s.stringEnum("Key to press.", ["home", "back", "recent", "power", "menu"]),
      },
    ),
    outputSchema: s.object(
      "Press key result.",
      {
        pressed: s.string("Key that was pressed."),
      },
    ),
  },
];
