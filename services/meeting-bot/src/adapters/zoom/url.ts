// Zoom join-link parsing, per docs/meetings/CONTRACT.md section 7.
//
// Contract pattern (on the lowercased host):
//   ^https://([a-z0-9-]+\.)?zoom\.(us|com)/(?:j|my|wc/join)/(\d{9,11})(?:[/?#].*)?$
//   also /wc/<id>/join
//   url_key = zoom:<digits>; the `pwd` query param stays in join_url only.
//   Personal room /my/<name> has no number: url_key = zoom:my:<name>.
//
// The Meeting SDK joins by number (JoinParam4WithoutLogin.meetingNumber) or by
// personal-room name (JoinParam4WithoutLogin.vanityID); the passcode goes in `psw`.

export interface ZoomJoinTarget {
  /** Meeting number as a decimal string (9 to 11 digits). Absent for a personal-room link. */
  meetingNumber?: string;
  /** Personal-room name from /my/<name>, as written in the link. */
  vanityId?: string;
  /** The `pwd` query parameter, if the link carries one. Passed to the SDK as `psw`. */
  passcode?: string;
  /** Contract url_key: zoom:<digits> or zoom:my:<name>. */
  urlKey: string;
}

const HOST_RE = /^([a-z0-9-]+\.)?zoom\.(us|com)$/;
const NUMBER_RE = /^\d{9,11}$/;
const VANITY_RE = /^[a-z0-9][a-z0-9._-]{0,63}$/i;

/** Parse a Zoom join link. Returns null for anything the contract does not accept. */
export function parseZoomJoinUrl(raw: string): ZoomJoinTarget | null {
  let u: URL;
  try {
    u = new URL(raw.trim());
  } catch {
    return null;
  }
  if (u.protocol !== "https:") return null;
  if (u.username || u.password || u.port) return null;
  const host = u.hostname.toLowerCase();
  if (!HOST_RE.test(host)) return null;

  // Path segments without empty parts; tolerate one trailing slash.
  const seg = u.pathname.split("/").filter((s) => s.length > 0);
  let number: string | undefined;
  let vanity: string | undefined;

  if (seg.length >= 2 && (seg[0] === "j" || seg[0] === "my")) {
    if (NUMBER_RE.test(seg[1])) number = seg[1];
    else if (seg[0] === "my" && VANITY_RE.test(seg[1])) vanity = seg[1];
  } else if (seg.length >= 3 && seg[0] === "wc" && seg[1] === "join" && NUMBER_RE.test(seg[2])) {
    number = seg[2];
  } else if (seg.length >= 3 && seg[0] === "wc" && NUMBER_RE.test(seg[1]) && seg[2] === "join") {
    number = seg[1];
  }
  if (!number && !vanity) return null;

  const pwd = u.searchParams.get("pwd");
  const passcode = pwd && pwd.length > 0 ? pwd : undefined;

  if (number) return { meetingNumber: number, passcode, urlKey: `zoom:${number}` };
  return { vanityId: vanity, passcode, urlKey: `zoom:my:${vanity!.toLowerCase()}` };
}
