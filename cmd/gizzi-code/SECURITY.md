# Security

## IMPORTANT

We do not accept AI generated security reports. We receive a large number of
these and we absolutely do not have the resources to review them all. If you
submit one that will be an automatic ban from the project.

## Threat Model

### Overview

Gizzi Code is an AI-powered coding assistant that runs locally on your machine. It provides an agent system with access to powerful tools including shell execution, file operations, and web access.

### No Sandbox

Gizzi Code does **not** sandbox the agent. The permission system exists as a UX feature to help users stay aware of what actions the agent is taking - it prompts for confirmation before executing commands, writing files, etc. However, it is not designed to provide security isolation.

If you need true isolation, run Gizzi Code inside a Docker container or VM.

### Built-in safety floors

A few guardrails hold in every permission mode, including `bypassPermissions`
and `GIZZI_SKIP_PERMISSIONS`. They are defense in depth, not a sandbox.

- **Catastrophic command floor** (`src/runtime/tools/guard/permission/catastrophic.ts`).
  The bash tool refuses, and no mode, rule or approval can allow: recursive
  `rm` of `/`, `/*`, `~` or `$HOME`; `mkfs` and `diskutil erase…`; `dd` onto a
  raw disk device; fork bombs; shutdown/reboot/halt/poweroff; a force push
  (`--force`, `-f`, `--force-with-lease`, `+ref`, `--mirror`) to `main` or
  `master`, or one whose target branch isn't named; and macOS keychain reads
  (`security find-generic-password`, `find-internet-password`,
  `dump-keychain`). It sees through `sudo`/`env`/`bash -c`/`eval`/`$(…)` and
  quoting. A command it can't parse that names one of these programs is
  refused.
- **Configured denies beat bypass.** A `deny` rule in your `permission` config
  is honored in `bypassPermissions` too. Bypass skips asks; it never overrides
  an explicit deny.
- **Bash commands don't inherit your credentials.** Commands the agent runs get
  an allowlisted environment (PATH, HOME, locale, TERM, TMPDIR, SSH agent,
  proxies and CA bundles, and the node/pnpm/bun, rust, go, python, ruby, java,
  Homebrew, git, XDG, `GIZZI_*` and `ALLTERNIT_*` families). Anything that
  looks like a credential (`*_KEY`, `*_TOKEN`, `*_SECRET`, `*PASSWORD`,
  `*_PAT`, `*CREDENTIALS`) is removed, even inside those families. To pass a
  variable through, list it by name or `PREFIX_*` glob:

  ```json
  { "bash": { "env_passthrough": ["NPM_TOKEN", "AWS_*"] } }
  ```

  or set `GIZZI_BASH_ENV_PASSTHROUGH=NPM_TOKEN,AWS_*` in the environment
  gizzi starts with. `shell.env` plugin values are always passed.
- **Budget checks fail closed for bots.** If the spend-limit check itself
  errors, a bot session or a session with a thread budget pauses (reason
  `budget-check-failed`) and retries in 15 minutes. A plain interactive
  session with no budget logs a warning and continues.

### Server Mode

Server mode is opt-in only. When enabled, set `GIZZI_SERVER_PASSWORD` to require HTTP Basic Auth. Without this, the server runs unauthenticated (with a warning). It is the end user's responsibility to secure the server - any functionality it provides is not a vulnerability.

### Out of Scope

| Category                        | Rationale                                                               |
| ------------------------------- | ----------------------------------------------------------------------- |
| **Server access when opted-in** | If you enable server mode, API access is expected behavior              |
| **Sandbox escapes**             | The permission system is not a sandbox (see above)                      |
| **LLM provider data handling**  | Data sent to your configured LLM provider is governed by their policies |
| **MCP server behavior**         | External MCP servers you configure are outside our trust boundary       |
| **Malicious config files**      | Users control their own config; modifying it is not an attack vector    |

---

# Reporting Security Issues

We appreciate your efforts to responsibly disclose your findings, and will make every effort to acknowledge your contributions.

To report a security issue, please use the GitHub Security Advisory ["Report a Vulnerability"](https://github.com/Gizziio/allternit-platform/security/advisories/new) tab.

The team will send a response indicating the next steps in handling your report. After the initial reply to your report, the security team will keep you informed of the progress towards a fix and full announcement, and may ask for additional information or guidance.

## Escalation

If you do not receive an acknowledgement of your report within 6 business days, you may send an email to security@allternit.com.
