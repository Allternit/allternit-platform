// Claude Code PreToolUse hook entry. Reads the hook JSON on stdin, prints a
// deny/ask decision (advise mode) or nothing. Any internal failure → print
// nothing and exit 0, i.e. fall back to the normal permission flow.
import { runGuard } from "../src/hook/guard.ts";

try {
  const raw = await Bun.stdin.text();
  const { output } = await runGuard(JSON.parse(raw));
  if (output) process.stdout.write(JSON.stringify(output));
} catch {
  // fail open to Claude Code's own permission flow — emit no decision
}
process.exit(0);
