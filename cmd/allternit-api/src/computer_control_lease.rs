//! One controller per computer (ACI P4 control lease).
//!
//! Whoever holds a computer's lease may send it input (mouse, keyboard,
//! shell); everyone else watches. The lease lives in
//! `computer_control_leases` (V189) and expires unless renewed, so a closed
//! phone or a crashed agent can't lock a computer.
//!
//! Compatibility: with no live lease, input is allowed as before. The rule
//! only bites once someone takes control.
//!
//! Preemption: a person (`user`) can take control from an agent, a session,
//! a bot, or their own other device — that's "Take over". Nobody but the
//! holder can take it from a person, and agents never preempt each other.
//!
//! Hand-offs (V190): anyone who can't take control asks for it (a
//! `request` to the holder), and a holder can hand control to someone (an
//! `offer`). The other side accepts or declines; accepting moves the lease.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// How long a lease lasts without a renew.
pub const LEASE_TTL_SECS: i64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HolderKind {
    User,
    Agent,
    Session,
    Bot,
}

impl HolderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HolderKind::User => "user",
            HolderKind::Agent => "agent",
            HolderKind::Session => "session",
            HolderKind::Bot => "bot",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(HolderKind::User),
            "agent" => Some(HolderKind::Agent),
            "session" => Some(HolderKind::Session),
            "bot" => Some(HolderKind::Bot),
            _ => None,
        }
    }
}

/// Who is asking to control a computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub kind: HolderKind,
    pub id: String,
    pub label: Option<String>,
    /// For a person: the device they control from.
    pub device_id: Option<String>,
}

impl Holder {
    fn same_as(&self, other: &Holder) -> bool {
        self.kind == other.kind && self.id == other.id && self.device_id == other.device_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lease {
    pub computer_id: String,
    pub holder: Holder,
    pub acquired_at: String,
    pub expires_at: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TakeError {
    /// Someone else holds it and the caller may not preempt them.
    Held(Lease),
    Db(String),
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

fn iso(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

fn parse_secs(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.timestamp())
        .unwrap_or(0)
}

/// The live lease on a computer, if any (expired leases count as none).
pub fn current(conn: &Connection, computer_id: &str, now: i64) -> rusqlite::Result<Option<Lease>> {
    let row = conn
        .query_row(
            "SELECT holder_kind, holder_id, holder_label, device_id, acquired_at, expires_at
             FROM computer_control_leases WHERE computer_id = ?1",
            params![computer_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?;
    Ok(row.and_then(|(kind, id, label, device_id, acquired_at, expires_at)| {
        if parse_secs(&expires_at) <= now {
            return None;
        }
        Some(Lease {
            computer_id: computer_id.to_string(),
            holder: Holder { kind: HolderKind::parse(&kind)?, id, label, device_id },
            acquired_at,
            expires_at,
        })
    }))
}

/// Whether `taker` may take control while `holder` has it.
pub fn may_preempt(taker: &Holder, holder: &Holder) -> bool {
    if taker.same_as(holder) {
        return true;
    }
    match (taker.kind, holder.kind) {
        // A person takes over from an agent, session or bot, or moves
        // control between their own devices.
        (HolderKind::User, HolderKind::User) => taker.id == holder.id,
        (HolderKind::User, _) => true,
        // Nobody else takes control away from a person or another agent.
        _ => false,
    }
}

/// Take (or renew) control. Fails with the current lease when it's held by
/// someone the caller may not preempt.
pub fn take(conn: &Connection, computer_id: &str, taker: &Holder, now: i64) -> Result<Lease, TakeError> {
    let existing = current(conn, computer_id, now).map_err(|e| TakeError::Db(e.to_string()))?;
    if let Some(lease) = &existing {
        if !may_preempt(taker, &lease.holder) {
            return Err(TakeError::Held(lease.clone()));
        }
    }
    let acquired_at = match &existing {
        Some(lease) if lease.holder.same_as(taker) => lease.acquired_at.clone(),
        _ => iso(now),
    };
    let expires_at = iso(now + LEASE_TTL_SECS);
    conn.execute(
        "INSERT INTO computer_control_leases (computer_id, holder_kind, holder_id, holder_label, device_id, acquired_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(computer_id) DO UPDATE SET
            holder_kind = excluded.holder_kind, holder_id = excluded.holder_id,
            holder_label = excluded.holder_label, device_id = excluded.device_id,
            acquired_at = excluded.acquired_at, expires_at = excluded.expires_at",
        params![
            computer_id,
            taker.kind.as_str(),
            taker.id,
            taker.label,
            taker.device_id,
            acquired_at,
            expires_at
        ],
    )
    .map_err(|e| TakeError::Db(e.to_string()))?;
    Ok(Lease { computer_id: computer_id.to_string(), holder: taker.clone(), acquired_at, expires_at })
}

/// Hand control back. Only the holder (or a person who could take it over)
/// can release; returns whether a lease was removed.
pub fn release(conn: &Connection, computer_id: &str, releaser: &Holder, now: i64) -> rusqlite::Result<bool> {
    let Some(lease) = current(conn, computer_id, now)? else {
        conn.execute("DELETE FROM computer_control_leases WHERE computer_id = ?1", params![computer_id])?;
        return Ok(false);
    };
    if !may_preempt(releaser, &lease.holder) {
        return Ok(false);
    }
    Ok(conn.execute("DELETE FROM computer_control_leases WHERE computer_id = ?1", params![computer_id])? > 0)
}

/// Input gate: Ok when nobody holds control or `caller` does; otherwise the
/// lease that blocks them.
pub fn check_input(conn: &Connection, computer_id: &str, caller: &Holder, now: i64) -> rusqlite::Result<Result<(), Lease>> {
    Ok(match current(conn, computer_id, now)? {
        Some(lease) if !lease.holder.same_as(caller) => Err(lease),
        _ => Ok(()),
    })
}

/// Agent input on this Mac: the agent takes (or renews) the lease on every
/// one of `computer_ids` before it may send mouse or keyboard input, the same
/// rule a person follows. All-or-nothing: when someone it may not preempt (a
/// person who took over, or another agent) holds any of them, nothing is
/// taken and that lease is returned so the caller can answer 423.
pub fn take_all_for_agent(conn: &Connection, computer_ids: &[String], agent: &Holder, now: i64) -> Result<Vec<Lease>, TakeError> {
    for id in computer_ids {
        if let Some(lease) = current(conn, id, now).map_err(|e| TakeError::Db(e.to_string()))? {
            if !may_preempt(agent, &lease.holder) {
                return Err(TakeError::Held(lease));
            }
        }
    }
    computer_ids.iter().map(|id| take(conn, id, agent, now)).collect()
}

/// How long a hand-off request or offer stays open.
pub const HANDOFF_TTL_SECS: i64 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HandoffDirection {
    /// Asking the current holder for control.
    Request,
    /// The holder handing control to someone.
    Offer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Handoff {
    pub id: String,
    pub computer_id: String,
    pub direction: HandoffDirection,
    pub from: Holder,
    pub to: Holder,
    pub note: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandoffError {
    /// Nobody holds control: take it directly instead.
    NotHeld,
    /// Only the current holder can offer control.
    NotHolder,
    /// No such pending hand-off (or it expired).
    NotFound,
    /// Only the side it was sent to can accept or decline.
    NotRecipient,
    /// The lease changed hands since the hand-off was made.
    Stale,
    Db(String),
}

fn db_err(e: rusqlite::Error) -> HandoffError {
    HandoffError::Db(e.to_string())
}

fn row_to_handoff(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<Handoff>> {
    let direction = match r.get::<_, String>(2)?.as_str() {
        "request" => HandoffDirection::Request,
        "offer" => HandoffDirection::Offer,
        _ => return Ok(None),
    };
    let holder = |kind: String, id: String, label: Option<String>, device_id: Option<String>| {
        HolderKind::parse(&kind).map(|kind| Holder { kind, id, label, device_id })
    };
    let (Some(from), Some(to)) = (
        holder(r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?),
        holder(r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?),
    ) else {
        return Ok(None);
    };
    Ok(Some(Handoff {
        id: r.get(0)?,
        computer_id: r.get(1)?,
        direction,
        from,
        to,
        note: r.get(11)?,
        created_at: r.get(12)?,
        expires_at: r.get(13)?,
    }))
}

const HANDOFF_COLUMNS: &str = "id, computer_id, direction, from_kind, from_id, from_label, from_device_id,
     to_kind, to_id, to_label, to_device_id, note, created_at, expires_at";

/// Open hand-offs on a computer (expired ones are marked and left out).
pub fn pending_handoffs(conn: &Connection, computer_id: &str, now: i64) -> rusqlite::Result<Vec<Handoff>> {
    conn.execute(
        "UPDATE computer_control_handoffs SET status = 'expired', resolved_at = ?2
         WHERE computer_id = ?1 AND status = 'pending' AND expires_at <= ?2",
        params![computer_id, iso(now)],
    )?;
    let mut stmt = conn.prepare(&format!(
        "SELECT {HANDOFF_COLUMNS} FROM computer_control_handoffs
         WHERE computer_id = ?1 AND status = 'pending' ORDER BY created_at"
    ))?;
    let rows = stmt.query_map(params![computer_id], row_to_handoff)?;
    Ok(rows.filter_map(|r| r.ok().flatten()).collect())
}

fn insert_handoff(conn: &Connection, h: &Handoff) -> rusqlite::Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO computer_control_handoffs ({HANDOFF_COLUMNS}, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 'pending')"
        ),
        params![
            h.id,
            h.computer_id,
            match h.direction { HandoffDirection::Request => "request", HandoffDirection::Offer => "offer" },
            h.from.kind.as_str(),
            h.from.id,
            h.from.label,
            h.from.device_id,
            h.to.kind.as_str(),
            h.to.id,
            h.to.label,
            h.to.device_id,
            h.note,
            h.created_at,
            h.expires_at
        ],
    )?;
    Ok(())
}

/// Ask whoever holds control for it. One open request per asker; asking
/// again refreshes it.
pub fn request_control(conn: &Connection, computer_id: &str, asker: &Holder, note: Option<String>, now: i64) -> Result<Handoff, HandoffError> {
    let lease = current(conn, computer_id, now).map_err(db_err)?.ok_or(HandoffError::NotHeld)?;
    conn.execute(
        "UPDATE computer_control_handoffs SET status = 'cancelled', resolved_at = ?5
         WHERE computer_id = ?1 AND status = 'pending' AND direction = 'request'
           AND from_kind = ?2 AND from_id = ?3 AND IFNULL(from_device_id, '') = IFNULL(?4, '')",
        params![computer_id, asker.kind.as_str(), asker.id, asker.device_id, iso(now)],
    )
    .map_err(db_err)?;
    let handoff = Handoff {
        id: format!("handoff_{}", uuid::Uuid::new_v4().simple()),
        computer_id: computer_id.to_string(),
        direction: HandoffDirection::Request,
        from: asker.clone(),
        to: lease.holder,
        note,
        created_at: iso(now),
        expires_at: iso(now + HANDOFF_TTL_SECS),
    };
    insert_handoff(conn, &handoff).map_err(db_err)?;
    Ok(handoff)
}

/// The holder hands control to `recipient`, who accepts or declines.
pub fn offer_control(conn: &Connection, computer_id: &str, holder: &Holder, recipient: &Holder, note: Option<String>, now: i64) -> Result<Handoff, HandoffError> {
    let lease = current(conn, computer_id, now).map_err(db_err)?.ok_or(HandoffError::NotHeld)?;
    if !lease.holder.same_as(holder) {
        return Err(HandoffError::NotHolder);
    }
    let handoff = Handoff {
        id: format!("handoff_{}", uuid::Uuid::new_v4().simple()),
        computer_id: computer_id.to_string(),
        direction: HandoffDirection::Offer,
        from: holder.clone(),
        to: recipient.clone(),
        note,
        created_at: iso(now),
        expires_at: iso(now + HANDOFF_TTL_SECS),
    };
    insert_handoff(conn, &handoff).map_err(db_err)?;
    Ok(handoff)
}

fn is_recipient(handoff: &Handoff, who: &Holder) -> bool {
    // A person answers from any of their devices.
    if handoff.to.kind == HolderKind::User && who.kind == HolderKind::User {
        return handoff.to.id == who.id;
    }
    handoff.to.same_as(who)
}

/// Accept or decline a hand-off addressed to `who`. Accepting a request
/// gives control to the asker; accepting an offer gives it to `who`.
pub fn answer_handoff(conn: &Connection, computer_id: &str, handoff_id: &str, who: &Holder, accept: bool, now: i64) -> Result<Option<Lease>, HandoffError> {
    let handoff = pending_handoffs(conn, computer_id, now)
        .map_err(db_err)?
        .into_iter()
        .find(|h| h.id == handoff_id)
        .ok_or(HandoffError::NotFound)?;
    if !is_recipient(&handoff, who) {
        return Err(HandoffError::NotRecipient);
    }
    let resolve = |status: &str| {
        conn.execute(
            "UPDATE computer_control_handoffs SET status = ?2, resolved_at = ?3 WHERE id = ?1",
            params![handoff.id, status, iso(now)],
        )
        .map_err(db_err)
    };
    if !accept {
        resolve("declined")?;
        return Ok(None);
    }
    // The hand-off only holds while the person who made it (request: the
    // holder it went to; offer: the holder who made it) still has control.
    let giver = match handoff.direction {
        HandoffDirection::Request => &handoff.to,
        HandoffDirection::Offer => &handoff.from,
    };
    let lease = current(conn, computer_id, now).map_err(db_err)?;
    if !lease.as_ref().is_some_and(|l| l.holder.id == giver.id && l.holder.kind == giver.kind) {
        resolve("expired")?;
        return Err(HandoffError::Stale);
    }
    let mut taker = match handoff.direction {
        HandoffDirection::Request => handoff.from.clone(),
        HandoffDirection::Offer => handoff.to.clone(),
    };
    // A person accepting an offer takes control on the device they accepted from.
    if handoff.direction == HandoffDirection::Offer && who.kind == HolderKind::User {
        taker.device_id = who.device_id.clone();
    }
    conn.execute("DELETE FROM computer_control_leases WHERE computer_id = ?1", params![computer_id]).map_err(db_err)?;
    let lease = take(conn, computer_id, &taker, now).map_err(|e| match e {
        TakeError::Db(e) => HandoffError::Db(e),
        TakeError::Held(_) => HandoffError::Stale,
    })?;
    resolve("accepted")?;
    Ok(Some(lease))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V189__computer_control_leases.sql")).unwrap();
        conn.execute_batch(include_str!("../migrations/V190__computer_control_handoffs.sql")).unwrap();
        conn
    }

    fn user(id: &str, device: &str) -> Holder {
        Holder { kind: HolderKind::User, id: id.into(), label: None, device_id: Some(device.into()) }
    }

    fn agent(id: &str) -> Holder {
        Holder { kind: HolderKind::Agent, id: id.into(), label: Some("Researcher".into()), device_id: None }
    }

    #[test]
    fn free_computer_accepts_input_from_anyone() {
        let conn = db();
        assert_eq!(check_input(&conn, "c1", &agent("a1"), 1_000).unwrap(), Ok(()));
    }

    #[test]
    fn an_agent_needs_the_lease_on_this_mac_and_a_person_takes_over() {
        let conn = db();
        let ids = vec!["mac".to_string()];
        // Free: the agent takes it.
        let taken = take_all_for_agent(&conn, &ids, &agent("a1"), 1_000).unwrap();
        assert_eq!(taken[0].holder.id, "a1");
        // The same agent renews it on its next step.
        assert!(take_all_for_agent(&conn, &ids, &agent("a1"), 1_010).is_ok());
        // The person takes over; the agent is then locked out (423).
        take(&conn, "mac", &user("u1", "laptop"), 1_020).unwrap();
        let Err(TakeError::Held(lease)) = take_all_for_agent(&conn, &ids, &agent("a1"), 1_030) else {
            panic!("a person holding control must block the agent");
        };
        assert_eq!(lease.holder.kind, HolderKind::User);
        // All-or-nothing: a blocked row means nothing else is taken.
        let two = vec!["free".to_string(), "mac".to_string()];
        assert!(take_all_for_agent(&conn, &two, &agent("a1"), 1_030).is_err());
        assert!(current(&conn, "free", 1_030).unwrap().is_none());
    }

    #[test]
    fn one_controller_at_a_time() {
        let conn = db();
        take(&conn, "c1", &agent("a1"), 1_000).unwrap();
        assert!(check_input(&conn, "c1", &agent("a1"), 1_010).unwrap().is_ok());
        let blocked = check_input(&conn, "c1", &user("u1", "phone"), 1_010).unwrap().unwrap_err();
        assert_eq!(blocked.holder.id, "a1");
        // Another agent can't grab it.
        assert!(matches!(take(&conn, "c1", &agent("a2"), 1_010), Err(TakeError::Held(_))));
    }

    #[test]
    fn a_person_takes_over_from_an_agent_and_hands_back() {
        let conn = db();
        take(&conn, "c1", &agent("a1"), 1_000).unwrap();
        let lease = take(&conn, "c1", &user("u1", "phone"), 1_005).unwrap();
        assert_eq!(lease.holder.kind, HolderKind::User);
        assert!(check_input(&conn, "c1", &agent("a1"), 1_006).unwrap().is_err());
        // The agent can't take it back from a person.
        assert!(matches!(take(&conn, "c1", &agent("a1"), 1_006), Err(TakeError::Held(_))));
        assert!(release(&conn, "c1", &user("u1", "phone"), 1_007).unwrap());
        assert!(check_input(&conn, "c1", &agent("a1"), 1_008).unwrap().is_ok());
    }

    #[test]
    fn control_moves_between_your_own_devices_but_not_to_other_people() {
        let conn = db();
        take(&conn, "c1", &user("u1", "mac"), 1_000).unwrap();
        assert!(take(&conn, "c1", &user("u1", "phone"), 1_001).is_ok());
        assert!(matches!(take(&conn, "c1", &user("u2", "laptop"), 1_002), Err(TakeError::Held(_))));
        assert!(!release(&conn, "c1", &user("u2", "laptop"), 1_003).unwrap());
    }

    #[test]
    fn leases_expire_and_renewing_keeps_the_start_time() {
        let conn = db();
        let first = take(&conn, "c1", &agent("a1"), 1_000).unwrap();
        let renewed = take(&conn, "c1", &agent("a1"), 1_100).unwrap();
        assert_eq!(renewed.acquired_at, first.acquired_at);
        assert!(current(&conn, "c1", 1_100 + LEASE_TTL_SECS - 1).unwrap().is_some());
        assert!(current(&conn, "c1", 1_100 + LEASE_TTL_SECS).unwrap().is_none());
        // Expired: anyone may send input and take control.
        assert!(take(&conn, "c1", &agent("a2"), 1_100 + LEASE_TTL_SECS + 1).is_ok());
    }

    #[test]
    fn an_agent_asks_a_person_for_control_and_gets_it_when_accepted() {
        let conn = db();
        take(&conn, "c1", &user("u1", "mac"), 1_000).unwrap();
        let ask = request_control(&conn, "c1", &agent("a1"), Some("need to finish the form".into()), 1_001).unwrap();
        assert_eq!(ask.to.id, "u1");
        assert_eq!(pending_handoffs(&conn, "c1", 1_002).unwrap().len(), 1);
        // The person answers from their phone.
        let lease = answer_handoff(&conn, "c1", &ask.id, &user("u1", "phone"), true, 1_003).unwrap().unwrap();
        assert_eq!(lease.holder.id, "a1");
        assert!(pending_handoffs(&conn, "c1", 1_004).unwrap().is_empty());
        assert!(check_input(&conn, "c1", &agent("a1"), 1_004).unwrap().is_ok());
    }

    #[test]
    fn declining_keeps_control_and_only_the_recipient_can_answer() {
        let conn = db();
        take(&conn, "c1", &user("u1", "mac"), 1_000).unwrap();
        let ask = request_control(&conn, "c1", &agent("a1"), None, 1_001).unwrap();
        assert_eq!(answer_handoff(&conn, "c1", &ask.id, &agent("a2"), true, 1_002), Err(HandoffError::NotRecipient));
        assert_eq!(answer_handoff(&conn, "c1", &ask.id, &user("u1", "mac"), false, 1_002), Ok(None));
        assert_eq!(current(&conn, "c1", 1_003).unwrap().unwrap().holder.id, "u1");
    }

    #[test]
    fn a_holder_offers_control_and_the_recipient_accepts() {
        let conn = db();
        take(&conn, "c1", &user("u1", "mac"), 1_000).unwrap();
        assert_eq!(offer_control(&conn, "c1", &agent("a1"), &user("u1", "mac"), None, 1_001), Err(HandoffError::NotHolder));
        let offer = offer_control(&conn, "c1", &user("u1", "mac"), &agent("a1"), None, 1_001).unwrap();
        let lease = answer_handoff(&conn, "c1", &offer.id, &agent("a1"), true, 1_002).unwrap().unwrap();
        assert_eq!(lease.holder.kind, HolderKind::Agent);
    }

    #[test]
    fn hand_offs_go_stale_or_expire() {
        let conn = db();
        take(&conn, "c1", &user("u1", "mac"), 1_000).unwrap();
        let ask = request_control(&conn, "c1", &agent("a1"), None, 1_001).unwrap();
        // Control changed hands before the answer: the request is stale.
        release(&conn, "c1", &user("u1", "mac"), 1_002).unwrap();
        take(&conn, "c1", &agent("a2"), 1_003).unwrap();
        assert_eq!(answer_handoff(&conn, "c1", &ask.id, &user("u1", "mac"), true, 1_004), Err(HandoffError::Stale));
        let old = request_control(&conn, "c1", &agent("a3"), None, 1_005).unwrap();
        assert!(pending_handoffs(&conn, "c1", 1_005 + HANDOFF_TTL_SECS).unwrap().iter().all(|h| h.id != old.id));
        assert_eq!(request_control(&conn, "c2", &agent("a1"), None, 1_006), Err(HandoffError::NotHeld));
    }
}
