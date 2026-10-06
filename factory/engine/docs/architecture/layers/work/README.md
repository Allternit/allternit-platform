# Work Layer

## Purpose
This layer handles the DAG-centric view of work, where every plan becomes a DAG task or subtask tethered to a canonical `dag_id`. It keeps WIH as the envelope for execution and prevents drift by linking prompts, policy data, and derived views to the graph.

## Key directories & files
- `src/dag/` (if present) or `src/domain/dag.rs` – defines DAG nodes, relationships, and the node/sibling metadata (parent_id, blocked_by, related_to).
- `factory/engine/src/api/cli/rails.rs` – entry point; command parsing for `allternit-factory internal core plan`, `allternit-factory internal core dag`, and `allternit-factory internal core wih`.
- `.allternit/work/dags/<dag_id>/` – derived view snapshots generated after each structural mutation (node create/edit, dependency change).

## Commands
| Command | Role | Notes |
| --- | --- | --- |
| `allternit-factory internal core plan create <description>` | Translates intent to nodes | emits `PromptCreated` + `DagNodeCreated`; anchors prompt ↔ DAG links. |
| `allternit-factory internal core dag node add` | Adds task/subtask | attaches parent/blocking relations and records `MutationProvenance`. |
| `allternit-factory internal core wih pickup <wih_id>` | Claims a WIH | writes `WIHPickedUp`, enforces `Gate` requirements. |
| `allternit-factory internal core wih close <wih_id>` | Initiates close | writes `WIHClosedSigned`, which is the trigger for the autonomous pipeline. |
| `allternit-factory internal core work status` | Observes DAG/WIH | shows the derived WIH view and the Ralph loop state from `.allternit/work`. |

## Invariants
- `dag_id` is the canonical `work_id`; there are no separate ticket IDs. Every WIH/run/transport thread references that `dag_id`.
- Subtasks can be `blocked_by` (hard dependency) or `related_to` (context-sharing) but always trace back to the root DAG node.
- Prompt deltas and graph mutations are append-only events, and each mutation references a prompt/delta or explicit agent decision (enforced by the gate). EOF