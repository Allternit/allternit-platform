//! Calendar sync and auto-join: the SSRF guard, the auto-join decision, and
//! the `meeting_join` run and ICS sync against a real Postgres schema
//! (routes::test_support, migration 130).

use super::*;
use crate::routes::test_support::{test_state, MockGateway};

fn at(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
}

#[test]
fn calendar_ssrf_guard_refuses_private_and_special_addresses() {
    for blocked in [
        "127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.169.254",
        "100.64.0.1", "100.127.255.254", "0.0.0.0", "255.255.255.255", "224.0.0.1", "198.18.0.1",
        "192.0.0.8", "240.0.0.1", "::1", "::", "fc00::1", "fd12:3456::1", "fe80::1", "::ffff:127.0.0.1",
        "::ffff:10.0.0.1", "64:ff9b::a9fe:a9fe", "2002:7f00:1::", "2001:db8::1", "ff02::1",
    ] {
        assert!(is_blocked_ip(blocked.parse().unwrap()), "{blocked} must be blocked");
    }
    for public in ["17.253.144.10", "140.82.112.3", "172.32.0.1", "100.128.0.1", "2606:4700::6810:84e5"] {
        assert!(!is_blocked_ip(public.parse().unwrap()), "{public} is public");
    }
}

#[test]
fn calendar_feed_urls_must_be_https_or_webcal() {
    assert_eq!(
        normalise_feed_url(" webcal://p52-caldav.icloud.com/published/2/abc ").as_deref(),
        Some("https://p52-caldav.icloud.com/published/2/abc")
    );
    assert_eq!(normalise_feed_url("WEBCALS://x.example/c.ics").as_deref(), Some("https://x.example/c.ics"));
    assert!(normalise_feed_url("https://outlook.office365.com/owa/calendar/x/reachcalendar.ics").is_some());
    for bad in ["http://example.com/c.ics", "ftp://example.com/c.ics", "file:///etc/passwd", "example.com/c.ics", ""] {
        assert_eq!(normalise_feed_url(bad), None, "{bad:?}");
    }
}

#[tokio::test]
async fn calendar_feed_fetch_refuses_loopback_before_connecting() {
    assert_eq!(fetch_feed("https://127.0.0.1/cal.ics", None).await.unwrap_err(), FeedError::Blocked);
    assert_eq!(fetch_feed("https://[::1]:8443/cal.ics", None).await.unwrap_err(), FeedError::Blocked);
    // A name that resolves to loopback is refused after DNS resolution.
    assert_eq!(fetch_feed("https://localhost/cal.ics", None).await.unwrap_err(), FeedError::Blocked);
    assert_eq!(fetch_feed("http://example.com/cal.ics", None).await.unwrap_err(), FeedError::InvalidUrl);
}

#[test]
fn meeting_auto_join_decision_rule_override_and_one_on_one() {
    use AutoJoinRule::*;
    let start = at("2026-11-16T17:00:00Z");
    let in_window = start - Duration::minutes(1);
    // (override, rule, 1:1) -> joins?
    let table = [
        (None, Off, false, false),
        (None, Off, true, false),
        (None, All, false, true),
        (None, All, true, true),
        (None, AllExcept1on1, false, true),
        (None, AllExcept1on1, true, false),
        (Some(true), Off, true, true),
        (Some(true), AllExcept1on1, true, true),
        (Some(false), All, false, false),
        (Some(false), AllExcept1on1, false, false),
    ];
    for (over, rule, one, expected) in table {
        assert_eq!(should_auto_join(over, rule, one, start, None, in_window), expected, "{over:?} {rule:?} 1:1={one}");
    }
    // The window: 2 minutes before the start, up to 15 after, never once over.
    let joins = |now: DateTime<Utc>, ends: Option<DateTime<Utc>>| should_auto_join(Some(true), All, false, start, ends, now);
    assert!(!joins(start - Duration::minutes(3), None));
    assert!(joins(start - Duration::minutes(2), None));
    assert!(joins(start + Duration::minutes(14), None));
    assert!(!joins(start + Duration::minutes(15), None));
    assert!(!joins(start + Duration::minutes(5), Some(start + Duration::minutes(5))), "already over");
}

#[test]
fn calendar_google_events_prefer_conference_links_and_count_outside_people() {
    let event: GEvent = serde_json::from_value(json!({
        "id": "evt1",
        "status": "confirmed",
        "summary": " Planning ",
        "start": { "dateTime": "2026-11-16T09:00:00-08:00" },
        "end": { "dateTime": "2026-11-16T09:30:00-08:00" },
        "location": "https://zoom.us/j/85512345678",
        "conferenceData": { "entryPoints": [
            { "entryPointType": "phone", "uri": "tel:+1-555-0100" },
            { "entryPointType": "video", "uri": "https://meet.google.com/abc-defg-hij" }
        ]},
        "attendees": [
            { "email": "me@acme.com", "self": true, "responseStatus": "accepted" },
            { "email": "ana@acme.com" },
            { "email": "guest@other.com" },
            { "email": "room@resource.calendar.google.com", "resource": true }
        ]
    }))
    .unwrap();
    let GoogleItem::Meeting(m) = google_item(event, "me@acme.com") else { panic!("a meeting") };
    assert_eq!(m.link.url_key, "meet:abc-defg-hij", "structured link beats the location");
    assert_eq!(m.starts_at, at("2026-11-16T17:00:00Z"));
    assert_eq!(m.title, "Planning");
    assert_eq!((m.attendee_count, m.external_attendee_count), (Some(3), Some(1)), "room excluded");

    let declined: GEvent = serde_json::from_value(json!({
        "id": "evt2", "start": { "dateTime": "2026-11-16T09:00:00Z" }, "hangoutLink": "https://meet.google.com/abc-defg-hij",
        "attendees": [{ "email": "me@gmail.com", "self": true, "responseStatus": "declined" }, { "email": "x@gmail.com" }]
    }))
    .unwrap();
    assert!(matches!(google_item(declined, "me@gmail.com"), GoogleItem::Removed(id) if id == "evt2"));
    let all_day: GEvent = serde_json::from_value(json!({
        "id": "evt3", "start": { "date": "2026-11-16" }, "hangoutLink": "https://meet.google.com/abc-defg-hij"
    }))
    .unwrap();
    assert!(matches!(google_item(all_day, "me@gmail.com"), GoogleItem::Skip));
    let html: GEvent = serde_json::from_value(json!({
        "id": "evt4", "start": { "dateTime": "2026-11-16T09:00:00Z" },
        "description": "<a href=\"https://us02web.zoom.us/j/85512345678?pwd=ab&amp;uname=x\">Join</a>"
    }))
    .unwrap();
    let GoogleItem::Meeting(h) = google_item(html, "me@gmail.com") else { panic!("a meeting") };
    assert_eq!(h.link.join_url, "https://us02web.zoom.us/j/85512345678?pwd=ab&uname=x");
    assert_eq!(h.external_attendee_count, None, "no attendee list");
}

// ---------------------------------------------------------------------------
// Database-backed
// ---------------------------------------------------------------------------

async fn setup() -> Arc<ApiState> {
    let state = test_state(Arc::new(MockGateway::new(None, vec![]))).await;
    for sql in [
        include_str!("../../../migrations_pg/130_meetings.sql"),
        "CREATE TABLE billing_subscriptions (user_id TEXT, plan_id TEXT, status TEXT, updated_at TIMESTAMPTZ DEFAULT now())",
    ] {
        sqlx::raw_sql(sql).execute(&state.db).await.expect("schema applies");
    }
    state
}

async fn paid_user(db: &PgPool, user: &str, rule: &str) {
    sqlx::query("INSERT INTO billing_subscriptions (user_id, plan_id, status) VALUES ($1, 'super', 'active')")
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_meeting_settings (user_id, auto_join) VALUES ($1, $2)")
        .bind(user)
        .bind(rule)
        .execute(db)
        .await
        .unwrap();
}

async fn calendar_meeting(db: &PgPool, user: &str, url: &str, starts_at: DateTime<Utc>, attendees: Option<i32>, auto_join: Option<bool>) -> Uuid {
    let link = meeting_links::parse_link(url).unwrap();
    sqlx::query_scalar(
        "INSERT INTO meetings (user_id, platform, title, join_url, url_key, starts_at, source, attendee_count, is_one_on_one, auto_join) \
         VALUES ($1, $2, 'Sync', $3, $4, $5, 'calendar_ics', $6, $7, $8) RETURNING id",
    )
    .bind(user)
    .bind(link.platform.as_str())
    .bind(&link.join_url)
    .bind(&link.url_key)
    .bind(starts_at)
    .bind(attendees)
    .bind(meeting_links::is_one_on_one(attendees))
    .bind(auto_join)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn session_of(db: &PgPool, meeting: Uuid) -> Option<(String, Option<String>)> {
    sqlx::query_as(
        "SELECT s.state, s.end_reason FROM meetings m JOIN meeting_sessions s ON s.id = m.latest_session_id WHERE m.id = $1",
    )
    .bind(meeting)
    .fetch_optional(db)
    .await
    .unwrap()
}

#[tokio::test]
async fn meeting_join_run_sends_gizzi_two_minutes_before_start() {
    let state = setup().await;
    let db = &state.db;
    let now = Utc::now();
    paid_user(db, "user_a", "all_except_1on1").await;

    let soon = now + Duration::seconds(90);
    let group = calendar_meeting(db, "user_a", "https://meet.google.com/aaa-bbbb-ccc", soon, Some(5), None).await;
    let one_on_one = calendar_meeting(db, "user_a", "https://meet.google.com/ddd-eeee-fff", soon, Some(2), None).await;
    let forced = calendar_meeting(db, "user_a", "https://meet.google.com/ggg-hhhh-iii", soon, Some(2), Some(true)).await;
    let later = calendar_meeting(db, "user_a", "https://meet.google.com/jjj-kkkk-lll", now + Duration::minutes(10), Some(5), None).await;
    let free_user = calendar_meeting(db, "user_free", "https://meet.google.com/mmm-nnnn-ooo", soon, Some(5), Some(true)).await;

    let stats = meeting_join_tick(db, now).await.unwrap();
    assert_eq!((stats.scheduled, stats.launching), (2, 2), "{stats:?}");
    assert_eq!(session_of(db, group).await, Some(("launching".into(), None)));
    assert_eq!(session_of(db, forced).await, Some(("launching".into(), None)), "override beats the 1:1 rule");
    assert_eq!(session_of(db, one_on_one).await, None, "1:1 skipped by all_except_1on1");
    assert_eq!(session_of(db, later).await, None, "not yet 2 minutes out");
    assert_eq!(session_of(db, free_user).await, None, "free plan: 402, nothing created");

    // The next tick changes nothing (one bot per url_key, no re-join).
    assert_eq!(meeting_join_tick(db, now).await.unwrap(), JoinStats::default());
}

#[tokio::test]
async fn meeting_join_run_records_no_capacity_and_honours_switch_off() {
    let state = setup().await;
    let db = &state.db;
    let now = Utc::now();
    paid_user(db, "user_b", "all").await;
    // The plan allows a set number of concurrent bots: fill them.
    let cap = crate::routes::org_meeting_settings::plan_limits("super").concurrent_cap;
    for i in 0..cap {
        let m = calendar_meeting(db, "user_b", &format!("https://zoom.us/j/8551234{:04}", i), now - Duration::minutes(30), Some(3), Some(false)).await;
        sqlx::query("INSERT INTO meeting_sessions (meeting_id, user_id, url_key, state, platform) SELECT id, user_id, url_key, 'live', platform FROM meetings WHERE id = $1")
            .bind(m)
            .execute(db)
            .await
            .unwrap();
    }
    let full = calendar_meeting(db, "user_b", "https://meet.google.com/aaa-bbbb-ccc", now + Duration::minutes(1), Some(4), None).await;
    let stats = meeting_join_tick(db, now).await.unwrap();
    assert_eq!(stats.no_capacity, 1, "{stats:?}");
    assert_eq!(session_of(db, full).await, Some(("ended".into(), Some("no_capacity".into()))));

    // Scheduled by hand, then switched off before the join time.
    let off = calendar_meeting(db, "user_b", "https://meet.google.com/ppp-qqqq-rrr", now + Duration::minutes(1), Some(4), None).await;
    let meeting = meetings::fetch_meeting(db, off).await.unwrap();
    let caller = Caller { id: "user_b".into(), ..Caller::default() };
    meetings::start_session(db, &caller, &meeting, When::Start, None).await.unwrap();
    sqlx::query("UPDATE meetings SET auto_join = false WHERE id = $1").bind(off).execute(db).await.unwrap();
    meeting_join_tick(db, now).await.unwrap();
    assert_eq!(session_of(db, off).await, Some(("ended".into(), Some("left_by_user".into()))));
}

fn feed(events: &[(&str, DateTime<Utc>, &str)]) -> String {
    let mut body = String::from("BEGIN:VCALENDAR\r\nX-WR-CALNAME:Test\r\n");
    for (uid, start, url) in events {
        body.push_str(&format!(
            "BEGIN:VEVENT\r\nUID:{uid}\r\nSUMMARY:Call {uid}\r\nDTSTART:{}\r\nDURATION:PT30M\r\nLOCATION:{url}\r\n\
ATTENDEE:mailto:a@x.com\r\nATTENDEE:mailto:b@x.com\r\nATTENDEE:mailto:c@x.com\r\nEND:VEVENT\r\n",
            start.format("%Y%m%dT%H%M%SZ")
        ));
    }
    body.push_str("END:VCALENDAR\r\n");
    body
}

#[tokio::test]
async fn calendar_ics_sync_upserts_moves_and_removes_meetings() {
    let state = setup().await;
    let db = &state.db;
    paid_user(db, "user_c", "all").await;
    let conn = sqlx::query_as::<_, ConnectionRow>(&format!(
        "INSERT INTO calendar_connections (user_id, provider, encrypted_feed_url, feed_url_hash) \
         VALUES ('user_c', 'ics', 'https://example.com/c.ics', 'h1') RETURNING {CONNECTION_COLUMNS}"
    ))
    .fetch_one(db)
    .await
    .unwrap();
    let t1 = Utc::now() + Duration::days(1);
    let t2 = Utc::now() + Duration::days(2);
    let body = feed(&[("a", t1, "https://meet.google.com/aaa-bbbb-ccc"), ("b", t2, "https://teams.microsoft.com/l/meetup-join/19%3ameeting_b%40thread.v2/0")]);
    sync_connection(&state, &conn, Some(FetchedFeed { body, ..Default::default() })).await;

    let rows: Vec<(String, String, i32, bool)> = sqlx::query_as(
        "SELECT external_event_id, source, attendee_count, is_one_on_one FROM meetings WHERE calendar_connection_id = $1 ORDER BY starts_at",
    )
    .bind(conn.id)
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(rows, vec![("a".into(), "calendar_ics".into(), 3, false), ("b".into(), "calendar_ics".into(), 3, false)]);

    // "b" got a Gizzi session scheduled; then "a" moves and "b" disappears.
    let b: Uuid = sqlx::query_scalar("SELECT id FROM meetings WHERE external_event_id = 'b'").fetch_one(db).await.unwrap();
    let meeting = meetings::fetch_meeting(db, b).await.unwrap();
    meetings::start_session(db, &Caller { id: "user_c".into(), ..Caller::default() }, &meeting, When::Start, None).await.unwrap();
    let t1_moved = t1 + Duration::hours(3);
    let body = feed(&[("a", t1_moved, "https://meet.google.com/aaa-bbbb-ccc")]);
    sync_connection(&state, &conn, Some(FetchedFeed { body, ..Default::default() })).await;

    let starts: Vec<DateTime<Utc>> = sqlx::query_scalar("SELECT starts_at FROM meetings WHERE calendar_connection_id = $1")
        .bind(conn.id)
        .fetch_all(db)
        .await
        .unwrap();
    assert_eq!(starts.len(), 1, "the dropped event and its scheduled bot are gone");
    assert_eq!(starts[0].timestamp(), t1_moved.timestamp());
    let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meeting_sessions").fetch_one(db).await.unwrap();
    assert_eq!(sessions, 0);

    // A meeting Gizzi already joined stays when the calendar drops it.
    let a: Uuid = sqlx::query_scalar("SELECT id FROM meetings WHERE external_event_id = 'a'").fetch_one(db).await.unwrap();
    sqlx::query("INSERT INTO meeting_sessions (meeting_id, user_id, url_key, state, end_reason, platform) SELECT id, user_id, url_key, 'ended', 'left_by_user', platform FROM meetings WHERE id = $1")
        .bind(a)
        .execute(db)
        .await
        .unwrap();
    sync_connection(&state, &conn, Some(FetchedFeed { body: feed(&[]), ..Default::default() })).await;
    assert!(meetings::fetch_meeting(db, a).await.is_ok(), "joined meeting kept");
}
