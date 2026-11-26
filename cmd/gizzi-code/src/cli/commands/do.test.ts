import { describe, expect, test } from "bun:test"
import { formatReview, needsOwnerApproval, statusLine, receipt, type Review, type BarJob } from "./do-format"

// Trimmed from BAR_FIXTURES (handoffs/island-bar-team-map-2026-11-25/fixtures/bar.json).
const review: Review = {
  job_id: "bjb_refund_1",
  state: "review",
  level: "app",
  needs_confirmation: false,
  rule: { id: "arl_refunds_250", text: "'refund_payment' in Stripe when amount is over 250 EUR needs approval from a named person." },
  approver: { user_id: "usr_noah", name: "Noah", is_you: false },
  lines: [
    { key: "order_id", label: "Order", value: "1042" },
    { key: "amount", label: "Amount", value: "320" },
  ],
}
const waiting: BarJob = { id: "bjb_refund_1", tool: "refund_payment", title: "Refund payment", state: "waiting_approval", args: { amount: 320 } }
const done: BarJob = {
  ...waiting,
  state: "completed",
  result: { structuredContent: { refund_id: "re_3PqX", amount: 320, currency: "eur" } },
}

describe("do-format", () => {
  test("review prints level, rule text and approver; owner rule detected", () => {
    const out = formatReview(review).join("\n")
    expect(out).toContain("Level: app")
    expect(out).toContain(review.rule!.text)
    expect(out).toContain("Approver: Noah")
    expect(needsOwnerApproval(review)).toBe(true)
  })
  test("status line and receipt follow the contract wording", () => {
    expect(statusLine(waiting, "Noah", 0)).toBe("● Waiting on Noah ▰▱▱▱")
    expect(statusLine({ ...waiting, state: "working" }, null, 1)).toBe("● Working ▰▰▱▱")
    expect(statusLine({ ...waiting, state: "input_required" })).toBe("● Needs input")
    expect(receipt(done)).toBe("Refunded €320")
  })
})
