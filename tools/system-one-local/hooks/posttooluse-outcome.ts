// Legacy PostToolUse entry (kept so existing settings keep working). hooks/s1-outcome is the one
// entry for every outcome event; this one dispatches the same way. Never prints a decision.
import { handleOutcomeHook } from "../src/hook/guard.ts";

try {
  await handleOutcomeHook(JSON.parse(await Bun.stdin.text()));
} catch {
  // outcome labels are best-effort
}
process.exit(0);
