# agent-frame fixtures

Real screens captured 2026-11-20 from Claude Code, Codex, Kimi, Antigravity (agy), opencode and Gizzi, as plain text (`*.txt`, one screen each, bytes as captured), plus `expected.json`: what `parseAgentFrame` (src/lib/terminal/agent-frame.ts) must return for each screen (`null` = not an agent frame, show the full terminal).

Language-neutral on purpose: the iOS (Swift) and Android (Kotlin) parsers are tested against these same files.

How a screen becomes rows (same as the terminal caller): split on `\n`, drop the one trailing empty piece, then pad with empty rows to 40 (the captured viewport height). `hideFrom` for an open menu is the row count (40). The terminal title is empty.

Known gap, encoded as `null`: `kimi-trust.txt` (Kimi's trust dialog puts its key hint above the options, which the parser does not read yet; the app falls back to the full terminal).

Source of truth: `allternit-ai/src/lib/terminal/__fixtures__/agent-frame/` (added in ai PR #1217). This folder is a byte-for-byte copy for the iOS (`ios/Tests/AgentFrameTests.swift`) and Android parser tests. Change the screens or `expected.json` in the ai folder first, then copy them here unchanged.
