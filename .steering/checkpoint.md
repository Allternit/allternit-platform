# Steering checkpoint

## Goal
Finish all Eoj’s Bot Mode feedback and the Computer gap list, verified and published across Desktop, web phone and PWA. DAG dag_685396 / WIH wih_6366. Eoj explicitly requested all work, including publication (2026-10-11).

## Just did
Prior UI feedback source retained: full-width workspace editor with bottom chat; bounded sessions; banner Edit bot sole entry, thread Bot settings navigation; authenticated integration requests/retry; channel marks/wordmarks; bot-specific mascot thoughts and unframed avatars; rail replace/swap placement; 119 icons and 20 banner hues; easy custom image drop/paste; project banners with independent bannerColor.
Closed Computer gaps: existing shared approvals/picker/health retained, pending approval reconnect; message while watching and hand-back notes; authenticated bounded per-step redacted snapshot recording in direct/structured executor; live action caption and run selection/scrubbing/read-only replay; controller-only explicit clipboard and safe uniquely named file upload/drop; focused control shortcut. All shared desktop/web-phone/PWA consumers wired. Privacy: no raw typed/tool input in metadata; snapshots require successful redaction; account computer ACL; 500 steps/30 days; file limit25MB and clipboard256KB.
UI typecheck clean. Existing targeted UI suites45 pass; history/replay/file-drop tests + appearance8 pass. Docs links0 problems; dependency map including new history/transfer HTTP links validates (33 products,29 features). Release compilation found driver method mismatch (push_file, not write_file); corrected, rebuilding.

## Next
Complete Rust production build and filtered server tests, companion PR landing, clean merged-main build and publication of UI/PWA/runtime/docs, live UI verification. Preserve live Factory/Gizzi binaries and Eoj’s active sessions. Source commits will pass repo steering gate (backend absent; hook explicitly fails open with CONSULT_FAILED). Finish ledger and worktree/artifact cleanup after shipping.

## Open questions
No requested features deferred. Historical pre-feature runs cannot have retrospective screenshots. Shared checkout unrelated WIP must remain intact.

## Latest build base and integration verification
UI #515 Agency Kernel and #516 owned connector tool mapping are merged: fb4ed3e0a0ccb80b3a89acf31d1330f3346c2a66. ai and m new assets verified; PWA v105 published at https://72838ac3.allternit-remote-control.pages.dev. Desktop staged UI-source names this exact clean/current commit. Server branch includes origin/main 9c3da533aa (Agency Kernel/docs new pushes). Gizzi/Factory sources unchanged since installed be735416; retain hashes. Live old Desktop Connected Apps/Gateway load, Tools exposed mismatched actions-vs-tools metadata and an Electron optional socket QoS EINVAL; fixed in #516 and this companion shell guard. Full Desktop unit suites399 pass; main/preload compilation clean; package preflight all resources present. PR1503 all GitHub checks pass. Release build passed in10m36; isolated Rust tests compiling. No final runtime or shell installation claimed yet.
