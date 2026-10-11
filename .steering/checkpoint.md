# Bot animation continuation

## Goal
Ground the selected bot mascot on the composer and animate all template identities consistently across rail, headers, launcher and previews, on desktop and both phone surfaces.

## Just did
Shared sprite renderer replaces static template/vendor portraits. Hover/focus and real working threads drive animation; reduced motion and visibility pause loops. Composer placement removes selected-bot positive gap. UI 51 tests pass (one existing skipped), TypeScript clean. Docs and map updated. Task dag_463331/n_1306/wih_9076 is in the durable shared platform root.

## Next
Merged-main build, live composer/rail/header animation check, desktop signed runtime and phone PWA publication, ledger and own cleanup. Preserve unrelated shared WIP.

## Open questions
None. Apple notarization credentials remain unavailable; this is the local development distribution. Native API/Factory/Gizzi code unchanged and verified installed binaries retained.
