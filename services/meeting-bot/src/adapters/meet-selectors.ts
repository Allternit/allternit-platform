// Every Google Meet selector the Meet adapter uses, in one place. Meet changes
// its DOM without notice; when a join breaks, this file is the only one to edit.
//
// Preference order: ARIA role + accessible name, then aria-label, then visible
// text, then data-* attributes, and only as a last resort Meet's obfuscated
// class names (they change on every Meet release).
//
// Copied from spike/meet-selectors.ts (step 0.3) and extended for step 2.2
// (signed-in bot profile, captions, people count, in-call dialogs). "Source"
// says where each one comes from. Mark a line VERIFIED <date> once a live run
// has used it. None is verified by this lane: no real Meet call was made.

export interface TextMatch {
  /** Regex source, matched case-insensitively against role names or text. */
  pattern: string;
  source: string;
}

export interface CssMatch {
  css: string;
  source: string;
}

export const meet = {
  // ---- pre-join screen -----------------------------------------------------
  /** Sign-in nudge shown to signed-out guests ("Sign in" / "Got it"). */
  dismissSignIn: {
    pattern: "^(Got it|Dismiss|No thanks|Not now|Close)$",
    source: "Meet guest pre-join, button text (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** Guest display name field. */
  nameInput: {
    css: 'input[aria-label="Your name"], input[placeholder="Your name"]',
    source: "Meet guest pre-join name field aria-label/placeholder (recalled 2025 Meet DOM, not checked this session)",
  } satisfies CssMatch,

  /** Mic/cam toggles on pre-join and in-call. The name carries a shortcut suffix, e.g. "Turn on microphone (ctrl + d)". */
  micOn: { pattern: "^Turn on microphone", source: "Meet toggle aria-label (recalled 2025 Meet DOM, not checked this session)" } satisfies TextMatch,
  camOn: { pattern: "^Turn on camera", source: "Meet toggle aria-label (recalled 2025 Meet DOM, not checked this session)" } satisfies TextMatch,

  /** The join button: "Ask to join" for guests, "Join now" when the host allows open joins. */
  joinButton: {
    pattern: "^(Ask to join|Join now|Join anyway)$",
    source: "Meet pre-join primary button text (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** A "continue without devices" dialog appears when getUserMedia fails. Ours should not fail; clicking it is a fallback. */
  continueWithoutDevices: {
    pattern: "^Continue without (microphone and camera|camera|microphone)$",
    source: "Meet device-permission dialog button text (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  // ---- admission -------------------------------------------------------------
  waitingText: {
    pattern: "Asking to be let in|Please wait until a meeting host brings you|someone in the call (will|to) let you in",
    source: "Meet waiting-room copy (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  deniedText: {
    pattern:
      "You can't join this call|denied your request to join|No one responded to your request|You've been removed from the meeting|This meeting has ended",
    source: "Meet denial / end copy (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** Guest blocked by the host's org policy (Workspace "only people in the org" or "signed-in users only"). */
  guestBlockedText: {
    pattern: "can't join this video call|You can't create a meeting yourself|Sign in to join|only people with a Google account",
    source: "Meet guest-restriction copy (recalled 2025 Meet DOM, not checked this session); see guest_blocked risk in the plan",
  } satisfies TextMatch,

  /** Present only while in the call. */
  leaveButton: {
    pattern: "^Leave call$",
    source: "Meet in-call toolbar aria-label (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  // ---- people ----------------------------------------------------------------
  peopleButton: {
    pattern: "^(People|Show everyone)",
    source: "Meet toolbar button aria-label; was 'Show everyone', now 'People' (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** Rows of the People panel. Each listitem's aria-label is the participant name. */
  participantRows: {
    css: '[role="list"][aria-label="Participants"] [role="listitem"]',
    source: "Meet People side panel list (recalled 2025 Meet DOM, not checked this session)",
  } satisfies CssMatch,

  /** Video tiles; one per participant in the grid. */
  participantTile: {
    css: "[data-participant-id]",
    source: "Meet tile data attribute, stable across 2023-2025 (as used by the vexa-ai/vexa and attendee Meet adapters; not re-checked this session)",
  } satisfies CssMatch,

  /** Name element inside a tile (first non-empty one wins). */
  tileName: {
    css: '[data-self-name], .notranslate, [jsslot] span',
    source: "Meet tile name label; data-self-name only on the self tile (recalled 2025 Meet DOM, not checked this session)",
  } satisfies CssMatch,

  /**
   * Active-speaker indicator inside a tile. Meet exposes no ARIA for this.
   * These are obfuscated class names on the speaking-ring/audio-level element
   * observed by open-source bots in 2025; expect them to break. The live run
   * should record the current classes (logged by join.ts as tileClassSample).
   */
  speakingClasses: {
    css: ".Oaajhc, .HX2H7, .wEsLMd, .OgVli",
    source: "vexa-ai/vexa Meet speaker detection as recalled (2025; not re-checked), obfuscated, UNSTABLE",
  } satisfies CssMatch,

  // ---- chat ------------------------------------------------------------------
  chatButton: {
    pattern: "^Chat with everyone",
    source: "Meet toolbar button aria-label (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  chatInput: {
    css: 'textarea[aria-label="Send a message"], textarea[aria-label^="Send a message"]',
    source: "Meet chat panel textarea aria-label; was 'Send a message to everyone' (recalled 2025 Meet DOM, not checked this session)",
  } satisfies CssMatch,

  chatSend: {
    pattern: "^Send a message$",
    source: "Meet chat send button aria-label (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** One element per chat message; the id makes dedupe cheap. */
  chatMessage: {
    css: "[data-message-id]",
    source: "Meet chat message data attribute (as recalled from the attendee Meet chat reader, 2025; not re-checked)",
  } satisfies CssMatch,

  /** Sender group around messages; holds the sender name. */
  chatSenderGroup: {
    css: "[data-sender-id], [data-sender-name]",
    source: "Meet chat sender block (recalled 2025 Meet DOM, not checked this session), UNVERIFIED",
  } satisfies CssMatch,
  // ---- step 2.2 additions -------------------------------------------------------
  /** Signed-in pre-join shows the account chip; a signed-out profile shows the guest name field instead. */
  signedOutHint: {
    pattern: "^Sign in$",
    source: "Meet signed-out pre-join 'Sign in' button (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** One-off in-call dialogs that cover the toolbar ("Got it" on tips, "Dismiss" on the safety banner). */
  inCallDismiss: {
    pattern: "^(Got it|Dismiss|OK|Close)$",
    source: "Meet in-call tip/banner buttons (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** The screen after the bot is removed or the call ends for everyone. */
  removedText: {
    pattern: "You've been removed from the meeting|You have been removed|The call ended for everyone|This call has ended|You left the meeting|Rejoin",
    source: "Meet post-call screen copy (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** The People toolbar button carries the participant count, e.g. aria-label "People - 3 joined" or a badge "3". */
  peopleCount: {
    css: 'button[aria-label^="People"], button[aria-label^="Show everyone"]',
    source: "Meet toolbar People button; count read from its text/aria-label digits (recalled 2025 Meet DOM, not checked this session)",
  } satisfies CssMatch,

  /** Captions toggle in the toolbar. */
  captionsOn: {
    pattern: "^Turn on captions",
    source: "Meet toolbar captions toggle aria-label (recalled 2025 Meet DOM, not checked this session)",
  } satisfies TextMatch,

  /** Captions region; each child block holds a speaker name and the running text. */
  captionsRegion: {
    css: '[role="region"][aria-label="Captions"], div[jsname="dsyhDe"]',
    source: "Meet captions container: ARIA region first, jsname fallback as used by open-source caption readers (2025; not re-checked), UNSTABLE",
  } satisfies CssMatch,

  /** Speaker name inside one caption block (img alt or the first text span). */
  captionSpeaker: {
    css: "img[alt], .NWpY1d, .zs7s8d",
    source: "Meet caption block speaker label (recalled 2025 Meet DOM, not checked this session), obfuscated, UNSTABLE",
  } satisfies CssMatch,

  /** Caption text inside one block. */
  captionText: {
    css: ".bh44bd, .ygicle, [jsname='tgaKEf']",
    source: "Meet caption block text span (recalled 2025 Meet DOM, not checked this session), obfuscated, UNSTABLE",
  } satisfies CssMatch,
} as const;

export function rx(m: TextMatch): RegExp {
  return new RegExp(m.pattern, "i");
}
