// Route router registry (spec §2/§4, task P0): the six deterministic route ids
// in their fixed selection order, per-route metadata, and the data-driven
// mapping from adapters/phone devices to routes. No provider-name branching:
// an adapter lands on a route because of its manifest interface/lane, a phone
// because of the shape Desktop's `phone.devices` reports (real response
// shapes, mirrored from surfaces/allternit-desktop/src/main/phone-tools.ts).
import type { AdapterManifest, AgentCapabilityManifest } from "@allternit/subscription-fabric-contracts";

// §4.1 fixed order. Selection never deviates from this sequence except for an
// eligible user override (§4.6): phones first (day-one setup with apps the
// user is already signed into), vision routes last (slowest).
export const ROUTE_IDS = [
  "android-app",
  "iphone-app-ictrl",
  "desktop-app-cdp",
  "desktop-app-ax",
  "web",
  "iphone-app-usb-agent",
] as const;
export type RouteId = (typeof ROUTE_IDS)[number];

export const ROUTE_ORDER: readonly RouteId[] = ROUTE_IDS;

export function isRouteId(v: unknown): v is RouteId {
  return typeof v === "string" && (ROUTE_IDS as readonly string[]).includes(v);
}

export interface RouteInfo {
  id: RouteId;
  label: string;
  device: string;
  hands: string;
  eyes: string;
  /** Vision-reading routes are the slowest; §4.1 keeps them last. */
  vision: boolean;
}

// §2 route table, verbatim from the spec.
export const ROUTES: Record<RouteId, RouteInfo> = {
  "android-app": {
    id: "android-app",
    label: "Android app (adb)",
    device: "Android phone over Wi-Fi",
    hands: "adb input (wireless debugging)",
    eyes: "UI tree (uiautomator dump) + screenshot",
    vision: false,
  },
  "iphone-app-ictrl": {
    id: "iphone-app-ictrl",
    label: "iPhone app (iCTRL)",
    device: "iPhone over Wi-Fi (set up once over the cable)",
    hands: "Our iCTRL fork's XCUITest runner, JSON-RPC on :8200",
    eyes: "UI tree (page_source) + OCR",
    vision: false,
  },
  "desktop-app-cdp": {
    id: "desktop-app-cdp",
    label: "Desktop app (debug port)",
    device: "Connected or cloud computer",
    hands: "Page access through the app's own debug port",
    eyes: "DOM",
    vision: false,
  },
  "desktop-app-ax": {
    id: "desktop-app-ax",
    label: "Desktop app (accessibility)",
    device: "Connected or cloud computer",
    hands: "OS accessibility (AX / UIA / AT-SPI)",
    eyes: "Accessibility tree",
    vision: false,
  },
  web: {
    id: "web",
    label: "Web (Playwright)",
    device: "Any computer's browser",
    hands: "Playwright",
    eyes: "DOM",
    vision: false,
  },
  "iphone-app-usb-agent": {
    id: "iphone-app-usb-agent",
    label: "iPhone app (USB agent)",
    device: "iPhone + ESP32-S3 board",
    hands: "USB mouse/keyboard (AssistiveTouch)",
    eyes: "AirPlay mirror (UxPlay) + vision",
    vision: true,
  },
};

// ---------------------------------------------------------------------------
// Adapter → route mapping (task item 1: "map existing adapters")
// ---------------------------------------------------------------------------

// Declarative (manifest.yaml) adapters: a web bridge drives the vendor's
// site in a browser → the web route.
export function routesForAdapterManifest(manifest: AdapterManifest): RouteId[] {
  return manifest.interface === "ui_bridge_web" ? ["web"] : [];
}

// AAI vendor adapters: only ui_bridge lanes are app-driving routes. A bridge
// that requires the local runtime is a desktop app → it declares BOTH desktop
// lanes (cdp and ax); the launch-time debug-port probe (§2 debug-mode rule,
// stored per app version) decides which one applies. A remote ui_bridge tab
// is the web route. official/local lanes are APIs, not app routes — they are
// not in the §2 table and map to none.
export function routesForAgentManifest(m: AgentCapabilityManifest): RouteId[] {
  if (m.lane !== "ui_bridge") return [];
  return m.runtime.localRequired ? ["desktop-app-cdp", "desktop-app-ax"] : ["web"];
}

// ---------------------------------------------------------------------------
// Phone devices → routes. Shapes mirrored from Desktop's `phone.devices`
// (phone-tools.ts / phone-link-backend.ts / phone-link-types.ts) so the
// follow-up phone adapters plug into real response shapes, never invented
// ones.
// ---------------------------------------------------------------------------

// phone-link-backend.ts LinkedPhoneDeviceInfo (subset the router consumes).
export interface LinkedPhoneDeviceLike {
  serial: string; // "link:<deviceId>" for phone-link devices, "usb:…" for USB-agent
  platform: "ios" | "android";
  via?: string; // "phone-link" | "usb-agent"
  route?: string; // 'iphone-app-ictrl' | 'android-app' on phone-link devices
  health?: { state?: string }; // PhoneLinkHealthState
}

export const PHONE_LINK_HEALTH_ONLINE = new Set(["HEALTHY", "DEVICE_LOCKED"]);

// phone-link-health.ts online gate, verbatim semantics: a device counts as
// online when it was never checked, is HEALTHY, or is DEVICE_LOCKED (locked
// phones still answer; the unlock is the user's action, not an offline one).
export function phoneDeviceOnline(d: LinkedPhoneDeviceLike): boolean {
  const s = d.health?.state;
  return s === undefined || PHONE_LINK_HEALTH_ONLINE.has(s);
}

// Which route a `phone.devices` entry serves. usb-agent iPhones (serial
// "usb:…", via "usb-agent") are the vision route; phone-link entries carry
// their route explicitly.
export function routeForPhoneDevice(d: LinkedPhoneDeviceLike): RouteId | null {
  if (d.route === "android-app" && d.platform === "android") return "android-app";
  if (d.route === "iphone-app-ictrl" && d.platform === "ios") return "iphone-app-ictrl";
  if (d.platform === "ios" && (d.via === "usb-agent" || d.serial.startsWith("usb:"))) {
    return "iphone-app-usb-agent";
  }
  return null;
}
