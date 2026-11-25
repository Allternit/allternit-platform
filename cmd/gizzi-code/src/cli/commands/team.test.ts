import { describe, expect, test } from "bun:test"
import { formatRoster, formatTeamMap, teamErrorMessage, type TeamMap } from "./team-format"

describe("formatRoster", () => {
  test("empty roster explains how to get teammates", () => {
    expect(formatRoster({ people: [] })[0]).toContain("Invite teammates")
  })
  test("people → machines → sessions with doing line", () => {
    const out = formatRoster({
      people: [{ display: "Eoj", me: true, unread: 2, held: 0, machines: [{ name: "MacBook", online: true, bots: [], sessions: [{ address: "eoj/a-claude", harness: "claude", status: "busy", doing: "fixing tests", tier: "push" }] }] }],
    }).join("\n")
    expect(out).toContain("Eoj (you)  [2 unread]")
    expect(out).toContain("● MacBook")
    expect(out).toContain("eoj/a-claude  claude  busy  push")
    expect(out).toContain("fixing tests")
  })
})

// Copy of handoffs/island-bar-team-map-2026-11-25/fixtures/team-map.json (TEAM_MAP_FIXTURE).
const fixture = {
 "org": {
  "id": "org_acme",
  "name": "Acme Coffee"
 },
 "me": "p:usr_maya",
 "nodes": [
  {
   "id": "p:usr_maya",
   "kind": "person",
   "name": "Maya",
   "avatar": null,
   "role": "owner",
   "is_main": null,
   "computer": null,
   "state": "idle",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  },
  {
   "id": "p:usr_noah",
   "kind": "person",
   "name": "Noah",
   "avatar": null,
   "role": "admin",
   "is_main": null,
   "computer": null,
   "state": "offline",
   "counts": {
    "asks": 1,
    "jobs": 0
   }
  },
  {
   "id": "p:usr_priya",
   "kind": "person",
   "name": "Priya",
   "avatar": null,
   "role": "member",
   "is_main": null,
   "computer": null,
   "state": "offline",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  },
  {
   "id": "b:gizzi",
   "kind": "bot",
   "name": "Gizzi",
   "avatar": null,
   "role": null,
   "is_main": true,
   "computer": null,
   "state": "working",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  },
  {
   "id": "b:triage",
   "kind": "bot",
   "name": "Triage",
   "avatar": null,
   "role": null,
   "is_main": false,
   "computer": null,
   "state": "idle",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  },
  {
   "id": "b:refunds",
   "kind": "bot",
   "name": "Refunds bot",
   "avatar": null,
   "role": null,
   "is_main": false,
   "computer": null,
   "state": "waiting_approval",
   "counts": {
    "asks": 1,
    "jobs": 1
   }
  },
  {
   "id": "b:code_review",
   "kind": "bot",
   "name": "Code review",
   "avatar": null,
   "role": null,
   "is_main": false,
   "computer": null,
   "state": "working",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  },
  {
   "id": "b:test_runner",
   "kind": "bot",
   "name": "Test runner",
   "avatar": null,
   "role": null,
   "is_main": false,
   "computer": null,
   "state": "idle",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  },
  {
   "id": "b:docs_writer",
   "kind": "bot",
   "name": "Docs writer",
   "avatar": null,
   "role": null,
   "is_main": false,
   "computer": null,
   "state": "needs_input",
   "counts": {
    "asks": 0,
    "jobs": 0
   }
  }
 ],
 "edges": [
  {
   "from": "p:usr_noah",
   "to": "p:usr_maya",
   "kind": "reports_to"
  },
  {
   "from": "p:usr_priya",
   "to": "p:usr_maya",
   "kind": "reports_to"
  },
  {
   "from": "p:usr_maya",
   "to": "b:gizzi",
   "kind": "owns"
  },
  {
   "from": "p:usr_maya",
   "to": "b:triage",
   "kind": "owns"
  },
  {
   "from": "p:usr_maya",
   "to": "b:refunds",
   "kind": "owns"
  },
  {
   "from": "b:triage",
   "to": "b:refunds",
   "kind": "parent"
  },
  {
   "from": "p:usr_priya",
   "to": "b:code_review",
   "kind": "owns"
  },
  {
   "from": "p:usr_priya",
   "to": "b:test_runner",
   "kind": "owns"
  },
  {
   "from": "b:code_review",
   "to": "b:test_runner",
   "kind": "parent"
  },
  {
   "from": "p:usr_noah",
   "to": "b:docs_writer",
   "kind": "owns"
  }
 ],
 "grants": [
  {
   "id": "bag_1",
   "bot": "b:refunds",
   "app": {
    "kind": "connector",
    "ref": "mcp_stripe",
    "name": "Stripe",
    "icon": null
   },
   "level": "app",
   "ceiling": "full",
   "granted_by": "usr_maya",
   "created_at": "2026-11-25T09:00:00Z"
  },
  {
   "id": "bag_2",
   "bot": "b:triage",
   "app": {
    "kind": "connector",
    "ref": "mcp_stripe",
    "name": "Stripe",
    "icon": null
   },
   "level": "browser",
   "ceiling": "full",
   "granted_by": "usr_maya",
   "created_at": "2026-11-25T09:00:00Z"
  },
  {
   "id": "bag_3",
   "bot": "b:gizzi",
   "app": {
    "kind": "connector",
    "ref": "mcp_stripe",
    "name": "Stripe",
    "icon": null
   },
   "level": "full",
   "ceiling": "full",
   "granted_by": "usr_maya",
   "created_at": "2026-11-25T09:00:00Z"
  },
  {
   "id": "bag_4",
   "bot": "b:code_review",
   "app": {
    "kind": "connector",
    "ref": "mcp_github",
    "name": "GitHub",
    "icon": null
   },
   "level": "app",
   "ceiling": "full",
   "granted_by": "usr_maya",
   "created_at": "2026-11-25T09:00:00Z"
  },
  {
   "id": "bag_5",
   "bot": "b:test_runner",
   "app": {
    "kind": "connector",
    "ref": "mcp_github",
    "name": "GitHub",
    "icon": null
   },
   "level": "browser",
   "ceiling": "full",
   "granted_by": "usr_maya",
   "created_at": "2026-11-25T09:00:00Z"
  },
  {
   "id": "bag_6",
   "bot": "b:docs_writer",
   "app": {
    "kind": "vendor",
    "ref": "notion",
    "name": "Notion",
    "icon": null
   },
   "level": "app",
   "ceiling": "app",
   "granted_by": "usr_maya",
   "created_at": "2026-11-25T09:00:00Z"
  }
 ],
 "rules": [
  {
   "id": "arl_refunds_250",
   "text": "anyone needs Noah to approve refund_payment in Stripe when amount is over 250 EUR.",
   "enabled": true,
   "scope": {
    "kind": "org",
    "ref": null
   },
   "approver": {
    "kind": "user",
    "user_id": "usr_noah"
   }
  }
 ],
 "asks": [
  {
   "id": "cowork:apr_refund_1",
   "store": "cowork",
   "title": "Refund \u20ac320 on order 1042",
   "summary": "Customer says the grinder arrived broken.",
   "actor": "b:refunds",
   "approver_user_id": "usr_noah",
   "rule_id": "arl_refunds_250",
   "app": null,
   "tool": "refund_payment",
   "risk": "normal",
   "state": "pending",
   "created_at": "2026-11-25T09:12:00Z",
   "expires_at": null,
   "job_id": "bjb_refund_1",
   "resolve": {
    "method": "POST",
    "path": "/api/v1/cowork/approvals",
    "body": {
     "actionId": "apr_refund_1",
     "decision": "approve"
    }
   }
  }
 ],
 "jobs": [
  {
   "job_id": "bjb_refund_1",
   "actor": "b:refunds",
   "waiting_on": "p:usr_noah",
   "app": {
    "kind": "connector",
    "ref": "mcp_stripe",
    "name": "Stripe",
    "icon": null
   },
   "tool": "refund_payment",
   "title": "Refund payment",
   "state": "waiting_approval"
  }
 ],
 "partial": []
}

describe("formatTeamMap (fixture)", () => {
  const out = formatTeamMap(fixture as unknown as TeamMap)
  const text = out.join("\n")
  test("tree order: Maya, her bots, then reports", () => {
    const at = (s: string) => out.findIndex((l) => l.includes(s))
    expect(at("Maya (you)")).toBeLessThan(at("Gizzi"))
    expect(at("Triage")).toBeLessThan(at("Refunds bot"))
    expect(out[at("Refunds bot")]).toMatch(/^ {4}/) // sub-bot indented under Triage
    expect(at("Refunds bot")).toBeLessThan(at("Noah"))
    expect(at("Noah")).toBeLessThan(at("Docs writer"))
  })
  test("ask marker with rule text", () => {
    expect(text).toContain("⚑ Refunds bot asks: anyone needs Noah to approve refund_payment in Stripe when amount is over 250 EUR.")
  })
  test("waiting marker and app level", () => {
    expect(text).toContain("→ waiting on Noah")
    expect(text).toContain("Stripe (app)")
  })
  test("error wording", () => {
    expect(teamErrorMessage(409, "no_runtime", "")).toContain("No computer")
    expect(teamErrorMessage(422, "above_ceiling", "")).toContain("above")
    expect(teamErrorMessage(503, "tier_check_unavailable", "")).toContain("Could not check")
  })
})
