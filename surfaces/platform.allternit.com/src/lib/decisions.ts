/** Types and display helpers for `/v1/decisions`. */

export type OutcomeStatus = "success" | "failure" | "error" | "skipped";
export const OUTCOME_STATUSES: OutcomeStatus[] = ["success", "failure", "error", "skipped"];

export interface DecisionOutcome {
  status: OutcomeStatus;
  label?: string;
  detail?: string;
}

export interface DecisionProbability {
  value: string | number;
  label?: string;
  probability: number;
}

export interface DecisionAnswer {
  name: string;
  type: "predicate" | "choice" | "score" | "refusal" | string;
  probability?: number;
  choice?: string;
  score?: number;
  confidence?: number;
  probabilities?: DecisionProbability[];
  backend?: string;
  /** Present on refusals: abstained | unavailable | budget_exhausted. */
  reason?: string;
}

export interface V1Decision {
  id: string;
  object: "decision";
  created_at: string;
  backend?: string;
  latency_ms?: number;
  metadata?: Record<string, unknown>;
  outcomes?: Record<string, DecisionOutcome>;
  usage?: { input_tokens?: number; questions?: number; [k: string]: unknown };
  answers: DecisionAnswer[];
}

export const REFUSAL_LABELS: Record<string, string> = {
  abstained: "Abstained",
  unavailable: "Unavailable",
  budget_exhausted: "Budget exhausted",
};

export function formatPct(p: number): string {
  const v = p * 100;
  const r = Math.round(v * 10) / 10;
  return `${Number.isInteger(r) ? r.toFixed(0) : r.toFixed(1)}%`;
}

/** The one-line top answer: "yes 92%", "billing 99.8%", "high · 0.98", or the refusal. */
export function topAnswer(a: DecisionAnswer): string {
  switch (a.type) {
    case "predicate": {
      if (typeof a.probability !== "number") return "—";
      return a.probability >= 0.5 ? `yes ${formatPct(a.probability)}` : `no ${formatPct(1 - a.probability)}`;
    }
    case "choice": {
      if (!a.choice) return "—";
      const top = a.probabilities?.find((p) => String(p.value) === a.choice)?.probability ?? a.confidence;
      return typeof top === "number" ? `${a.choice} ${formatPct(top)}` : a.choice;
    }
    case "score": {
      if (typeof a.score !== "number") return "—";
      const pr = a.probabilities ?? [];
      const best = pr.reduce<DecisionProbability | null>((m, p) => (!m || p.probability > m.probability ? p : m), null);
      const label = best?.label;
      const s = Number.isInteger(a.score) ? String(a.score) : a.score.toFixed(2);
      return label ? `${label} · ${s}` : s;
    }
    case "refusal":
      return REFUSAL_LABELS[a.reason ?? ""] ?? a.reason ?? "Refused";
    default:
      return "—";
  }
}

export function hasOutcomes(d: Pick<V1Decision, "outcomes">): boolean {
  return Object.keys(d.outcomes ?? {}).length > 0;
}
