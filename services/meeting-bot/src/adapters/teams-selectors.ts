// Every Microsoft Teams selector the adapter uses, in one place. Teams changes its
// DOM without notice; when a join breaks, this file is the only one to edit.
//
// Preference order: ARIA role + accessible name, then data-tid (Teams' own stable
// test ids), then visible text. No generated class names.
//
// Status words in "source":
//   VERIFIED 2026-11-13   seen on the live pre-join page (teams.live.com and
//                         teams.microsoft.com, headless Chrome, signed out, invalid meeting id)
//   UNVERIFIED            recalled from the 2025 Teams web client (open-source bots such as
//                         vexa-ai/vexa and attendee); needs a live admitted call to confirm.

export interface TextMatch {
  /** Regex source, matched case-insensitively against role names or text. */
  pattern: string;
  source: string;
}
export interface CssMatch {
  css: string;
  source: string;
}

export const teams = {
  // ---- launcher page (teams.microsoft.com/l/meetup-join, teams.live.com/meet) ------
  /** "Continue on this browser" (teams.microsoft.com) / "Join meeting from this browser" (teams.live.com). */
  continueOnBrowser: {
    pattern: "^(Continue on this browser|Join meeting from this browser)$",
    source: "VERIFIED 2026-11-13: launcher button on teams.live.com/dl/launcher; page text reads 'Continue on this browser'",
  } satisfies TextMatch,

  /** Browser prompt to open the Teams desktop app; never wanted. */
  dismissOpenApp: {
    pattern: "^(Cancel|Not now|Stay on this browser)$",
    source: "UNVERIFIED: 'Open Teams?' interstitial in some builds",
  } satisfies TextMatch,

  // ---- pre-join ----------------------------------------------------------------------
  /** Guest display-name field ("Type your name"). */
  nameInput: {
    css: '[data-tid="prejoin-display-name-input"], input[placeholder="Type your name"]',
    source: "VERIFIED 2026-11-13: pre-join text input, data-tid prejoin-display-name-input, placeholder 'Type your name'",
  } satisfies CssMatch,

  /** Camera and mic toggles on pre-join (checkbox inputs, checked = on). */
  camToggle: {
    css: '[data-tid="toggle-video"]',
    source: "VERIFIED 2026-11-13: pre-join checkbox data-tid toggle-video",
  } satisfies CssMatch,
  micToggle: {
    css: '[data-tid="toggle-mute"]',
    source: "VERIFIED 2026-11-13: pre-join checkbox data-tid toggle-mute ('Mic on' text beside it)",
  } satisfies CssMatch,

  /** The join button, "Join now". */
  joinButton: {
    pattern: "^Join now$",
    source: "VERIFIED 2026-11-13: button aria-label 'Join now', data-tid prejoin-join-button",
  } satisfies TextMatch,
  joinButtonCss: {
    css: '[data-tid="prejoin-join-button"]',
    source: "VERIFIED 2026-11-13",
  } satisfies CssMatch,

  /** Device dialog when getUserMedia fails. Ours should not fail; this is a fallback. */
  continueWithoutDevices: {
    pattern: "^Continue without (audio or video|audio|video)$",
    source: "UNVERIFIED: Teams device-permission dialog button text",
  } satisfies TextMatch,

  // ---- admission -----------------------------------------------------------------------
  /** Lobby copy. */
  waitingText: {
    pattern:
      "let you in soon|waiting for someone to let you in|we've let people in the meeting know you're waiting|Someone in the meeting should let you in",
    source: "UNVERIFIED: Teams lobby copy ('Someone in the meeting should let you in soon')",
  } satisfies TextMatch,

  /** Host declined, or nobody answered in time. A decline is final. */
  deniedText: {
    pattern:
      "denied your request|declined your request|request to join was (denied|declined)|no one (let|admitted) you in|nobody (let|admitted) you in|unable to connect you|wasn't able to let you in|You were removed from this meeting|You've been removed",
    source: "UNVERIFIED: Teams lobby-denied and lobby-timeout copy ('Sorry, but you were denied access to the meeting', 'No one let you in')",
  } satisfies TextMatch,

  /** Bad, expired or deleted link; seen after "Join now" on an invalid id. */
  notFoundText: {
    pattern: "We can't find this meeting|meeting might have ended, been deleted",
    source: "VERIFIED 2026-11-13: page shown after Join now on an invalid meeting id (teams.live.com)",
  } satisfies TextMatch,

  /** The meeting has ended (host ended it, or the last person left). */
  endedText: {
    pattern: "meeting has ended|meeting ended|This meeting is over|Meeting ended",
    source: "UNVERIFIED: Teams post-call copy",
  } satisfies TextMatch,

  /** Anonymous join refused by the tenant or the organizer. */
  guestBlockedText: {
    pattern:
      "Sign in to join|sign in with (a|your) (work|school)|only people (in|with)|organi[sz]ation (doesn't|does not|has not) allow|doesn't allow (anonymous|guests|external)|anonymous (users|people|participants) can't join|can't join this meeting|couldn't join this meeting because|verify your (info|identity|email)|enter the code|Meeting requires sign-?in",
    source: "UNVERIFIED: tenant policy copy ('Anonymous users can't join this meeting', 'Sign in to join'), plus organizer verify option and Teams Premium email-code step",
  } satisfies TextMatch,

  /** Signed-out "Sign in" link on pre-join; seen on every pre-join page, so NOT a block by itself. */
  signInLink: {
    css: '[data-tid="auth-sign-in-link"]',
    source: "VERIFIED 2026-11-13: pre-join 'Sign in' link; present on a normal anonymous pre-join page",
  } satisfies CssMatch,

  // ---- in call ---------------------------------------------------------------------------
  /** Present only while in the call (also shown in the lobby on some builds, so pair with waitingText). */
  leaveButton: {
    pattern: "^(Leave|Hang up)$",
    source: "UNVERIFIED: in-call toolbar button aria-label 'Leave' (data-tid hangup-button)",
  } satisfies TextMatch,
  leaveButtonCss: {
    css: '[data-tid="hangup-button"], button#hangup-button',
    source: "UNVERIFIED: in-call hang-up button test id",
  } satisfies CssMatch,

  /** Confirmation after Leave in some tenants ("Leave" / "Cancel"). */
  leaveConfirm: {
    pattern: "^Leave$",
    source: "UNVERIFIED: leave-confirmation dialog button",
  } satisfies TextMatch,

  /** "More" overflow in the toolbar, holds captions on narrow windows. */
  moreButton: {
    pattern: "^More( actions)?$",
    source: "UNVERIFIED: toolbar button aria-label 'More'",
  } satisfies TextMatch,

  // ---- people ------------------------------------------------------------------------------
  peopleButton: {
    pattern: "^(People|Show participants|Participants)",
    source: "UNVERIFIED: toolbar aria-label 'People' (data-tid roster-button)",
  } satisfies TextMatch,
  peopleButtonCss: {
    css: '[data-tid="roster-button"]',
    source: "UNVERIFIED",
  } satisfies CssMatch,

  /** Rows of the People panel; the row's text starts with the participant name. */
  participantRows: {
    css: '[data-tid="roster-list"] [role="listitem"], [role="tree"][aria-label*="articipants" i] [role="treeitem"], [data-tid="participantsInCall"] [role="listitem"]',
    source: "UNVERIFIED: People pane rows (role treeitem/listitem under the participants tree)",
  } satisfies CssMatch,

  /** The name node inside a roster row, when present. */
  participantRowName: {
    css: '[data-tid="participant-name"], [data-tid^="participant-item-name"], span[id^="roster-avatar-img"] + span',
    source: "UNVERIFIED",
  } satisfies CssMatch,

  /** Video tiles in the stage. aria-label holds the name, "<Name>, Video is off, Unmuted" style. */
  stageTile: {
    css: '[data-cid="calling-participant-stream"], [data-tid^="video-tile"], [data-stream-type]',
    source: "UNVERIFIED: stage tile containers",
  } satisfies CssMatch,

  /** Speaking indicator inside a stage tile (voice-level ring). */
  speakingIndicator: {
    css: '[data-tid="voice-level-stream-outline"], [data-tid="voice-level"] , [data-is-speaking="true"]',
    source: "UNVERIFIED: voice-level outline test id; Teams exposes no ARIA for this. Captions are the cross-check.",
  } satisfies CssMatch,

  // ---- chat ---------------------------------------------------------------------------------
  chatButton: {
    pattern: "^(Chat|Show conversation|Open chat)",
    source: "UNVERIFIED: toolbar aria-label 'Chat' (data-tid chat-button / toggle-chat)",
  } satisfies TextMatch,
  chatButtonCss: {
    css: '[data-tid="chat-button"], [data-tid="toggle-chat"], [data-tid="calling-chat-button"]',
    source: "UNVERIFIED",
  } satisfies CssMatch,

  chatInput: {
    css: '[data-tid="ckeditor"][contenteditable="true"], div[role="textbox"][aria-label^="Type a message" i], [data-tid="chat-pane-compose-message-footer"] [contenteditable="true"]',
    source: "UNVERIFIED: chat compose box (CKEditor contenteditable), aria-label 'Type a message'",
  } satisfies CssMatch,

  chatSend: {
    css: '[data-tid="newMessageCommands-send"], button[aria-label="Send"]',
    source: "UNVERIFIED: chat send button, aria-label 'Send'",
  } satisfies CssMatch,

  /** Chat unavailable to anonymous users in some tenants. */
  chatDisabledText: {
    pattern: "chat is (turned off|disabled|not available)|can't (send|post) messages|Chat has been disabled|Anonymous users can't chat",
    source: "UNVERIFIED: copy shown in place of the compose box when the organizer disabled chat or anon users have none",
  } satisfies TextMatch,

  /** One element per chat message in the meeting chat pane. */
  chatMessage: {
    css: '[data-tid="chat-pane-message"], [data-tid="chat-pane-item"] [data-mid]',
    source: "UNVERIFIED: message bubble in the meeting chat pane",
  } satisfies CssMatch,
  chatMessageId: {
    css: "[data-mid]",
    source: "UNVERIFIED: message id attribute (data-mid) on the message or its container",
  } satisfies CssMatch,
  chatAuthor: {
    css: '[data-tid="message-author-name"]',
    source: "UNVERIFIED: author name node in a message",
  } satisfies CssMatch,
  chatBody: {
    css: '[data-tid="chat-pane-message"] [id^="content-"], [data-tid="message-body"], [id^="content-"]',
    source: "UNVERIFIED: message body node",
  } satisfies CssMatch,

  // ---- live captions ---------------------------------------------------------------------------
  /** Menu entry that turns captions on. Also Ctrl+Shift+U. */
  captionsMenuItem: {
    pattern: "^(Turn on live captions|Live captions)$",
    source: "UNVERIFIED: More > Language and speech > 'Turn on live captions'",
  } satisfies TextMatch,
  languageSpeechMenuItem: {
    pattern: "^Language and speech$",
    source: "UNVERIFIED: More menu entry that holds captions",
  } satisfies TextMatch,
  captionsWindow: {
    css: '[data-tid="closed-caption-v2-window-wrapper"], [data-tid="closed-caption-renderer-wrapper"]',
    source: "UNVERIFIED: live captions container",
  } satisfies CssMatch,
  captionRow: {
    css: '[data-tid="closed-caption-message"], .ui-chat__message, [data-tid="closed-caption-v2-virtual-list-content"] > div',
    source: "UNVERIFIED: one caption entry (speaker + text)",
  } satisfies CssMatch,
  captionAuthor: {
    css: '[data-tid="author"], [data-tid="closed-caption-author"]',
    source: "UNVERIFIED: caption speaker name",
  } satisfies CssMatch,
  captionText: {
    css: '[data-tid="closed-caption-text"]',
    source: "UNVERIFIED: caption text node",
  } satisfies CssMatch,
} as const;

export function rx(m: TextMatch): RegExp {
  return new RegExp(m.pattern, "i");
}
