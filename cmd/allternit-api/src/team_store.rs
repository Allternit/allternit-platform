//! Team map store (migration V294, PLAN.md §2.2-2.4 of the Island bar + Team
//! map program): per-bot app grants, approval rules, who reports to whom, and
//! the owner-routing columns on `cowork_approvals`.
//!
//! Store only: no routes and no rule evaluation here. Rule matching on tool
//! arguments and approver resolution live in `team_rules.rs` (lane 11); the
//! HTTP surface lives in `team_routes.rs` (lanes 12-13).
//!
//! Style follows the SQLite helpers in `cowork_routes.rs`
//! (`apply_approval_decision`, `prepare_approvals_list_stmt`): plain functions
//! over a borrowed `rusqlite::Connection`, so a route can run several of them
//! inside one `spawn_blocking` and one transaction. Rows are typed structs like
//! `agency_api/decisions/store.rs`.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

// ── Shared vocabulary ────────────────────────────────────────────────────────

/// Grant levels; the same ids as #2921's `app_permission_tier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantLevel {
    Browser,
    App,
    Full,
}

impl GrantLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            GrantLevel::Browser => "browser",
            GrantLevel::App => "app",
            GrantLevel::Full => "full",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "browser" => Some(GrantLevel::Browser),
            "app" => Some(GrantLevel::App),
            "full" => Some(GrantLevel::Full),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BotKind {
    /// `agents.id`
    Agent,
    /// `vendor_bot_connectors.vendor_bot_id`
    Vendor,
}

impl BotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BotKind::Agent => "agent",
            BotKind::Vendor => "vendor",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(BotKind::Agent),
            "vendor" => Some(BotKind::Vendor),
            _ => None,
        }
    }

    /// Team map node id for a bot: `b:<agent_id>` or `v:<vendor_bot_id>`.
    /// Bot-scoped rules store this in `approval_rules.scope_ref`.
    pub fn node_ref(self, bot_id: &str) -> String {
        match self {
            BotKind::Agent => format!("b:{bot_id}"),
            BotKind::Vendor => format!("v:{bot_id}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppKind {
    Connector,
    Vendor,
    Artifact,
}

impl AppKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AppKind::Connector => "connector",
            AppKind::Vendor => "vendor",
            AppKind::Artifact => "artifact",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "connector" => Some(AppKind::Connector),
            "vendor" => Some(AppKind::Vendor),
            "artifact" => Some(AppKind::Artifact),
            _ => None,
        }
    }
}

/// Parse a stored enum column, turning a bad value into a row error instead of
/// a panic (the CHECK constraints make this unreachable on V294 rows).
fn col_enum<T>(row: &Row<'_>, idx: usize, parse: fn(&str) -> Option<T>) -> rusqlite::Result<T> {
    let s: String = row.get(idx)?;
    parse(&s).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            idx,
            rusqlite::types::Type::Text,
            format!("unexpected value {s:?}").into(),
        )
    })
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", uuid::Uuid::new_v4().simple())
}

/// Run `f` inside a SAVEPOINT so it is atomic whether or not the caller already
/// holds a transaction (`unchecked_transaction` cannot nest).
fn atomically<T>(conn: &Connection, name: &str, f: impl FnOnce() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    conn.execute_batch(&format!("SAVEPOINT {name}"))?;
    match f() {
        Ok(v) => {
            conn.execute_batch(&format!("RELEASE {name}"))?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"));
            Err(e)
        }
    }
}

// ── Grants (bot_app_grants) ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BotAppGrant {
    pub id: String,
    pub bot_id: String,
    pub bot_kind: BotKind,
    pub app_kind: AppKind,
    pub app_ref: String,
    pub level: GrantLevel,
    pub granted_by: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

const GRANT_COLS: &str = "id, bot_id, bot_kind, app_kind, app_ref, level, granted_by, created_at, revoked_at";

fn grant_from_row(row: &Row<'_>) -> rusqlite::Result<BotAppGrant> {
    Ok(BotAppGrant {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        bot_kind: col_enum(row, 2, BotKind::parse)?,
        app_kind: col_enum(row, 3, AppKind::parse)?,
        app_ref: row.get(4)?,
        level: col_enum(row, 5, GrantLevel::parse)?,
        granted_by: row.get(6)?,
        created_at: row.get(7)?,
        revoked_at: row.get(8)?,
    })
}

/// The live (not revoked) grant for a bot on an app. `None` = the bot may not
/// call the app (fail closed).
pub fn get_live_grant(
    conn: &Connection,
    bot_kind: BotKind,
    bot_id: &str,
    app_kind: AppKind,
    app_ref: &str,
) -> rusqlite::Result<Option<BotAppGrant>> {
    conn.query_row(
        &format!(
            "SELECT {GRANT_COLS} FROM bot_app_grants
             WHERE bot_kind = ?1 AND bot_id = ?2 AND app_kind = ?3 AND app_ref = ?4 AND revoked_at IS NULL"
        ),
        params![bot_kind.as_str(), bot_id, app_kind.as_str(), app_ref],
        grant_from_row,
    )
    .optional()
}

/// Live grants of one bot, by app.
pub fn list_grants_for_bot(conn: &Connection, bot_kind: BotKind, bot_id: &str) -> rusqlite::Result<Vec<BotAppGrant>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {GRANT_COLS} FROM bot_app_grants
         WHERE bot_kind = ?1 AND bot_id = ?2 AND revoked_at IS NULL
         ORDER BY app_kind, app_ref"
    ))?;
    let rows = stmt.query_map(params![bot_kind.as_str(), bot_id], grant_from_row)?;
    rows.collect()
}

/// Live grants on one app ("Who can use this").
pub fn list_grants_for_app(conn: &Connection, app_kind: AppKind, app_ref: &str) -> rusqlite::Result<Vec<BotAppGrant>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {GRANT_COLS} FROM bot_app_grants
         WHERE app_kind = ?1 AND app_ref = ?2 AND revoked_at IS NULL
         ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map(params![app_kind.as_str(), app_ref], grant_from_row)?;
    rows.collect()
}

/// Set a bot's level on an app. Same level as the live row → that row, no
/// write. Otherwise the live row is revoked and a new one inserted, atomically,
/// so the table keeps the history. Ceiling checks (vendor max, installer tier)
/// are the caller's job and must run before this.
pub fn upsert_grant(
    conn: &Connection,
    bot_kind: BotKind,
    bot_id: &str,
    app_kind: AppKind,
    app_ref: &str,
    level: GrantLevel,
    granted_by: &str,
) -> rusqlite::Result<BotAppGrant> {
    atomically(conn, "team_upsert_grant", || {
        if let Some(live) = get_live_grant(conn, bot_kind, bot_id, app_kind, app_ref)? {
            if live.level == level {
                return Ok(live);
            }
            conn.execute(
                "UPDATE bot_app_grants SET revoked_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![live.id],
            )?;
        }
        let id = new_id("bag");
        conn.execute(
            "INSERT INTO bot_app_grants (id, bot_id, bot_kind, app_kind, app_ref, level, granted_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, bot_id, bot_kind.as_str(), app_kind.as_str(), app_ref, level.as_str(), granted_by],
        )?;
        get_live_grant(conn, bot_kind, bot_id, app_kind, app_ref)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
    })
}

/// Revoke the live grant. Returns whether a live row existed.
pub fn revoke_grant(
    conn: &Connection,
    bot_kind: BotKind,
    bot_id: &str,
    app_kind: AppKind,
    app_ref: &str,
) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE bot_app_grants SET revoked_at = CURRENT_TIMESTAMP
         WHERE bot_kind = ?1 AND bot_id = ?2 AND app_kind = ?3 AND app_ref = ?4 AND revoked_at IS NULL",
        params![bot_kind.as_str(), bot_id, app_kind.as_str(), app_ref],
    )?;
    Ok(n > 0)
}

// ── Rules (approval_rules) ───────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScopeKind {
    Org,
    Person,
    Bot,
}

impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ScopeKind::Org => "org",
            ScopeKind::Person => "person",
            ScopeKind::Bot => "bot",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "org" => Some(ScopeKind::Org),
            "person" => Some(ScopeKind::Person),
            "bot" => Some(ScopeKind::Bot),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleOp {
    Always,
    Gt,
    Gte,
    Eq,
    In,
}

impl RuleOp {
    pub fn as_str(self) -> &'static str {
        match self {
            RuleOp::Always => "always",
            RuleOp::Gt => "gt",
            RuleOp::Gte => "gte",
            RuleOp::Eq => "eq",
            RuleOp::In => "in",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "always" => Some(RuleOp::Always),
            "gt" => Some(RuleOp::Gt),
            "gte" => Some(RuleOp::Gte),
            "eq" => Some(RuleOp::Eq),
            "in" => Some(RuleOp::In),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApproverKind {
    Owner,
    Manager,
    User,
}

impl ApproverKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ApproverKind::Owner => "owner",
            ApproverKind::Manager => "manager",
            ApproverKind::User => "user",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(ApproverKind::Owner),
            "manager" => Some(ApproverKind::Manager),
            "user" => Some(ApproverKind::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ApprovalRule {
    pub id: String,
    pub org_id: Option<String>,
    pub owner_user_id: String,
    pub scope_kind: ScopeKind,
    pub scope_ref: Option<String>,
    pub app_kind: Option<AppKind>,
    pub app_ref: Option<String>,
    pub tool: String,
    pub field: Option<String>,
    pub op: RuleOp,
    pub value: Option<serde_json::Value>,
    pub unit_label: Option<String>,
    pub approver_kind: ApproverKind,
    pub approver_user_id: Option<String>,
    pub enabled: bool,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
}

/// The editable part of a rule: create takes all of it, update replaces all of
/// it (a PATCH route reads the rule, merges, and passes the merged input).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RuleInput {
    pub org_id: Option<String>,
    pub owner_user_id: String,
    pub scope_kind: ScopeKind,
    pub scope_ref: Option<String>,
    pub app_kind: Option<AppKind>,
    pub app_ref: Option<String>,
    /// `"*"` or an exact tool name.
    pub tool: String,
    pub field: Option<String>,
    pub op: RuleOp,
    pub value: Option<serde_json::Value>,
    pub unit_label: Option<String>,
    pub approver_kind: ApproverKind,
    pub approver_user_id: Option<String>,
    pub enabled: bool,
}

const RULE_COLS: &str = "id, org_id, owner_user_id, scope_kind, scope_ref, app_kind, app_ref, tool, field, op, \
     value_json, unit_label, approver_kind, approver_user_id, enabled, created_by, created_at, updated_at";

fn rule_from_row(row: &Row<'_>) -> rusqlite::Result<ApprovalRule> {
    let app_kind: Option<String> = row.get(5)?;
    let value_json: Option<String> = row.get(10)?;
    Ok(ApprovalRule {
        id: row.get(0)?,
        org_id: row.get(1)?,
        owner_user_id: row.get(2)?,
        scope_kind: col_enum(row, 3, ScopeKind::parse)?,
        scope_ref: row.get(4)?,
        app_kind: app_kind.as_deref().and_then(AppKind::parse),
        app_ref: row.get(6)?,
        tool: row.get(7)?,
        field: row.get(8)?,
        op: col_enum(row, 9, RuleOp::parse)?,
        value: value_json.and_then(|s| serde_json::from_str(&s).ok()),
        unit_label: row.get(11)?,
        approver_kind: col_enum(row, 12, ApproverKind::parse)?,
        approver_user_id: row.get(13)?,
        enabled: row.get::<_, i64>(14)? != 0,
        created_by: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

pub fn create_rule(conn: &Connection, input: &RuleInput, created_by: &str) -> rusqlite::Result<ApprovalRule> {
    let id = new_id("arl");
    conn.execute(
        "INSERT INTO approval_rules (id, org_id, owner_user_id, scope_kind, scope_ref, app_kind, app_ref, tool,
             field, op, value_json, unit_label, approver_kind, approver_user_id, enabled, created_by)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            id,
            input.org_id,
            input.owner_user_id,
            input.scope_kind.as_str(),
            input.scope_ref,
            input.app_kind.map(AppKind::as_str),
            input.app_ref,
            input.tool,
            input.field,
            input.op.as_str(),
            input.value.as_ref().map(|v| v.to_string()),
            input.unit_label,
            input.approver_kind.as_str(),
            input.approver_user_id,
            input.enabled,
            created_by,
        ],
    )?;
    get_rule(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

pub fn get_rule(conn: &Connection, id: &str) -> rusqlite::Result<Option<ApprovalRule>> {
    conn.query_row(&format!("SELECT {RULE_COLS} FROM approval_rules WHERE id = ?1"), params![id], rule_from_row)
        .optional()
}

/// Replace a rule's editable fields. `None` when the rule does not exist.
pub fn update_rule(conn: &Connection, id: &str, input: &RuleInput) -> rusqlite::Result<Option<ApprovalRule>> {
    let n = conn.execute(
        "UPDATE approval_rules SET org_id = ?2, owner_user_id = ?3, scope_kind = ?4, scope_ref = ?5, app_kind = ?6,
             app_ref = ?7, tool = ?8, field = ?9, op = ?10, value_json = ?11, unit_label = ?12, approver_kind = ?13,
             approver_user_id = ?14, enabled = ?15, updated_at = CURRENT_TIMESTAMP
         WHERE id = ?1",
        params![
            id,
            input.org_id,
            input.owner_user_id,
            input.scope_kind.as_str(),
            input.scope_ref,
            input.app_kind.map(AppKind::as_str),
            input.app_ref,
            input.tool,
            input.field,
            input.op.as_str(),
            input.value.as_ref().map(|v| v.to_string()),
            input.unit_label,
            input.approver_kind.as_str(),
            input.approver_user_id,
            input.enabled,
        ],
    )?;
    if n == 0 {
        return Ok(None);
    }
    get_rule(conn, id)
}

/// Delete a rule. Returns whether it existed. `cowork_approvals.rule_id` keeps
/// pointing at the id (no foreign key) so past asks still name the rule.
pub fn delete_rule(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM approval_rules WHERE id = ?1", params![id])? > 0)
}

/// Every rule (enabled or not) of an org, or of a solo account when `org_id`
/// is `None` (rules with no org owned by `owner_user_id`). For the map and the
/// rules editor.
pub fn list_rules(conn: &Connection, org_id: Option<&str>, owner_user_id: &str) -> rusqlite::Result<Vec<ApprovalRule>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {RULE_COLS} FROM approval_rules
         WHERE (?1 IS NOT NULL AND org_id = ?1) OR (?1 IS NULL AND org_id IS NULL AND owner_user_id = ?2)
         ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map(params![org_id, owner_user_id], rule_from_row)?;
    rows.collect()
}

/// Who is acting, for rule lookup.
#[derive(Debug, Clone, Copy)]
pub struct RuleActor<'a> {
    /// The org the call runs in; `None` for a solo account.
    pub org_id: Option<&'a str>,
    /// The person acting, or the bot's owner (`agents.user_id`) when a bot acts.
    pub person_user_id: &'a str,
    /// The acting bot, if any.
    pub bot: Option<(BotKind, &'a str)>,
}

/// Enabled rules that could apply to this actor calling `tool` on this app.
/// Candidates only: the caller (`team_rules.rs`) still checks `field`/`op`/
/// `value` against the tool arguments and resolves the approver.
///
/// Scope match: `org` = the actor's org (solo account: the person's own
/// no-org rules); `person` = the acting person or the bot's owner, so a rule
/// on a person also covers their bots; `bot` = the bot's node ref (`b:`/`v:`).
/// App match: no app on the rule = every app. Tool match: `'*'` or exact.
pub fn list_rules_for_call(
    conn: &Connection,
    actor: &RuleActor<'_>,
    app_kind: AppKind,
    app_ref: &str,
    tool: &str,
) -> rusqlite::Result<Vec<ApprovalRule>> {
    let bot_ref = actor.bot.map(|(kind, id)| kind.node_ref(id));
    let mut stmt = conn.prepare(&format!(
        "SELECT {RULE_COLS} FROM approval_rules
         WHERE enabled = 1
           AND (tool = '*' OR tool = ?5)
           AND (app_kind IS NULL OR (app_kind = ?3 AND (app_ref IS NULL OR app_ref = ?4)))
           AND (
                (scope_kind = 'org' AND ((?1 IS NOT NULL AND org_id = ?1)
                                         OR (?1 IS NULL AND org_id IS NULL AND owner_user_id = ?2)))
             OR (scope_kind = 'person' AND scope_ref = ?2 AND (org_id IS NULL OR org_id = ?1))
             OR (scope_kind = 'bot' AND ?6 IS NOT NULL AND scope_ref = ?6 AND (org_id IS NULL OR org_id = ?1))
           )
         ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map(
        params![actor.org_id, actor.person_user_id, app_kind.as_str(), app_ref, tool, bot_ref],
        rule_from_row,
    )?;
    rows.collect()
}

// ── People (organization_members.reports_to_user_id) ─────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemberRow {
    pub user_id: String,
    pub role: String,
    pub reports_to_user_id: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    pub joined_at: Option<String>,
}

/// Members of an org with who they report to. Name/email/avatar come from
/// `users` when the row exists there.
pub fn list_members(conn: &Connection, org_id: &str) -> rusqlite::Result<Vec<MemberRow>> {
    let mut stmt = conn.prepare(
        "SELECT m.user_id, m.role, m.reports_to_user_id, u.name, u.email, u.avatar_url, m.joined_at
         FROM organization_members m LEFT JOIN users u ON u.id = m.user_id
         WHERE m.organization_id = ?1
         ORDER BY m.joined_at, m.user_id",
    )?;
    let rows = stmt.query_map(params![org_id], |r| {
        Ok(MemberRow {
            user_id: r.get(0)?,
            role: r.get(1)?,
            reports_to_user_id: r.get(2)?,
            name: r.get(3)?,
            email: r.get(4)?,
            avatar_url: r.get(5)?,
            joined_at: r.get(6)?,
        })
    })?;
    rows.collect()
}

/// A member's manager, `None` when at the top or not a member.
pub fn get_reports_to(conn: &Connection, org_id: &str, user_id: &str) -> rusqlite::Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT reports_to_user_id FROM organization_members WHERE organization_id = ?1 AND user_id = ?2",
            params![org_id, user_id],
            |r| r.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportsToOutcome {
    Set,
    /// `user_id` is not a member of the org.
    NotMember,
    /// The manager is not a member of the org.
    ManagerNotMember,
    /// A person cannot report to themself.
    SelfReport,
    /// The manager already reports (directly or not) to `user_id`.
    Cycle,
}

/// Set (or clear with `None`) who `user_id` reports to. Rejects a cycle by
/// walking up from the new manager; the walk is bounded by the member count so
/// a cycle already in the data cannot loop forever.
pub fn set_reports_to(
    conn: &Connection,
    org_id: &str,
    user_id: &str,
    reports_to_user_id: Option<&str>,
) -> rusqlite::Result<ReportsToOutcome> {
    atomically(conn, "team_set_reports_to", || {
        let is_member = |uid: &str| -> rusqlite::Result<bool> {
            conn.query_row(
                "SELECT COUNT(*) FROM organization_members WHERE organization_id = ?1 AND user_id = ?2",
                params![org_id, uid],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
        };
        if !is_member(user_id)? {
            return Ok(ReportsToOutcome::NotMember);
        }
        if let Some(manager) = reports_to_user_id {
            if manager == user_id {
                return Ok(ReportsToOutcome::SelfReport);
            }
            if !is_member(manager)? {
                return Ok(ReportsToOutcome::ManagerNotMember);
            }
            let limit: i64 = conn.query_row(
                "SELECT COUNT(*) FROM organization_members WHERE organization_id = ?1",
                params![org_id],
                |r| r.get(0),
            )?;
            let mut cursor = Some(manager.to_string());
            let mut steps = 0;
            while let Some(current) = cursor {
                if current == user_id {
                    return Ok(ReportsToOutcome::Cycle);
                }
                steps += 1;
                if steps > limit {
                    // Existing data already loops above the manager; adding
                    // another edge into it is refused too.
                    return Ok(ReportsToOutcome::Cycle);
                }
                cursor = get_reports_to(conn, org_id, &current)?;
            }
        }
        conn.execute(
            "UPDATE organization_members SET reports_to_user_id = ?3 WHERE organization_id = ?1 AND user_id = ?2",
            params![org_id, user_id, reports_to_user_id],
        )?;
        Ok(ReportsToOutcome::Set)
    })
}

// ── Owner routing on cowork_approvals ────────────────────────────────────────

/// The V294 routing columns of a `cowork_approvals` row. All `None` = today's
/// behaviour (the ask belongs to `user_id`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ApprovalRouting {
    pub approver_user_id: Option<String>,
    pub rule_id: Option<String>,
    pub bot_id: Option<String>,
    pub job_id: Option<String>,
}

pub struct NewRoutedApproval<'a> {
    pub id: &'a str,
    /// The requester (who the ask is about), as in every other row.
    pub user_id: Option<&'a str>,
    pub content: &'a str,
    pub source: &'a str,
    pub routing: &'a ApprovalRouting,
}

/// Insert a pending `cowork_approvals` row with routing set. `INSERT OR
/// IGNORE` like the other writers (`gateway_runner.rs`), so a retried call
/// with the same id is a no-op; returns whether a row was inserted.
pub fn insert_routed_approval(conn: &Connection, a: &NewRoutedApproval<'_>) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "INSERT OR IGNORE INTO cowork_approvals
             (id, user_id, content, source, dismissed, approver_user_id, rule_id, bot_id, job_id)
         VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8)",
        params![
            a.id,
            a.user_id,
            a.content,
            a.source,
            a.routing.approver_user_id,
            a.routing.rule_id,
            a.routing.bot_id,
            a.routing.job_id,
        ],
    )?;
    Ok(n > 0)
}

/// Set the routing columns on an existing row. Returns whether it exists.
pub fn set_approval_routing(conn: &Connection, approval_id: &str, routing: &ApprovalRouting) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE cowork_approvals SET approver_user_id = ?2, rule_id = ?3, bot_id = ?4, job_id = ?5 WHERE id = ?1",
        params![approval_id, routing.approver_user_id, routing.rule_id, routing.bot_id, routing.job_id],
    )?;
    Ok(n > 0)
}

pub fn get_approval_routing(conn: &Connection, approval_id: &str) -> rusqlite::Result<Option<ApprovalRouting>> {
    conn.query_row(
        "SELECT approver_user_id, rule_id, bot_id, job_id FROM cowork_approvals WHERE id = ?1",
        params![approval_id],
        |r| {
            Ok(ApprovalRouting {
                approver_user_id: r.get(0)?,
                rule_id: r.get(1)?,
                bot_id: r.get(2)?,
                job_id: r.get(3)?,
            })
        },
    )
    .optional()
}

/// A routed approval with its decision, for the gate (`allternit/approvalId`
/// check) and the approvals read model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutedApproval {
    pub id: String,
    pub user_id: Option<String>,
    pub content: String,
    pub source: String,
    pub dismissed: bool,
    pub decision: Option<String>,
    pub decided_at: Option<String>,
    pub created_at: Option<String>,
    pub routing: ApprovalRouting,
}

const ROUTED_COLS: &str = "id, user_id, content, source, dismissed, decision, decided_at, created_at, \
     approver_user_id, rule_id, bot_id, job_id";

fn routed_from_row(r: &Row<'_>) -> rusqlite::Result<RoutedApproval> {
    Ok(RoutedApproval {
        id: r.get(0)?,
        user_id: r.get(1)?,
        content: r.get(2)?,
        source: r.get(3)?,
        dismissed: r.get::<_, i64>(4)? != 0,
        decision: r.get(5)?,
        decided_at: r.get(6)?,
        created_at: r.get(7)?,
        routing: ApprovalRouting {
            approver_user_id: r.get(8)?,
            rule_id: r.get(9)?,
            bot_id: r.get(10)?,
            job_id: r.get(11)?,
        },
    })
}

pub fn get_routed_approval(conn: &Connection, approval_id: &str) -> rusqlite::Result<Option<RoutedApproval>> {
    conn.query_row(
        &format!("SELECT {ROUTED_COLS} FROM cowork_approvals WHERE id = ?1"),
        params![approval_id],
        routed_from_row,
    )
    .optional()
}

/// Pending asks routed to this approver, newest first.
pub fn list_pending_for_approver(conn: &Connection, approver_user_id: &str) -> rusqlite::Result<Vec<RoutedApproval>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ROUTED_COLS} FROM cowork_approvals
         WHERE approver_user_id = ?1 AND dismissed = 0
         ORDER BY created_at DESC, id"
    ))?;
    let rows = stmt.query_map(params![approver_user_id], routed_from_row)?;
    rows.collect()
}

/// Every ask tied to one bar job (newest first).
pub fn list_approvals_for_job(conn: &Connection, job_id: &str) -> rusqlite::Result<Vec<RoutedApproval>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {ROUTED_COLS} FROM cowork_approvals WHERE job_id = ?1 ORDER BY created_at DESC, id"
    ))?;
    let rows = stmt.query_map(params![job_id], routed_from_row)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;

    fn db() -> Connection {
        DbHandle::new_memory().expect("migrated db").connect().expect("connect")
    }

    /// Lane 10 Verify: V294 applies on main's schema, and a `cowork_approvals`
    /// row written the old way (as `gateway_runner.rs` does) still reads the
    /// same through `list_approvals`' statement, with the new columns NULL.
    #[test]
    fn v294_keeps_old_cowork_approvals_rows_readable() {
        let conn = db();
        conn.execute(
            "INSERT INTO cowork_approvals (id, user_id, content, source, dismissed) VALUES (?1, ?2, ?3, ?4, 0)",
            params!["appr-old", "user-a", "{\"tool\":\"send\"}", "gizzi-permission"],
        )
        .expect("old-shape insert");

        let mut stmt = crate::cowork_routes::prepare_approvals_list_stmt(&conn).expect("prepare");
        let rows: Vec<(String, Option<String>, String, String, i64)> = stmt
            .query_map(params!["user-a", 100, 0], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![(
                "appr-old".to_string(),
                Some("user-a".to_string()),
                "{\"tool\":\"send\"}".to_string(),
                "gizzi-permission".to_string(),
                0
            )]
        );
        assert_eq!(get_approval_routing(&conn, "appr-old").unwrap(), Some(ApprovalRouting::default()));
    }

    #[test]
    fn grant_upsert_keeps_one_live_row_and_history() {
        let conn = db();
        let g1 = upsert_grant(&conn, BotKind::Agent, "bot-1", AppKind::Connector, "gmail", GrantLevel::Browser, "u1")
            .unwrap();
        let same = upsert_grant(&conn, BotKind::Agent, "bot-1", AppKind::Connector, "gmail", GrantLevel::Browser, "u1")
            .unwrap();
        assert_eq!(g1.id, same.id, "same level must not churn rows");
        let g2 = upsert_grant(&conn, BotKind::Agent, "bot-1", AppKind::Connector, "gmail", GrantLevel::Full, "u1")
            .unwrap();
        assert_ne!(g1.id, g2.id);
        assert_eq!(list_grants_for_app(&conn, AppKind::Connector, "gmail").unwrap(), vec![g2.clone()]);
        let total: i64 = conn.query_row("SELECT COUNT(*) FROM bot_app_grants", [], |r| r.get(0)).unwrap();
        assert_eq!(total, 2, "the revoked row stays as history");

        assert!(revoke_grant(&conn, BotKind::Agent, "bot-1", AppKind::Connector, "gmail").unwrap());
        assert!(get_live_grant(&conn, BotKind::Agent, "bot-1", AppKind::Connector, "gmail").unwrap().is_none());
        assert!(list_grants_for_bot(&conn, BotKind::Agent, "bot-1").unwrap().is_empty());
    }

    fn seed_org(conn: &Connection, org: &str, users: &[&str]) {
        conn.execute("INSERT INTO organizations (id, name) VALUES (?1, 'Org')", params![org]).unwrap();
        for u in users {
            conn.execute("INSERT OR IGNORE INTO users (id, email) VALUES (?1, ?1 || '@x.test')", params![u]).unwrap();
            conn.execute(
                "INSERT INTO organization_members (id, organization_id, user_id) VALUES (?1, ?2, ?3)",
                params![format!("m-{u}"), org, u],
            )
            .unwrap();
        }
    }

    #[test]
    fn reports_to_rejects_cycles_and_outsiders() {
        let conn = db();
        seed_org(&conn, "org-1", &["ceo", "lead", "dev"]);
        assert_eq!(set_reports_to(&conn, "org-1", "lead", Some("ceo")).unwrap(), ReportsToOutcome::Set);
        assert_eq!(set_reports_to(&conn, "org-1", "dev", Some("lead")).unwrap(), ReportsToOutcome::Set);
        assert_eq!(set_reports_to(&conn, "org-1", "ceo", Some("dev")).unwrap(), ReportsToOutcome::Cycle);
        assert_eq!(set_reports_to(&conn, "org-1", "dev", Some("dev")).unwrap(), ReportsToOutcome::SelfReport);
        assert_eq!(set_reports_to(&conn, "org-1", "dev", Some("nobody")).unwrap(), ReportsToOutcome::ManagerNotMember);
        assert_eq!(get_reports_to(&conn, "org-1", "ceo").unwrap(), None);
        let dev = list_members(&conn, "org-1").unwrap().into_iter().find(|m| m.user_id == "dev").unwrap();
        assert_eq!(dev.reports_to_user_id.as_deref(), Some("lead"));
    }

    #[test]
    fn rules_for_call_match_scope_app_and_tool() {
        let conn = db();
        let base = RuleInput {
            org_id: Some("org-1".into()),
            owner_user_id: "ceo".into(),
            scope_kind: ScopeKind::Org,
            scope_ref: None,
            app_kind: Some(AppKind::Connector),
            app_ref: Some("stripe".into()),
            tool: "*".into(),
            field: None,
            op: RuleOp::Always,
            value: None,
            unit_label: None,
            approver_kind: ApproverKind::Owner,
            approver_user_id: None,
            enabled: true,
        };
        let org_rule = create_rule(&conn, &base, "ceo").unwrap();
        let bot_rule = create_rule(
            &conn,
            &RuleInput {
                scope_kind: ScopeKind::Bot,
                scope_ref: Some(BotKind::Agent.node_ref("bot-9")),
                tool: "refund".into(),
                field: Some("amount".into()),
                op: RuleOp::Gt,
                value: Some(serde_json::json!(5000)),
                ..base.clone()
            },
            "ceo",
        )
        .unwrap();
        let other_app = create_rule(&conn, &RuleInput { app_ref: Some("gmail".into()), ..base.clone() }, "ceo").unwrap();
        let disabled = create_rule(&conn, &RuleInput { enabled: false, ..base.clone() }, "ceo").unwrap();

        let actor = RuleActor { org_id: Some("org-1"), person_user_id: "dev", bot: Some((BotKind::Agent, "bot-9")) };
        let mut ids: Vec<String> = list_rules_for_call(&conn, &actor, AppKind::Connector, "stripe", "refund")
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.sort();
        let mut want = vec![org_rule.id.clone(), bot_rule.id.clone()];
        want.sort();
        assert_eq!(ids, want);
        assert!(!ids.contains(&other_app.id) && !ids.contains(&disabled.id));

        let other_org = RuleActor { org_id: Some("org-2"), person_user_id: "dev", bot: None };
        assert!(list_rules_for_call(&conn, &other_org, AppKind::Connector, "stripe", "refund").unwrap().is_empty());

        assert_eq!(bot_rule.value, Some(serde_json::json!(5000)));
        let updated = update_rule(&conn, &bot_rule.id, &RuleInput { enabled: false, ..base.clone() }).unwrap().unwrap();
        assert!(!updated.enabled);
        assert!(delete_rule(&conn, &org_rule.id).unwrap());
        assert_eq!(list_rules(&conn, Some("org-1"), "ceo").unwrap().len(), 3);
    }

    #[test]
    fn routed_approval_insert_and_list_for_approver() {
        let conn = db();
        let routing = ApprovalRouting {
            approver_user_id: Some("ceo".into()),
            rule_id: Some("arl_x".into()),
            bot_id: Some("bot-9".into()),
            job_id: Some("bjb_1".into()),
        };
        let row = NewRoutedApproval { id: "appr-1", user_id: Some("dev"), content: "{}", source: "team-rule", routing: &routing };
        assert!(insert_routed_approval(&conn, &row).unwrap());
        assert!(!insert_routed_approval(&conn, &row).unwrap(), "same id is a no-op");
        let pending = list_pending_for_approver(&conn, "ceo").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].routing, routing);
        assert_eq!(list_approvals_for_job(&conn, "bjb_1").unwrap().len(), 1);
        assert!(set_approval_routing(&conn, "appr-1", &ApprovalRouting::default()).unwrap());
        assert!(list_pending_for_approver(&conn, "ceo").unwrap().is_empty());
        assert!(get_routed_approval(&conn, "appr-1").unwrap().is_some());
    }
}
