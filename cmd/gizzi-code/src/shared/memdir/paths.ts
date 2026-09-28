// src/shared code imports memdir paths relative to itself; the real
// resolution (GIZZI_CONFIG_DIR / ~/.gizzi, remote override, settings) lives in
// src/memdir/paths.ts. A separate copy here pointed shared code at a
// different directory (~/.config/gizzi/memory) and ignored the auto-memory
// setting.
export * from '../../memdir/paths.js'
