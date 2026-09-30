Goal: F5 fixes #14, #18–#22; draft PR only; dag:dag_211503.
Just did: Every assigned finding reproduced with failing tests. #14 and compiler fixes are published on the task branch. Lifecycle integration 4/4, kernel suite 58/58, conformance 25/25, judge 21/21, decision suite 33/33, compiler suite 21/21. Shared role/mode resolution rejects inferred S2 writes and invalid/nonlegal mode restrictions. gate.rs changes confined to validate_mutations status checks.
Next: Publish #19, #20, #18 as separate fixes; finish draft PR and FIX_F5_NOTES.md. Final integration check reruns after graph/router edits.
Open questions: None. No auth/launch-mode changes, deploy paths, migrations, broad builds, or shared-checkout writes. Preserve worktree for draft review.
