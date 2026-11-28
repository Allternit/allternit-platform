import type { ApprovalSource, ApprovalStatus, V1Approval, V1KernelApproval } from "@/lib/platform-v1";

export const APPROVAL_STATUS_LABEL: Record<ApprovalStatus, string> = {
  pending: "Pending",
  approved: "Approved",
  denied: "Denied",
  expired: "Expired",
  consumed: "Consumed",
};

export const APPROVAL_SOURCE_LABEL: Record<ApprovalSource, string> = {
  twin: "Twin",
  computer: "Computer",
  kernel: "Kernel",
};

export function approvalStatusLabel(status: string): string {
  return APPROVAL_STATUS_LABEL[status as ApprovalStatus] ?? status;
}

function scalar(v: unknown): string | null {
  if (typeof v === "string" && v.trim()) return v.trim();
  if (typeof v === "number" || typeof v === "boolean") return String(v);
  return null;
}

/** One line describing what the agent wants to do, from its free-form `proposed_action`. */
export function summarizeProposedAction(action: V1Approval["proposed_action"] | null | undefined, max = 120): string {
  if (!action || typeof action !== "object") return "No details";
  const a = action as Record<string, unknown>;
  const head = ["summary", "description", "title", "message", "command", "prompt", "action", "tool", "name"]
    .map((k) => scalar(a[k]))
    .find((x) => x !== null);
  let text = head;
  if (!text) {
    const parts = Object.entries(a)
      .map(([k, v]) => {
        const s = scalar(v);
        return s ? `${k}: ${s}` : null;
      })
      .filter((x): x is string => x !== null);
    text = parts.length ? parts.join(", ") : Object.keys(a).length ? JSON.stringify(a) : "No details";
  }
  text = text.replace(/\s+/g, " ");
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

/** Kernel approvals are decided by the owner on the Allternit app or phone, never from the console. */
export const KERNEL_APPROVE_ELSEWHERE = "Approve on the Allternit app or phone";

export function isKernelApproval(a: Pick<V1Approval, "source">): boolean {
  return a.source === "kernel";
}

/** First 12 characters of an action hash, for display; the full hash is what gets copied. */
export function shortActionHash(hash: string | null | undefined): string {
  const h = (hash ?? "").replace(/^sha256:/, "");
  return h.length > 12 ? h.slice(0, 12) : h || "—";
}

/** `Expired` once the deadline passed on a still-pending row, else null. */
export function kernelDeadlineState(k: Pick<V1KernelApproval, "deadline">, status: string, now: number = Date.now()): "expired" | "open" | "none" {
  if (!k.deadline) return "none";
  const at = Date.parse(k.deadline);
  if (Number.isNaN(at)) return "none";
  return status === "expired" || (status === "pending" && at <= now) ? "expired" : "open";
}

/** The console never offers Approve/Deny on a kernel row. */
export function canDecideInConsole(a: Pick<V1Approval, "source" | "status">): boolean {
  return a.status === "pending" && !isKernelApproval(a);
}
