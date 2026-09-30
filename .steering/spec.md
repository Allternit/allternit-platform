# F5 kernel/S1/compiler review fixes

Authority and scope: COMMON_RULES.md and REVIEW_FIX_TASK.md in Research/agency-kernel-reconciliation-2026-09-29/tasks; assigned findings #14, #18, #19, #20, #21, #22. Task state lives in dag:dag_211503.

Required behavior:
- #14: live S1 cannot serve a calibration manifest for a different deployed readout head. Matching calibration still permits reversible AUTO after Q22 passes.
- #18: ChangeStatus validates its declared source against current projected state and its target against the lifecycle table; batch rejection emits no mutation records. CLI update/close/reopen use actual state; invalid status cannot partially update metadata. Projection rejects illegal/unknown changes using accumulated state while retaining compatibility with historical absent or incorrect from fields.
- #19: graph invariants use the same effective cognitive role as routing. Inferred S2 mutation/write nodes are rejected; S2 candidates must reach verification/policy.
- #20: every mode in a nonempty restriction must parse; role-incompatible restrictions are rejected. Empty restrictions retain all legal modes. Graph validation and routing share mode resolution.
- #21: policy-fixed winning arguments pass required-value, type, and enum checks before resource extraction; valid policy overrides retain POLICY provenance.
- #22: serialized compressed chunk content controls its CAS hash/reference and token estimate; original source hash remains provenance. Fingerprints include rendered bytes, including source headers and trust fencing.

Verification: regression per finding demonstrated failure first; targeted decision/compiler, kernel, lifecycle integration, conformance, and judge suites must pass. Only lean tests, shared Rust target, no application build or dev server.

Boundary: assigned worktree only. gate.rs changes limited to #18 validate_mutations; no replay/idempotency edits, auth changes, or harness auto-approve changes. Publish separate fix commits and one draft PR; no merge/deploy/release. Write requested FIX_F5_NOTES.md and retain resumable worktree for review.
