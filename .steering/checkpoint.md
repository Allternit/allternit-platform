Goal: Fix F5 findings #14, #18–#22; draft PR only. dag:dag_211503.
Just did: #14 committed/pushed (929953603), decision suite 33/33. Reproduced #21 accepting policy-fixed numeric filesystem resource; unified type/enum/required validation now rejects it. Compiler regression + WP9 suite 19/19 passing. Read-only dependency symlinks in worktree only, not staged.
Next: Commit/push #21; reproduce and fix #22. #18 regression harness drafted; Rust test compile typo corrected before rerun. Graph/router next.
Open questions: None. No auth or harness-mode changes; gate.rs only #18 status validation.
