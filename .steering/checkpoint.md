Goal: Fix assigned review findings #14, #18–#22 in F5 worktree; draft PR only.
Just did: Read binding rules/authority and relocated all findings at ed3598a21. Created dag:dag_211503; wih:wih_3058 tracks #14. Reproduced live AUTO for mismatched deployed head, then bound calibration to actual runtime scope.
Next: Run decision regression suite, commit/push #14; implement lifecycle and graph/router regressions, compiler regressions.
Open questions: None. gate.rs changes limited to #18 status mutations; no replay/idempotency edits. No deploy paths intended.
