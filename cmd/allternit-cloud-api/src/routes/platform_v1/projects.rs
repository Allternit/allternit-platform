//! Developer projects: the unit that owns keys, accounts, agents and numbers.
//!
//! Access rule (console routes): the project's `owner_user_id`, or any Clerk
//! org admin of the project's `org_id`. A project someone else owns is always
//! a 404.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{FromRow, PgPool};

use super::{new_id, PlatformError};

pub const MAX_PROJECTS_PER_OWNER: i64 = 25;
pub const MAX_SPEND_CAP_CENTS: i64 = 100_000_000;

/// Who is asking, from the Clerk session.
#[derive(Debug, Clone)]
pub struct Principal {
    pub user_id: String,
    pub org_id: Option<String>,
    pub org_admin: bool,
}

#[derive(Debug, Clone, Serialize, FromRow)]
pub struct Project {
    pub id: String,
    pub object: String,
    pub owner_user_id: String,
    pub org_id: Option<String>,
    pub name: String,
    pub env: String,
    pub plan: String,
    pub spend_cap_cents: i64,
    pub rpm_override: Option<i32>,
    pub call_cap_override: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, 'project'::text AS object, owner_user_id, org_id, name, env, plan, \
    spend_cap_cents, rpm_override, call_cap_override, created_at, archived_at";

/// SQL predicate on `platform_projects` for "this principal may manage it"
/// ($1 = user id, $2 = org id or null, $3 = org admin).
const ACCESS: &str = "(owner_user_id = $1 OR (org_id IS NOT NULL AND org_id = $2 AND $3))";

fn not_found() -> PlatformError {
    PlatformError::not_found("project_not_found", "No such project.")
}

pub fn validate_project_name(name: &str) -> Result<String, PlatformError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(PlatformError::invalid_request(
            "invalid_name",
            "name must be 1 to 120 characters.",
        )
        .with_param("name"));
    }
    Ok(name.to_string())
}

pub async fn create_project(
    db: &PgPool,
    who: &Principal,
    name: &str,
    env: super::ProjectEnv,
) -> Result<Project, PlatformError> {
    let name = validate_project_name(name)?;
    let owned: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM platform_projects WHERE owner_user_id = $1 AND archived_at IS NULL",
    )
    .bind(&who.user_id)
    .fetch_one(db)
    .await?;
    if owned >= MAX_PROJECTS_PER_OWNER {
        return Err(PlatformError::permission(
            "project_limit",
            format!("You can have at most {MAX_PROJECTS_PER_OWNER} active projects. Archive one first."),
        ));
    }
    Ok(sqlx::query_as::<_, Project>(&format!(
        "INSERT INTO platform_projects (id, owner_user_id, org_id, name, env) \
         VALUES ($1, $2, $3, $4, $5) RETURNING {COLUMNS}"
    ))
    .bind(new_id("proj_"))
    .bind(&who.user_id)
    .bind(&who.org_id)
    .bind(&name)
    .bind(env.as_str())
    .fetch_one(db)
    .await?)
}

/// Projects the principal can manage (archived ones excluded), oldest first.
/// Returns up to `limit + 1` rows so the caller can build a page.
pub async fn list_projects(
    db: &PgPool,
    who: &Principal,
    after: Option<(DateTime<Utc>, String)>,
    limit: i64,
) -> Result<Vec<Project>, PlatformError> {
    let (after_at, after_id) = match after {
        Some((at, id)) => (Some(at), Some(id)),
        None => (None, None),
    };
    Ok(sqlx::query_as::<_, Project>(&format!(
        "SELECT {COLUMNS} FROM platform_projects \
         WHERE {ACCESS} AND archived_at IS NULL \
           AND ($4::timestamptz IS NULL OR (created_at, id) > ($4, $5)) \
         ORDER BY created_at, id LIMIT $6"
    ))
    .bind(&who.user_id)
    .bind(&who.org_id)
    .bind(who.org_admin)
    .bind(after_at)
    .bind(after_id)
    .bind(limit + 1)
    .fetch_all(db)
    .await?)
}

/// Fetch one project the principal may manage; 404 otherwise. Archived
/// projects stay readable so the console can show them.
pub async fn get_project(db: &PgPool, who: &Principal, id: &str) -> Result<Project, PlatformError> {
    sqlx::query_as::<_, Project>(&format!(
        "SELECT {COLUMNS} FROM platform_projects WHERE id = $4 AND {ACCESS}"
    ))
    .bind(&who.user_id)
    .bind(&who.org_id)
    .bind(who.org_admin)
    .bind(id)
    .fetch_optional(db)
    .await?
    .ok_or_else(not_found)
}

pub struct ProjectPatch {
    pub name: Option<String>,
    pub spend_cap_cents: Option<i64>,
    pub archived: Option<bool>,
}

pub async fn update_project(
    db: &PgPool,
    who: &Principal,
    id: &str,
    patch: ProjectPatch,
) -> Result<Project, PlatformError> {
    let current = get_project(db, who, id).await?;
    let name = match patch.name.as_deref() {
        Some(n) => validate_project_name(n)?,
        None => current.name,
    };
    let spend_cap = match patch.spend_cap_cents {
        Some(c) if !(0..=MAX_SPEND_CAP_CENTS).contains(&c) => {
            return Err(PlatformError::invalid_request(
                "invalid_spend_cap",
                "spend_cap_cents must be between 0 and 100000000.",
            )
            .with_param("spend_cap_cents"))
        }
        Some(c) => c,
        None => current.spend_cap_cents,
    };
    let archived_at = match patch.archived {
        Some(true) => Some(current.archived_at.unwrap_or_else(Utc::now)),
        Some(false) => None,
        None => current.archived_at,
    };
    let newly_archived = patch.archived == Some(true) && current.archived_at.is_none();

    let mut tx = db.begin().await?;
    let project = sqlx::query_as::<_, Project>(&format!(
        "UPDATE platform_projects SET name = $4, spend_cap_cents = $5, archived_at = $6 \
         WHERE id = $7 AND {ACCESS} RETURNING {COLUMNS}"
    ))
    .bind(&who.user_id)
    .bind(&who.org_id)
    .bind(who.org_admin)
    .bind(&name)
    .bind(spend_cap)
    .bind(archived_at)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(not_found)?;
    if newly_archived {
        // Archived projects already fail key auth (the lookup requires
        // archived_at IS NULL); revoking makes it permanent across un-archive.
        sqlx::query(
            "UPDATE api_keys SET revoked_at = NOW(), updated_at = NOW() \
             WHERE project_id = $1 AND revoked_at IS NULL",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(project)
}

/// `account_id` must be a live account of `project_id`.
pub async fn account_in_project(
    db: &PgPool,
    project_id: &str,
    account_id: &str,
) -> Result<(), PlatformError> {
    let found: Option<String> = sqlx::query_scalar(
        "SELECT id FROM platform_accounts WHERE id = $1 AND project_id = $2 AND deleted_at IS NULL",
    )
    .bind(account_id)
    .bind(project_id)
    .fetch_optional(db)
    .await?;
    found.map(|_| ()).ok_or_else(|| {
        PlatformError::invalid_request("account_not_found", "No such account in this project.")
            .with_param("account_id")
    })
}
