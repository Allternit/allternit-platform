import Foundation

/// Meetings answers for `-sample-workspace` (DEBUG screenshots and UI
/// checks): a live standup with transcript, artifacts and an approval, an
/// agenda for today and tomorrow, and two past calls (a recap and a "not let
/// in"). Shapes follow CONTRACT.md §4. `-sample-no-calendar` answers no
/// calendar connections, so the connect prompts show.
enum SampleMeetings {
    private static let liveId = "sample-meeting-standup"
    private static let liveSession = "sample-session-standup"
    private static let recapId = "sample-meeting-pricing"
    private static let recapSession = "sample-session-pricing"

    static func answer(method: String, parts: [String], query: [String: String], body: Data?) -> SampleWorkspaceAPI.Answer? {
        guard let first = parts.first else { return nil }
        let now = Date()
        switch (method, first) {
        case ("GET", "meetings") where parts.count == 1:
            switch query["range"] {
            case "now": return SampleWorkspaceAPI.json(["meetings": [live(now)], "next_cursor": NSNull()])
            case "past": return SampleWorkspaceAPI.json(["meetings": past(now), "next_cursor": NSNull()])
            default: return SampleWorkspaceAPI.json(["meetings": [live(now)] + upcoming(now), "next_cursor": NSNull()])
            }
        case ("GET", "meetings"):
            let all = [live(now)] + upcoming(now) + past(now)
            guard let row = all.first(where: { $0["id"] as? String == parts[1] }) else { return SampleWorkspaceAPI.json(["error": "meeting_not_found", "message": "Not found"], status: 404) }
            return SampleWorkspaceAPI.json(row)
        case ("POST", "meetings") where parts.count == 1:
            let payload = body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
            var row = meeting(id: "sample-meeting-pasted", title: "", platform: "meet", startsAt: nil, gizzi: "off", people: "")
            row["join_url"] = payload["url"] as? String ?? ""
            row["source"] = "link"
            return SampleWorkspaceAPI.json(row, status: 201)
        case ("PATCH", "meetings"):
            let payload = body.flatMap { try? JSONSerialization.jsonObject(with: $0) as? [String: Any] } ?? [:]
            guard parts.count >= 2, var row = upcoming(now).first(where: { $0["id"] as? String == parts[1] }) else { return nil }
            if let on = payload["auto_join"] as? Bool { row["auto_join"] = on; row["gizzi"] = on ? "scheduled" : "off" }
            return SampleWorkspaceAPI.json(row)
        case ("POST", "meetings") where parts.count == 3 && parts[2] == "sessions":
            return SampleWorkspaceAPI.json(session(id: liveSession, meetingId: liveId, now: now), status: 201)
        case (_, "meeting-sessions") where parts.count >= 2:
            return sessionRoute(method: method, sid: parts[1], rest: Array(parts.dropFirst(2)), now: now)
        case (_, "me") where parts.count == 2 && parts[1] == "meeting-settings":
            return SampleWorkspaceAPI.json(["auto_join": "all_except_1on1", "recap_email": true, "default_reply_mode": "chat"])
        case ("GET", "me") where parts.count == 2 && parts[1] == "meeting-usage":
            return SampleWorkspaceAPI.json(["period_start": iso(now.addingTimeInterval(-12 * 86400)), "period_end": iso(now.addingTimeInterval(18 * 86400)),
                                            "used_minutes": 214, "included_minutes": 600, "overage_minutes": 0, "concurrent_cap": 2, "plan": "plus"])
        case ("GET", "zoom") where parts == ["zoom", "connection"]:
            return SampleWorkspaceAPI.json(["connected": false, "status": NSNull(), "connected_at": NSNull()])
        case ("GET", "calendar") where parts == ["calendar", "connections"]:
            if launchArgumentEnabled("sample-no-calendar") { return SampleWorkspaceAPI.json(["connections": [Any]()]) }
            return SampleWorkspaceAPI.json(["connections": [[
                "id": "sample-cal-google", "provider": "google", "account_email": SampleWorkspace.userEmail,
                "status": "active", "scopes": [String](), "last_synced_at": iso(now.addingTimeInterval(-300)), "created_at": iso(now.addingTimeInterval(-20 * 86400)),
            ]]])
        default:
            return nil
        }
    }

    private static func sessionRoute(method: String, sid: String, rest: [String], now: Date) -> SampleWorkspaceAPI.Answer? {
        let isRecap = sid == recapSession
        let meetingId = isRecap ? recapId : liveId
        switch (method, rest.first) {
        case ("GET", nil):
            let meetingRow = isRecap ? past(now)[0] : live(now)
            return SampleWorkspaceAPI.json([
                "session": session(id: sid, meetingId: meetingId, now: now),
                "meeting": meetingRow,
                "participants": [
                    participant("p1", "Ana Ruiz", now), participant("p2", "Marco Chen", now), participant("p3", "Priya Shah", now),
                ],
                "pending_approvals": isRecap ? [Any]() : [approval(now)],
            ])
        case ("GET", "transcript"):
            return SampleWorkspaceAPI.json(["segments": transcript(sid), "next_after_seq": NSNull()])
        case ("GET", "artifacts"):
            return SampleWorkspaceAPI.json(["artifacts": [
                artifact("a1", sid, "doc", "Launch checklist", 3, now),
                artifact("a2", sid, "slides", "Beta pricing options", 1, now),
            ]])
        case ("GET", "events"):
            return SampleWorkspaceAPI.json(["error": "preview"], status: 404)
        case ("POST", "approvals"):
            var a = approval(now)
            a["status"] = "approved"
            return SampleWorkspaceAPI.json(a)
        case ("POST", "control"), ("PATCH", nil):
            return SampleWorkspaceAPI.json(session(id: sid, meetingId: meetingId, now: now))
        default:
            return nil
        }
    }

    // MARK: - Rows

    private static func iso(_ d: Date) -> String { MeetingDates.iso(d) }

    private static func meeting(id: String, title: String, platform: String, startsAt: Date?, gizzi: String, people: String,
                                attendees: Int = 5, outside: Int = 0, oneOnOne: Bool = false, autoJoin: Bool? = nil, session: String? = nil) -> [String: Any] {
        let label = ["meet": "Google Meet", "zoom": "Zoom", "teams": "Microsoft Teams"][platform] ?? "Call"
        return [
            "id": id, "user_id": SampleWorkspace.userId, "org_id": NSNull(), "platform": platform, "platform_label": label,
            "title": title, "join_url": "https://meet.google.com/abc-defg-hij", "url_key": "meet:abc-defg-hij",
            "starts_at": startsAt.map(iso) ?? NSNull(), "ends_at": startsAt.map { iso($0.addingTimeInterval(1800)) } ?? NSNull(),
            "source": "calendar_google", "calendar_connection_id": "sample-cal-google", "external_event_id": NSNull(),
            "attendee_count": attendees, "external_attendee_count": outside, "is_one_on_one": oneOnOne,
            "auto_join": autoJoin.map { $0 as Any } ?? NSNull(), "reply_mode": "chat", "gizzi": gizzi, "people_label": people,
            "latest_session_id": session.map { $0 as Any } ?? NSNull(),
            "live_at": gizzi == "live" ? iso(Date().addingTimeInterval(-12 * 60)) as Any : NSNull(),
            "page_url": "https://ai.allternit.com/meetings/\(id)", "created_at": iso(Date()), "updated_at": iso(Date()),
        ]
    }

    private static func live(_ now: Date) -> [String: Any] {
        meeting(id: liveId, title: "Weekly standup", platform: "meet", startsAt: now.addingTimeInterval(-13 * 60),
                gizzi: "live", people: "6 people", attendees: 6, session: liveSession)
    }

    private static func upcoming(_ now: Date) -> [[String: Any]] {
        let cal = Calendar.current
        let tomorrow10 = cal.date(bySettingHour: 10, minute: 0, second: 0, of: cal.date(byAdding: .day, value: 1, to: now) ?? now) ?? now
        func today(_ minutes: Double) -> Date {
            let t = now.addingTimeInterval(minutes * 60)
            return cal.isDate(t, inSameDayAs: now) ? t : cal.date(bySettingHour: 23, minute: 30, second: 0, of: now) ?? t
        }
        return [
            meeting(id: "sample-meeting-design", title: "Design review", platform: "meet", startsAt: today(45), gizzi: "scheduled", people: "5 people"),
            meeting(id: "sample-meeting-ana", title: "1:1 with Ana", platform: "zoom", startsAt: today(90), gizzi: "off", people: "2 people",
                    attendees: 2, oneOnOne: true),
            meeting(id: "sample-meeting-northwind", title: "Northwind kickoff", platform: "teams", startsAt: today(150), gizzi: "scheduled",
                    people: "8 people, 3 outside", attendees: 8, outside: 3),
            meeting(id: "sample-meeting-board", title: "Board prep", platform: "meet", startsAt: tomorrow10, gizzi: "scheduled", people: "4 people", attendees: 4),
        ]
    }

    private static func past(_ now: Date) -> [[String: Any]] {
        let yesterday = Calendar.current.date(byAdding: .day, value: -1, to: now) ?? now
        return [
            meeting(id: recapId, title: "Pricing sync", platform: "meet", startsAt: yesterday, gizzi: "ended", people: "4 people",
                    attendees: 4, session: recapSession),
            meeting(id: "sample-meeting-vendor", title: "Vendor call", platform: "zoom", startsAt: yesterday.addingTimeInterval(-3 * 3600),
                    gizzi: "not_admitted", people: "3 people, 2 outside", attendees: 3, outside: 2, session: "sample-session-vendor"),
        ]
    }

    private static func session(id: String, meetingId: String, now: Date) -> [String: Any] {
        let isRecap = id == recapSession
        let isVendor = id == "sample-session-vendor"
        let liveAt = isRecap ? now.addingTimeInterval(-86400) : now.addingTimeInterval(-12 * 60)
        var s: [String: Any] = [
            "id": id, "meeting_id": meetingId, "state": isRecap || isVendor ? "ended" : "live",
            "gizzi": isVendor ? "not_admitted" : (isRecap ? "ended" : "live"),
            "end_reason": isVendor ? "not_admitted" as Any : (isRecap ? "left_by_user" as Any : NSNull()),
            "platform": isVendor ? "zoom" : "meet", "speak_replies": false, "listening": true, "bot_identity": "Gizzi (Allternit)",
            "requested_by": SampleWorkspace.userId, "scheduled_join_at": NSNull(), "joined_at": iso(liveAt.addingTimeInterval(-40)),
            "live_at": isVendor ? NSNull() as Any : iso(liveAt) as Any, "ended_at": isRecap ? iso(liveAt.addingTimeInterval(38 * 60)) as Any : NSNull(),
            "summary": NSNull(), "decisions": [String](), "action_items": [Any](),
            "created_at": iso(liveAt), "updated_at": iso(now),
        ]
        if isRecap {
            s["summary"] = "The team agreed to keep the Plus plan at $20 and add a yearly option. Beta pricing goes live with the Friday release, and Ana will check the checkout copy before then."
            s["decisions"] = ["Plus stays at $20 a month", "Add a yearly plan at two months free", "Beta pricing ships with Friday's release", "No free trial for Ultra"]
            s["action_items"] = [
                ["text": "Update the checkout copy for the yearly plan", "owner": "Ana", "due": iso(now.addingTimeInterval(2 * 86400))],
                ["text": "Draft the pricing announcement", "owner": "Marco", "due": NSNull()],
                ["text": "Check Stripe tax settings for EU buyers", "owner": "Priya", "due": NSNull()],
            ]
        }
        return s
    }

    private static func participant(_ id: String, _ name: String, _ now: Date) -> [String: Any] {
        ["id": id, "session_id": liveSession, "display_name": name, "email": NSNull(), "user_id": NSNull(), "class": "org_member",
         "joined_at": iso(now.addingTimeInterval(-12 * 60)), "left_at": NSNull(), "notice_at": iso(now.addingTimeInterval(-11 * 60))]
    }

    private static func approval(_ now: Date) -> [String: Any] {
        ["id": "sample-approval-1", "session_id": liveSession, "tool": "linear.list_issues",
         "args_summary": "Read your Linear board to answer Marco's question about open launch bugs.",
         "asked_by": "Marco Chen", "status": "pending", "requested_at": iso(now.addingTimeInterval(-40)), "resolved_at": NSNull()]
    }

    private static func artifact(_ id: String, _ sid: String, _ kind: String, _ title: String, _ version: Int, _ now: Date) -> [String: Any] {
        ["id": id, "session_id": sid, "artifact_id": "art-\(id)", "kind": kind, "title": title,
         "url": "https://ai.allternit.com/artifacts/art-\(id)", "version": version, "created_at": iso(now), "updated_at": iso(now)]
    }

    private static func transcript(_ sid: String) -> [[String: Any]] {
        let lines: [(String, String)] = [
            ("Ana Ruiz", "Morning. Quick one today, we need the beta out Friday."),
            ("Marco Chen", "Checkout is done. The yearly plan copy still needs a pass."),
            ("Priya Shah", "I'll take the EU tax check this afternoon."),
            ("Marco Chen", "Gizzi, draft the launch checklist from what we just said."),
            ("Gizzi", "Drafted it. The link is in the chat: Launch checklist."),
            ("Ana Ruiz", "Great. Put the QA item on me, due Thursday."),
        ]
        return lines.enumerated().map { i, line in
            ["id": "seg-\(i)", "session_id": sid, "seq": i + 1, "speaker": line.0, "participant_id": NSNull(),
             "confidence": 0.9, "source": "audio", "start_ms": 20_000 * (i + 1), "end_ms": 20_000 * (i + 1) + 6_000, "text": line.1]
        }
    }
}
