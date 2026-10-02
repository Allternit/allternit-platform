// WP-L2 label harvest: shared types. A source turns local history into
// HarvestItems (a bank's exact live request + the state the caller would have
// sent + a label with provenance). The runner redacts, replays each item through
// the real decision router (so the record carries S1's raw readout, scope and a
// decision_id exactly like a live shadow row) and attaches the label.
import type { Candidate, DecisionOperation } from "../decision/contract.ts";
import type { LabelSource } from "../decision/shadow.ts";

/** A bank's request template, copied verbatim from its live caller. */
export interface BankSpec {
  bank: string;
  /** Ledger primitive_id (x-primitive_id ?? decision_bank_id). */
  primitive_id: string;
  operation: DecisionOperation;
  question_id: string | null;
  instructions: string;
  candidates?: Candidate[];
  criteria?: { true: string; false: string };
  /** x-motif extension when the live caller sets one (e.g. CONFIDENCE_GATE). */
  motif?: string;
  /** Where the live caller lives (documentation + drift checks). */
  caller: string;
}

export interface HarvestItem {
  spec: BankSpec;
  /** Stable key inside its source (e.g. "<session>:<prompt uuid>"): the idempotency key. */
  key: string;
  /** Source family, e.g. "claude-code", "gizzi". */
  source: string;
  /** Historic event time (ISO). */
  ts: string;
  /** Unredacted state text; the runner redacts it before replay or storage. */
  state: string;
  truth: string;
  label_source: LabelSource;
  /** Who established the label, e.g. "replay:claude-code.turn_actions", "human:claude-code.user_rejected". */
  outcome_source: string;
  incumbent?: string | null;
  /** Optional sub-cap bucket (e.g. bypass-mode permission rows), capped separately so rare labels are never crowded out. */
  capGroup?: string;
}

export interface HarvestSource {
  name: string;
  items(): Iterable<HarvestItem> | AsyncIterable<HarvestItem>;
}
