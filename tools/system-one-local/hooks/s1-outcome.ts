// Claude Code outcome hook for the shadow S1 permission GATEs (WP-S1U-3). Register it for
// PermissionRequest, PostToolUse, PostToolUseFailure and Stop (see README). It dispatches on
// hook_event_name, never prints a decision, and always exits 0.
import { handleOutcomeHook } from "../src/hook/guard.ts";

try {
  await handleOutcomeHook(JSON.parse(await Bun.stdin.text()));
} catch {
  // outcome labels are best-effort
}
process.exit(0);
