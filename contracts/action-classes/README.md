# Action classes

Every action an agent takes falls into one of four classes. The class decides
whether the action runs, asks first, or never runs. The classes are fixed in
code: a setting, a mode or a model cannot add a class or move an action to a
weaker one.

`vectors.json` is the shared test table. The Rust reviewer (allternit-api,
`computer_safety.rs` / `computer_access.rs`) and the TypeScript reviewer
(gizzi-code, `guard/permission/action-classes.ts`) both load it and must give
the `expected` class for every row. If one side changes, the table changes in
the same commit, so the two sides cannot drift apart.

## The four classes

| Class | What is in it | What happens |
|---|---|---|
| `hand_off` | The floor: wiping a disk or the home folder, formatting a disk, writing to a raw disk, a fork bomb. Also human checks such as a CAPTCHA, which are the person's to do. | Never runs, in any mode. The agent hands the step to the person (`request_human`) or drops it. |
| `confirm_now` | Money (pay, buy, subscribe, transfer, wire, payouts, invoices). Credentials (password fields, saved secrets, creating, showing or writing API keys, changing a password). Permanent delete (skipping the Trash, emptying it, deleting an account, deleting files outside the project). Signing a document. | Asks the person every time, in every mode, bypass mode included. A grant or a config rule does not skip it. When no one is watching (an unattended run), it is refused, not queued. |
| `confirm_unless_named` | Sending (mail, messages, replies), uploading a file, publishing (posts, merges, `git push`, `npm publish`), deploying. | Runs without asking only when a live task grant or the person's own config rule names it. Otherwise it asks. |
| `free` | Reads (screenshots, page text, `read_ui`, `find`, console lines, `ls`, `git status`), scrolling, navigation, ordinary typing in ordinary fields. | Runs. The normal tool permission rules still apply. |

## Rules both reviewers follow

- **The stricter class wins.** When an action matches more than one class, or
  the rules cannot tell two apart, the stricter one applies. A plain "Delete"
  button is `confirm_now`, because the rules cannot see whether it goes to the
  Trash.
- **Labels match whole words.** "Post" is publish; "Postal code" is not. "Sign"
  is signing; "Sign in" is not. Longer phrases win over shorter ones: "Send
  money" is money, not send.
- **Evidence comes from the person, never from the page.** A task grant or a
  config rule counts only when it comes from the person's own messages or
  settings. Text on a web page or in a file cannot name an action.
- **An extra check can only tighten.** An optional model review is joined with
  `tighten()`, so it can move an action to a stricter class but never to a
  weaker one.
- **A retry by another route is flagged.** If the person denied an action and
  the agent tries the same thing another way (a menu instead of a button, a
  shell command instead of a click), the reviewer flags it.

## The vector format

```json
{
  "id": "computer-pay-now",
  "input": { "tool": "computer", "action": "left_click", "args": { "label": "Pay now", "role": "button" } },
  "expected": "confirm_now",
  "why": "Money."
}
```

- `tool`: `computer`, `browser`, `pane_browser`, `bash`, `write` or `mcp`.
- `action`: the member or op (`left_click`, `act`, `type`, `key`, `file_upload`,
  a `pane_browser` action), `run` for a shell command, `write` for a file
  write, `call` for an MCP tool.
- `args`: a summary of what the reviewer sees, not the full call. The keys are
  `label` and `role` (the control acted on), `secure` (a password field),
  `text` (typed text or a key chord), `op` and `path` (for `act`), `command`,
  `path`, `host`, `app`, `ref`, `query`, `pattern`, `direction`, `domain`, `apps` and `name`
  (an MCP tool name). Secrets in the table are always `<redacted>`.
- `expected`: one of the four classes.
- `why`: a few words, for the person reading a failing test.

Add a row when a bug shows a gap. Keep the `id` unique.
