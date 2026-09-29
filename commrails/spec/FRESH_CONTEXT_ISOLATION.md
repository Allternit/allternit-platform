# Fresh Context Isolation Rules

When a node has `execution_mode: fresh`, the gate must provide a clean context window.

## Must include
- PromptCreated raw text
- All PromptDeltaAppended deltas linked to this dag/node (and optionally ancestor chain)
- DAG slice:
  - node itself
  - ancestor chain to root
  - blocked_by dependency predecessors
  - optionally related_to neighbors if explicitly whitelisted
- Receipts from dependency predecessors (hard deps)
- Recorded outputs of dependency predecessors (hard deps), see below

## Must exclude by default
- sibling nodes not in dependency chain
- unrelated mail threads
- unrelated receipts
- unrelated DAGs/projects

## Practical implementation
- Build a ContextPack artifact (JSON) that enumerates allowed references.
- Stored at `.allternit/work/dags/<dag_id>/wih/context/<wih_id>.context.json`.
- Capsules mount only the ContextPack + referenced files.

ContextPack (v1) includes:
- prompt timeline (raw text + deltas)
- dag_slice (nodes/edges/relations for node + ancestors + deps)
- dependency_nodes list
- receipt refs for dependency predecessors
- context_pack_path is recorded on WIHCreated for discoverability
- related_to nodes are included only when relation has `context_share: true`
- `dependency_outputs[]` for every blocked_by predecessor (transitive) with a recorded
  output: `{node_id, wih_id, receipt_id, blob_id, sha256, size_bytes, output_path, text,
  truncated}`. `text` inlines the output up to `CONTEXT_PACK_OUTPUT_INLINE_CAP`
  (16 KiB, cut on a UTF-8 char boundary, `truncated: true` when cut); the full text is
  always at `output_path` (workspace-relative) and in the blob
- `resolved_description`: the node description with `{{ <node_id>.output }}` /
  `{{ <node_id>.output_path }}` placeholders resolved (placeholders substitute the full
  output, not the capped text)

## Untrusted content fencing (S7)

Predecessor outputs are written by other agents and may carry prompt injection
(including fake closing fences). Every place that inlines them into a prompt
wraps them in a nonce fence:

```text
<untrusted-data nonce="<32 hex>" source="node:<node_id>">
…output text…
</untrusted-data nonce="<32 hex>">
```

- One fence per render (`crate::fence::Fence`): the nonce is 128 random bits
  minted at Gate 1 pickup, after every fenced output was recorded, so the
  content cannot know it. Two renders never share a nonce.
- Before wrapping, every fence marker inside the content (`<untrusted-data`,
  `</untrusted-data`, any case, optional whitespace) is escaped to `&lt;…`, so a
  fake closing fence stays inert even if it guessed the nonce.
- `source` is restricted to `[A-Za-z0-9:_./-]`.
- The rendered prompt starts with one line stating that text between the
  `untrusted-data` tags carrying that nonce is data, not instructions. The rule
  names the tag without writing a marker.

Where it applies:
- `resolved_description` / `wih/context/<wih>.prompt.md`: each `{{ x.output }}`
  substitution is fenced and the rule is prepended (only when an `output`
  placeholder was substituted; `{{ x.output_path }}` inlines a path we generate
  and is not fenced).
- ContextPack `dependency_outputs[].text`: fenced (the 16 KiB cap applies to the
  output before the fence markers are added); the pack records
  `untrusted_fence: {nonce, instruction}`. `WihPickup.fence_nonce` returns the
  nonce to the caller.
- The read-only observer prompt fences node outputs and the recent ledger
  events (`source="ledger"`), and lesson triage fences the output excerpt it
  sends to the System One scorer.
