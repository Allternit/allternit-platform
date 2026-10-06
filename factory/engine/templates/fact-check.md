---
name: Fact check
description: Research a claim, verify each citation supports it, and hold the result for a person to approve before it is used.
---

# Fact check

Every factual or regulated claim is verified and approved by a person
before it ships. A researcher finds sources for the claim. A checker
confirms that each quote actually supports the claim. If it does not, the
claim goes back to research with the checker's findings, up to
`max_rounds` times. A person approves by reading `proof/claims.md`.

Roles come from the team's `team.yaml`: `research` and `check`.

```yaml template-spec
params:
  - name: claim
    description: The claim to check, word for word.
  - name: source
    description: Where to start looking (a URL, a document, or "any").
    default: any
max_rounds: 2
steps:
  - id: research
    title: Find sources for the claim
    description: >-
      Claim: {{ params.claim }}

      Start from: {{ params.source }}. Find primary sources where you can.
      Write proof/claims.md with one row per claim: claim, source link,
      exact quote, and supported / partly supported / not supported. If this
      step was routed back, read the findings appended below first.
    executor: role:research
  - id: verify
    title: Verify the citations
    description: >-
      Read proof/claims.md ({{ research.output_path }}). For each row, open
      the source and confirm the quote is there and supports the claim as
      marked. Fail this step if any row is wrong or unsupported, and say
      which row and why.
    executor: role:check
    blocked_by: [research]
    on_fail: research
  - id: approve
    title: Person approves the claims
    description: >-
      A person reads proof/claims.md and the sources, and approves the
      claims before they are used anywhere.
    blocked_by: [verify]
    wait_gate:
      kind: manual
      description: Approve the checked claims
      evidence: proof/claims.md and the linked sources
closure:
  success: Each claim has a checked source and a person approved it.
  degraded: Verification kept failing and stopped at max_rounds. The open rows are on the verify step.
  failed: The claim could not be supported. Do not use it.
```
