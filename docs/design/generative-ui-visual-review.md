# Generative UI visual review

Status: Eoj approved the mixed direction and polish. Shared visual styling and the task, flight and numeric-input interactions are implemented in unreleased source. The independent study remains a design comparison.

Eoj rejected the first visual pass as still generic. This review compares two directions across six representative jobs: records, travel selection, work tracking, calculation, evidence and observation. The current app-renderer capture remains available for comparison.

## Proposed grammar

- Workspace: system type, 13–14px body, 18px headings, aligned records, compact supporting fields, 12px outer radius.
- Editorial: the same controls and palette, more reading space, a 25px serif heading, larger lead results and linear findings.
- Use existing Allternit light/dark tokens. Use color for selection and meaningful state, not a different decorative category theme.
- Remove repeated category ribbons, generic icon tiles, unsupported verified markers and containers that add no grouping.
- Preserve supplied data and semantics: listed transactions are not an account balance, flight selection is not booking, and a source URL is not proof of verification.
- Keep empty/error states specific to the job. Local prototype actions do not change real records.
- Adapt at the card container width. Narrow and coarse-pointer controls need 44px targets and readable input type.

## Coverage proposal

This is a visual-family inventory, not deterministic response routing. Every one of the 82 existing catalog IDs is assigned once.

| Family | Entries |
| --- | ---: |
| Records | 12 |
| Options | 14 |
| Work | 27 |
| Numbers | 10 |
| Evidence | 13 |
| Observation | 6 |

## Entries

| Template | Proposed visual family |
| --- | --- |
| `quick-answer` | Observation |
| `comparison` | Options |
| `decision` | Options |
| `plan` | Work |
| `how-to` | Work |
| `troubleshooting` | Work |
| `status-report` | Work |
| `research-brief` | Evidence |
| `table` | Records |
| `chart` | Numbers |
| `form` | Options |
| `calculator` | Numbers |
| `metric-grid` | Numbers |
| `fact-sheet` | Records |
| `progress-tracker` | Work |
| `timeline` | Work |
| `code-explanation` | Evidence |
| `source-list` | Evidence |
| `image-gallery` | Observation |
| `tabbed-report` | Observation |
| `faq` | Observation |
| `alert` | Observation |
| `option-grid` | Options |
| `evidence-quote` | Evidence |
| `weather` | Observation |
| `stock` | Numbers |
| `task-approval` | Options |
| `itinerary` | Work |
| `flight-options` | Options |
| `boarding-pass` | Work |
| `baggage-tracker` | Work |
| `seat-map` | Options |
| `loyalty-summary` | Numbers |
| `hotel-options` | Options |
| `restaurant-results` | Options |
| `reading-list` | Evidence |
| `transaction-ledger` | Records |
| `spending-breakdown` | Numbers |
| `budget-usage` | Numbers |
| `income-expenses` | Numbers |
| `product-grid` | Records |
| `trading-list` | Records |
| `inventory-risk` | Records |
| `shipment-tracker` | Work |
| `exception-board` | Work |
| `tradeoff-table` | Options |
| `people-directory` | Records |
| `team-breakdown` | Records |
| `executive-brief` | Work |
| `initiative-tracker` | Work |
| `deal-pipeline` | Work |
| `deal-brief` | Work |
| `account-research` | Evidence |
| `followup-review` | Options |
| `sentiment-split` | Evidence |
| `argument-map` | Evidence |
| `social-posts` | Evidence |
| `pdf-analysis` | Evidence |
| `repo-review` | Evidence |
| `agent-roster` | Records |
| `tool-run` | Work |
| `build-report` | Work |
| `deployment-report` | Work |
| `incident-report` | Work |
| `connector-health` | Records |
| `capability-audit` | Work |
| `permissions-review` | Options |
| `invoice-review` | Options |
| `quote-review` | Options |
| `task-board` | Work |
| `workflow-run` | Work |
| `session-handoff` | Work |
| `cost-breakdown` | Numbers |
| `design-review` | Work |
| `media-shotlist` | Work |
| `decision-log` | Work |
| `source-verification` | Evidence |
| `goal-tracker` | Work |
| `knowledge-map` | Evidence |
| `file-browser` | Records |
| `trend-comparison` | Numbers |
| `calendar-agenda` | Records |

## Review evidence

Local study: `/Users/joe/.codex/reviews/generative-ui-template-audit.html`. The independent prototype renders six jobs in two directions with a Current capture option. Functional DOM checks passed for transaction filtering, flight selection, task movement, bill calculation, source disclosure, narrow mode, empty/error states and switching to the current renderer. Safari review inspected Workspace records at wide and 390px widths, Editorial calculation at wide width, and Editorial evidence at narrow width.

## Next implementation boundary

Choose the direction before replacing more product views. Preserve existing source enforcement work and cover Desktop, web phone layout and PWA together. Mandatory template selection, final-response validation and runtime usage evidence remain unfinished work. No commits, pushes or deployments were made for this design review.

## Polish standard

Use the existing app controls and tokens. Acceptance covers type hierarchy and numeric formatting, spacing and overflow, default/hover/pressed/selected/focus/disabled states, inline validation, preserved input and keyboard focus, reversible feedback, stable loading/streaming, narrow cards, dark mode, zoom and touch targets. The same standard applies to every catalog family and all 82 entries.

The review artifact now includes local action/Undo, keyboard-focus, selected/disabled and inline-validation examples. Their DOM checks passed. Motion remains a specification: inspect normal, interrupted, reduced-motion and phone frame strips during implementation before accepting it. No motion completion is claimed by these examples.

## Shared source polish pass

The accepted visual grammar now reaches all 82 templates through typed application-owned family metadata. The shared root no longer renders category ribbons. Styles remove blanket tints, decorative product/book artwork, extra shadows and long entrance animations. Focus, disabled controls, narrow layouts and numeric alignment have shared rules. The source remains uncommitted and undeployed.

Verification: 201 existing UI checks passed across 15 files; the presentation modules passed focused strict type checks; all 82 actual-renderer previews exported successfully; six generated contract files match. The standalone prototype checks cover task movement/completion/Undo, explicit flight selection, inline calculator errors including step/range violations, preserved drafts and focus, source disclosure, loading and recovery states. Fresh native-browser visual review was blocked by blank Chrome rendering and Safari timeouts; it is not recorded as passed for this revision.

The prototype interactions remain independent of product action/persistence contracts. No product-level Undo, booking or new approval behavior is claimed by the shared styling pass. Mandatory template selection and runtime response enforcement remain unfinished.


## Approved interaction integration

Eoj approved the preview. The shared renderer now includes task movement/completion with a bounded local Undo history and restored focus; explicit flight selection through the existing chat callback, pending-send protection and retry; numeric drafts with range/step/finite validation, result suppression and disabled invalid form submission. Current task state and flight selection use existing card persistence and survive reload. No connector mutation or booking action was added.

Verification: the focused UI suite passed 206 checks across 15 files before the additional flight-recovery case; the all-82 exporter also passed. The flight-recovery follow-up passed 14 public-renderer interaction checks. The scoped strict type check passed. Native-browser visual verification remains open, as recorded above. Mandatory runtime selection is the next implementation gap. Approval did not authorize a commit, push or deployment.


## Required response policy implementation

Source now includes the shared answer-policy module and a central Gizzi turn guard. Explicit requests cover all 82 IDs; fixed domain display-name and alias routes select recognized jobs before generation. Required replies stay private until strict envelope and selected-ID validation pass. One formatting correction is bounded; a second failure publishes a readable error. Interrupted turns do not retry. Durable `answer.validation` records preserve the choice and outcome, with bounded diagnostic captures for rejected responses.

Scope is visual-capable Gizzi app turns, direct or API-bridged. Conversation, Text only, artifact/JSON-schema outputs and external paths without this capability are explicit exceptions. Source is unreleased. Runtime tests establish invocation and failure handling; they do not establish production adoption or universal vendor support.


Runtime verification completed: 14 public SessionPrompt scenarios passed against a local vendor API fixture with the real event bus and persistence. The five guardrail, four existing refusal-handoff and three usage-limit compatibility checks passed in separate processes. Shared routing checks passed 156 cases, including all 82 explicit IDs and all 70 domain display-name routes. The earlier focused UI suite passed 293 cases; the extra 70 route cases were added afterward. The shared policy passed its strict focused type check. No production usage or signed-in cross-device pass is claimed.

The implementation also prevents delayed event references from exposing a held text buffer, retains the original policy across synthetic hook continuations, respects quota stops before correction, and clears stale partial output on transport retries. Both selected-policy and validation events are durable. Release verification and external/vendor paths without interactive-answer capability remain outside this completed source stage.


## Release preparation — 2026-11-02

Integrated the reviewed source onto UI main `c8198aeb280ca19af0d95d40d146cbe9277cd8bd` and platform main `c3a88eb9ae6e6913691f1581e9ab4f7f32a0d8c9`. Resolved the runtime-links conflict by retaining current Office records and adding the answer-policy edge. The fresh trees are `allternit-ai-genui-release-20261102` and `allternit-genui-release-20261102`, branch `session/genui-release-20261102`. Factory tracking is `dag:dag_491381` (`wih:wih_6268`, verification `wih:wih_8628`).

The initial shared dependency links were stale. A frozen, script-free pnpm install in the owned UI tree restored the declared packages and the full pinned typecheck passed. The Desktop release preflight misread the existing four-variable Apple/keychain destructuring; its credential-presence pattern now accepts the same required three keys with additional variables. No notarization policy changed. The preflight passes 58 checks. Publication still requires the exact deployment preview and subsequent approval.

## Released and verified — 2026-11-02

Eoj approved the coordinated release after the command preview. UI PR #937, platform PR #2043 and the runtime metadata type fix #2044 merged. The released UI is `1df83e37de4d6c5afaf758c920e0152a405360ff`; the runtime source is `0f563cce31e4e8c272800f1f040fdcc631fd6677`.

- Workspace, bots and phone PWA deployments serve the merged UI; their public roots and app entry scripts return HTTP 200. The phone source receipt matches the UI commit.
- Docs and both API deployment workflows succeeded. The API suite passed 2,957 tests, with 12 existing ignored cases; its fresh-database boot check and deployed health check passed.
- Gizzi 2.2.16 was published for all five platforms by workflow `41407117304`. Homebrew tap PR #4 carries its verified platform hashes.
- Signed runtime feeds are `r20261102.0733-0f563cc-770` (Darwin arm64) and `r20261102.0733-0f563cc-961` (Linux x64). Both signatures, source commits, manifest hashes and required objects were verified. Both contain the same UI commit.
- Each compiled Gizzi runtime passed correction with tools disabled, terminal validation failure, and Text only against a local vendor fixture. Both compiled APIs booted on isolated fresh databases. The Mac applied the new runtime through a normal restart and confirmed healthy; its served UI receipt matches the release.

The UI suite passed 444 tests with clean dependencies; the full UI and runtime typechecks passed. Fourteen real-runtime enforcement scenarios passed in CI with production feature flags. Browser automation was unavailable and native accessibility inspection timed out after relaunch, so a fresh visual pass across signed-in Desktop and phone remains unverified. Unsupported external paths remain outside the required-card policy.

The local Mac Factory build encountered an Xcode/Zig compatibility failure. Its matching Factory binary came from the successful official release build at the same source commit. Local Homebrew CLI activation remains blocked by its outdated Command Line Tools check; the installed Desktop runtime is already updated independently.
