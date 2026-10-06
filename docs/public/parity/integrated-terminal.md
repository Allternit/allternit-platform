# Integrated terminal parity

ChatGPT/Codex's integrated terminal runs project commands without leaving the work surface. Allternit provides both the `gizzi` TUI's shell tools and server-backed real PTYs on the Allternit Factory pane engine. Terminal sessions preserve scrollback and can survive API restarts.

## Run and validate your project

Ask `gizzi` to run a bounded validation task, or execute the project command directly in the interactive shell:

```bash
cd /path/to/project
gizzi exec "Run the focused tests for the parser, fix failures, and report the exact commands and results"
```

The model-facing `bash` tool returns stdout, stderr, exit code, and success. Use `code_execution` when you need a sandboxed Python, Node, Bash, or Rust snippet. Keep the workspace sandbox enabled and require approval for broad or destructive commands.

Web/desktop clients can create a PTY through the terminal API:

```bash
curl -s http://127.0.0.1:8013/terminal/create \
  -H 'Content-Type: application/json' \
  -d '{"shell":"/bin/zsh","cwd":"/path/to/project","cols":120,"rows":32}'
```

Use the returned session ID with `POST /terminal/{id}/input`, `POST /terminal/{id}/resize`, `GET /terminal/{id}/stream`, and `POST /terminal/{id}/close`. The stream replays persisted scrollback before live SSE output. The current server implementation uses Unix sockets and defaults to `/bin/zsh`, so native Windows PTYs are roadmap; run it in WSL/Linux.

## Create reusable actions

Allternit skills are reusable prompt-and-tool packages. Put a project action in `.gizzi/skills/run-tests/SKILL.md` (or a user-wide skill directory):

```markdown
---
name: run-tests
version: 1.0.0
description: Run focused project validation and summarize failures.
tools: [bash, file.read]
permission_mode: ask
---

Detect the affected package from the current diff. Run its smallest relevant
test command, then report the command, exit status, and first actionable error.
Do not run a full workspace build unless the user explicitly asks.
```

Plugins can bundle skills, slash commands, hooks, agents, MCP servers, and LSP servers for distribution. Use project instructions for always-on conventions and skills for explicit reusable workflows.

See [Build skills](../cli/build-skills.md), [Build plugins](../cli/build-plugins.md), and [agent approvals](../cli/agent-approvals-security.md).
