// Claude Code PostToolUse hook entry: reports "the call proceeded" as the outcome label for the
// shadow S1 permission GATE the PreToolUse guard logged. Never prints a decision; always exits 0.
import { reportToolRan } from "../src/hook/guard.ts";

try {
  await reportToolRan(JSON.parse(await Bun.stdin.text()));
} catch {
  // outcome labels are best-effort
}
process.exit(0);
