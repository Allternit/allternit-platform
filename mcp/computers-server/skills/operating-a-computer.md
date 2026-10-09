# Skill: operating a computer with the computers tools

Moved here 2026-10-09 from the retired `platform/packages/computer-use/plugins`
package. The parts that still apply are kept and rewritten for the tools this
server exposes (`computers.*`, `templates.*`).

**Triggers:** "open the app", "click", "fill the form", "take a screenshot of the
computer", any task that needs a running cloud computer.

## Loop: look, act once, look again

1. `computers.list` / `computers.get` to find the computer. If it is stopped,
   `computers.start` and wait until `status` is `running`.
2. `computers.screenshot` before every action. Never act on a stale image.
3. One action per step: `computers.mouse` (move, click, drag, scroll) or
   `computers.keyboard` (type, key). Use `computers.shell` for anything a
   command can do faster than the UI (opening apps, reading files).
4. `computers.screenshot` again and check the change you expected happened.
   If the screen is unchanged, do not repeat the same click blindly.

## Coordinates

Coordinates are pixels in the most recent screenshot. Aim for the centre of
the element. After any scroll or window change, take a new screenshot first.

## Forms

Click the field, then `computers.keyboard` type. Check each field in the next
screenshot before submitting. Never type passwords or payment details the user
did not give you in this session; ask for them.

## Files

`computers.files.upload` puts a local file on the computer;
`computers.files.download` brings one back. Prefer these over pasting file
contents through the keyboard.

## When something fails

| Class | Examples | What to do |
|-------|----------|------------|
| Transient | page still loading, element not visible yet | wait, screenshot, retry once or twice |
| Moved | element is somewhere else, layout changed | screenshot and find it again |
| State | already submitted, already signed in | detect it and skip the step |
| Auth | session expired, sign-in needed | stop and ask the user |
| Policy | CAPTCHA, bot check, rate limit | stop and tell the user; never try to bypass it |
| Fatal | computer stopped, tool errors repeat | report the error and stop |

Before risky work, `computers.snapshots.create`; if the task goes wrong,
`computers.snapshots.restore` returns the computer to that point.
