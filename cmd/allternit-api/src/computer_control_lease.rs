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

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(include_str!("../migrations/V189__computer_control_leases.sql")).unwrap();
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
}
