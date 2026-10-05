---
name: Build, check, prove
description: Build one piece of work, check it against its proof contract, and hold it for a person to approve before anything ships.
---

# Build, check, prove

A builder does the work. A checker runs the checks named in the proof
contract. If the check fails, the work goes back to the builder with the
checker's findings, up to `max_rounds` times. After that the flow stops as
degraded and asks a person what to do. A person approves the result by
looking at the proof files, not a summary. Nothing ships automatically.

Roles come from the team's `team.yaml`: `build` and `check`.

```yaml template-spec
params:
  - name: intent
    description: What to build, in the person's words.
max_rounds: 3
steps:
  - id: build
    title: Build it
    description: >-
      {{ params.intent }}

      Write PROGRESS.md as you go. For each line of the proof contract, put
      the evidence in proof/ and list it in PROOF.md. If this step was routed
      back, read the check findings appended below and fix what they report.
    executor: role:build
  - id: check
    title: Check it
    description: >-
      Run the checks named in the proof contract against the build
      ({{ build.output_path }}). Fail this step if any contract line has no
      evidence in proof/, or if a check does not pass. Say which line failed
      and why, so the builder can fix it.
    executor: role:check
    blocked_by: [build]
    on_fail: build
  - id: prove
    title: Person approves the result
    description: >-
      A person reads PROOF.md and the files in proof/ and approves the
      result. Approving does not ship or publish anything.
    blocked_by: [check]
    wait_gate:
      kind: manual
      description: Approve the checked build
      evidence: PROOF.md and proof/ files
closure:
  success: Built, checked, and approved by a person. Nothing shipped automatically.
  degraded: The check kept failing and stopped at max_rounds. The last findings are on the check step.
  failed: The work could not be built or checked. Findings are kept in PROGRESS.md.
```
