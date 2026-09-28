/**
 * Local defaults for runtime feature gates (`tengu_*` GrowthBook keys).
 *
 * gizzi has no remote flag server, so without an entry here a gate returns
 * the hardcoded default at its call site — for most Claude Code-era gates
 * that is `false`, which silently disables a feature even when its
 * compile-time flag (script/features.mjs) is on. A remote value or a
 * GIZZI_INTERNAL_FC_OVERRIDES / /config override still wins over these.
 *
 * Only list gates whose compile-time flag is enabled and whose feature has
 * been checked live.
 */
const GIZZI_GATE_DEFAULTS: Readonly<Record<string, unknown>> = {
  // AWAY_SUMMARY: "while you were away" recap
  tengu_sedge_lantern: true,
  // EXTRACT_MEMORIES: background memory extraction (interactive sessions)
  tengu_passport_quail: true,
  // TRANSCRIPT_CLASSIFIER: auto mode appears in the shift+tab cycle (for
  // supported Claude models only); the first entry shows the consent dialog,
  // so it is never active without the user accepting it. ('opt-in' would
  // require a CLI flag gizzi doesn't have, leaving auto mode unreachable.)
  tengu_auto_mode_config: { enabled: 'enabled' },
  // TERMINAL_PANEL: meta+j opens a persistent shell (tmux-backed)
  tengu_terminal_panel: true,
}

export function gizziGateDefault<T>(gate: string, fallback: T): T {
  return Object.hasOwn(GIZZI_GATE_DEFAULTS, gate)
    ? (GIZZI_GATE_DEFAULTS[gate] as T)
    : fallback
}
