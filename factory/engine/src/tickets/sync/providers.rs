//! External tracker provider implementations for the Rails sync framework.

use std::collections::HashMap;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::Utc;

use crate::rails_id::TicketId;
use crate::tickets::{Ticket, TicketKind, TicketPriority, TicketStatus, TicketStore, TicketUpdate};

use super::{SyncResult, TrackerConfig};

/// Trait implemented by external tracker providers.
pub trait TrackerProvider: Send + Sync {
    fn name(&self) -> &'static str;

    /// Pull issues from the external tracker into Rails tickets.
    fn pull(&self, root: &std::path::Path) -> Result<SyncResult>;

    /// Push Rails tickets to the external tracker.
    fn push(&self, root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult>;

    /// Check connectivity and configuration.
    fn status(&self) -> Result<TrackerStatus>;
}

/// Status of a tracker provider.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TrackerStatus {
    pub provider: String,
    pub configured: bool,
    pub connected: bool,
    pub message: String,
}

fn require_config(config: &TrackerConfig) -> Result<()> {
    if !config.enabled {
        anyhow::bail!("sync is not enabled");
    }
    if config.token.is_none() {
        anyhow::bail!("token not configured");
    }
    Ok(())
}

fn repo_config(config: &TrackerConfig) -> Result<(String, String)> {
    let owner = config.owner.clone().context("missing owner")?;
    let repo = config.repo.clone().context("missing repo")?;
    Ok((owner, repo))
}

// ---------- GitHub ----------

pub struct GithubTracker {
    config: TrackerConfig,
}

impl GithubTracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent("rails-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")
    }

    fn auth_header(&self) -> Result<String> {
        let token = self.config.token.clone().context("missing token")?;
        Ok(format!("Bearer {token}"))
    }
}

impl TrackerProvider for GithubTracker {
    fn name(&self) -> &'static str {
        "github"
    }

    fn pull(&self, root: &std::path::Path) -> Result<SyncResult> {
        require_config(&self.config)?;
        let (owner, repo) = repo_config(&self.config)?;
        let client = self.client()?;
        let url = format!("https://api.github.com/repos/{owner}/{repo}/issues?state=all&per_page=100");

        let response = client
            .get(&url)
            .header("Authorization", self.auth_header()?)
            .header("Accept", "application/vnd.github+json")
            .send()
            .context("failed to fetch GitHub issues")?;

        if !response.status().is_success() {
            anyhow::bail!("GitHub API error: {}", response.status());
        }

        let issues: Vec<GithubIssue> = response.json().context("failed to parse GitHub issues")?;
        let store = TicketStore::new(root)?;
        let mut created = 0;
        let mut updated = 0;

        for issue in &issues {
            if issue.pull_request.is_some() {
                continue; // skip PRs
            }
            let external_ref = format!("github:{}/{}/#{}", owner, repo, issue.number);
            let existing = store.list()?.into_iter().find(|t| {
                t.external_ref.as_ref() == Some(&external_ref)
            });

            let priority = map_github_labels_to_priority(&issue.labels);
            let kind = map_github_labels_to_kind(&issue.labels);
            let status = if issue.state == "closed" {
                TicketStatus::Closed
            } else {
                TicketStatus::Open
            };

            if let Some(existing) = existing {
                let mut update = TicketUpdate::default();
                if existing.title != issue.title {
                    update.title = Some(issue.title.clone());
                }
                if existing.description != issue.body.clone().unwrap_or_default() {
                    update.description = Some(issue.body.clone().unwrap_or_default());
                }
                if existing.status != status {
                    store.set_status(&existing.id, status, "github-sync", None)?;
                }
                if update.title.is_some() || update.description.is_some() {
                    store.update(&existing.id, update)?;
                }
                updated += 1;
            } else {
                let id = TicketId::mint(format!("{}:{}", external_ref, Utc::now()).as_bytes());
                let ticket = Ticket {
                    id,
                    hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(issue.title.as_bytes())),
                    title: issue.title.clone(),
                    description: issue.body.clone().unwrap_or_default(),
                    design: None,
                    acceptance: None,
                    notes: Vec::new(),
                    status,
                    kind,
                    priority,
                    assignee: issue.assignee.as_ref().map(|a| a.login.clone()),
                    estimate_minutes: None,
                    due_at: None,
                    defer_until: None,
                    labels: issue.labels.iter().map(|l| l.name.clone()).collect(),
                    external_ref: Some(external_ref),
                    metadata: HashMap::new(),
                    created_at: issue.created_at.as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now),
                    updated_at: issue.updated_at.as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(Utc::now),
                    closed_at: issue.closed_at.as_deref()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .map(|d| d.with_timezone(&Utc)),
                    close_reason: None,
                };
                store.create(ticket)?;
                created += 1;
            }
        }

        Ok(SyncResult {
            provider: "github".to_string(),
            direction: "pull".to_string(),
            created,
            updated,
            unchanged: issues.len() - created - updated,
            errors: Vec::new(),
            message: Some(format!("synced {} GitHub issues", issues.len())),
        })
    }

    fn push(&self, _root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult> {
        require_config(&self.config)?;
        let (owner, repo) = repo_config(&self.config)?;
        let client = self.client()?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for ticket in tickets {
            if ticket.status == TicketStatus::Closed {
                continue;
            }

            match &ticket.external_ref {
                Some(external_ref) if external_ref.starts_with("github:") => {
                    let number = external_ref.split('#').last().and_then(|s| s.parse::<u64>().ok());
                    if let Some(number) = number {
                        let url = format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}");
                        let body = serde_json::json!({
                            "title": ticket.title,
                            "body": ticket.description,
                            "state": "open",
                        });
                        let response = client
                            .patch(&url)
                            .header("Authorization", self.auth_header()?)
                            .header("Accept", "application/vnd.github+json")
                            .json(&body)
                            .send();
                        match response {
                            Ok(r) if r.status().is_success() => updated += 1,
                            Ok(r) => errors.push(format!("update {} failed: {}", ticket.id, r.status())),
                            Err(e) => errors.push(format!("update {} failed: {}", ticket.id, e)),
                        }
                    }
                }
                _ => {
                    let url = format!("https://api.github.com/repos/{owner}/{repo}/issues");
                    let body = serde_json::json!({
                        "title": ticket.title,
                        "body": ticket.description,
                    });
                    let response = client
                        .post(&url)
                        .header("Authorization", self.auth_header()?)
                        .header("Accept", "application/vnd.github+json")
                        .json(&body)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => created += 1,
                        Ok(r) => errors.push(format!("create {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("create {} failed: {}", ticket.id, e)),
                    }
                }
            }
        }

        Ok(SyncResult {
            provider: "github".to_string(),
            direction: "push".to_string(),
            created,
            updated,
            unchanged: tickets.len() - created - updated - errors.len(),
            errors,
            message: Some(format!("pushed {created} created, {updated} updated")),
        })
    }

    fn status(&self) -> Result<TrackerStatus> {
        let configured = self.config.token.is_some() && self.config.owner.is_some() && self.config.repo.is_some();
        let mut connected = false;
        let mut message = "not configured".to_string();
        if configured {
            match self.client()?.get("https://api.github.com/rate_limit")
                .header("Authorization", self.auth_header()?)
                .send()
            {
                Ok(r) => {
                    connected = r.status().is_success();
                    message = format!("GitHub API response: {}", r.status());
                }
                Err(e) => message = format!("GitHub API connection failed: {e}"),
            }
        }
        Ok(TrackerStatus {
            provider: "github".to_string(),
            configured,
            connected,
            message,
        })
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GithubIssue {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    #[serde(default)]
    labels: Vec<GithubLabel>,
    assignee: Option<GithubUser>,
    #[serde(default)]
    pull_request: Option<serde_json::Value>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GithubLabel {
    name: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GithubUser {
    login: String,
}

fn map_github_labels_to_priority(labels: &[GithubLabel]) -> TicketPriority {
    for label in labels {
        match label.name.to_lowercase().as_str() {
            "p0" | "priority/p0" | "critical" => return TicketPriority::P0,
            "p1" | "priority/p1" | "high" => return TicketPriority::P1,
            "p2" | "priority/p2" | "medium" => return TicketPriority::P2,
            "p3" | "priority/p3" | "low" => return TicketPriority::P3,
            "p4" | "priority/p4" => return TicketPriority::P4,
            _ => {}
        }
    }
    TicketPriority::P2
}

fn map_github_labels_to_kind(labels: &[GithubLabel]) -> TicketKind {
    for label in labels {
        match label.name.to_lowercase().as_str() {
            "bug" | "kind/bug" => return TicketKind::Bug,
            "feature" | "kind/feature" | "enhancement" => return TicketKind::Feature,
            "epic" | "kind/epic" => return TicketKind::Epic,
            "chore" | "kind/chore" => return TicketKind::Chore,
            _ => {}
        }
    }
    TicketKind::Task
}

// ---------- Stubs for remaining providers ----------


// ---------- Linear ----------

pub struct LinearTracker {
    config: TrackerConfig,
}

impl LinearTracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent("rails-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")
    }

    fn token(&self) -> Result<String> {
        self.config.token.clone().context("missing Linear token")
    }

    fn graphql(&self, query: &str, variables: Option<serde_json::Value>) -> Result<serde_json::Value> {
        let body = serde_json::json!({ "query": query, "variables": variables.unwrap_or_default() });
        let response = self.client()?
            .post("https://api.linear.app/graphql")
            .header("Authorization", self.token()?)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .context("failed to call Linear API")?;

        if !response.status().is_success() {
            anyhow::bail!("Linear API error: {}", response.status());
        }

        let json: serde_json::Value = response.json().context("failed to parse Linear response")?;
        if let Some(errors) = json.get("errors") {
            anyhow::bail!("Linear GraphQL errors: {}", errors);
        }
        Ok(json)
    }
}

impl TrackerProvider for LinearTracker {
    fn name(&self) -> &'static str {
        "linear"
    }

    fn pull(&self, root: &std::path::Path) -> Result<SyncResult> {
        require_config(&self.config)?;
        let query = r#"
            query Issues {
                issues(first: 100) {
                    nodes {
                        id
                        identifier
                        title
                        description
                        state { name }
                        createdAt
                        updatedAt
                        completedAt
                        canceledAt
                        labels { nodes { name } }
                        assignee { name }
                    }
                }
            }
        "#;

        let response = self.graphql(query, None)?;
        let nodes = response["data"]["issues"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let store = TicketStore::new(root)?;
        let mut created = 0;
        let mut updated = 0;

        for node in &nodes {
            let linear_id = node["id"].as_str().unwrap_or("").to_string();
            let identifier = node["identifier"].as_str().unwrap_or("").to_string();
            let title = node["title"].as_str().unwrap_or("").to_string();
            let description = node["description"].as_str().unwrap_or("").to_string();
            let state = node["state"]["name"].as_str().unwrap_or("").to_lowercase();
            let external_ref = format!("linear:{identifier}");

            let labels: Vec<String> = node["labels"]["nodes"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|l| l["name"].as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            let status = if state == "canceled" || state == "done" || state == "completed" {
                TicketStatus::Closed
            } else {
                TicketStatus::Open
            };

            let existing = store.list()?.into_iter().find(|t| {
                t.external_ref.as_ref() == Some(&external_ref)
            });

            if let Some(existing) = existing {
                let mut update = TicketUpdate::default();
                if existing.title != title {
                    update.title = Some(title.clone());
                }
                if existing.description != description {
                    update.description = Some(description.clone());
                }
                if existing.status != status {
                    store.set_status(&existing.id, status, "linear-sync", None)?;
                }
                if update.title.is_some() || update.description.is_some() {
                    store.update(&existing.id, update)?;
                }
                updated += 1;
            } else {
                let parse_dt = |s: &serde_json::Value| {
                    s.as_str()
                        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                        .map(|d| d.with_timezone(&Utc))
                };
                let id = TicketId::mint(format!("{}:{}", external_ref, Utc::now()).as_bytes());
                let ticket = Ticket {
                    id,
                    hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(title.as_bytes())),
                    title,
                    description,
                    design: None,
                    acceptance: None,
                    notes: Vec::new(),
                    status,
                    kind: map_linear_labels_to_kind(&labels),
                    priority: map_linear_labels_to_priority(&labels),
                    assignee: node["assignee"]["name"].as_str().map(|s| s.to_string()),
                    estimate_minutes: None,
                    due_at: None,
                    defer_until: None,
                    labels,
                    external_ref: Some(external_ref),
                    metadata: {
                        let mut m = HashMap::new();
                        m.insert("linear_id".to_string(), serde_json::Value::String(linear_id));
                        m
                    },
                    created_at: parse_dt(&node["createdAt"]).unwrap_or_else(Utc::now),
                    updated_at: parse_dt(&node["updatedAt"]).unwrap_or_else(Utc::now),
                    closed_at: parse_dt(&node["completedAt"]).or_else(|| parse_dt(&node["canceledAt"])),
                    close_reason: None,
                };
                store.create(ticket)?;
                created += 1;
            }
        }

        Ok(SyncResult {
            provider: "linear".to_string(),
            direction: "pull".to_string(),
            created,
            updated,
            unchanged: nodes.len() - created - updated,
            errors: Vec::new(),
            message: Some(format!("synced {} Linear issues", nodes.len())),
        })
    }

    fn push(&self, _root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult> {
        require_config(&self.config)?;
        let mut created = 0;
        let mut errors = Vec::new();

        for ticket in tickets {
            if ticket.status == TicketStatus::Closed {
                continue;
            }

            let query = r#"
                mutation IssueCreate($title: String!, $description: String) {
                    issueCreate(input: { title: $title, description: $description }) {
                        issue { id identifier url }
                        success
                    }
                }
            "#;
            let variables = serde_json::json!({
                "title": ticket.title,
                "description": ticket.description,
            });

            match self.graphql(query, Some(variables)) {
                Ok(response) => {
                    if response["data"]["issueCreate"]["success"].as_bool() == Some(true) {
                        created += 1;
                    } else {
                        errors.push(format!("create {} failed: {:?}", ticket.id, response));
                    }
                }
                Err(e) => errors.push(format!("create {} failed: {}", ticket.id, e)),
            }
        }

        let updated = 0;
        Ok(SyncResult {
            provider: "linear".to_string(),
            direction: "push".to_string(),
            created,
            updated,
            unchanged: tickets.len() - created - updated - errors.len(),
            errors,
            message: Some(format!("pushed {created} Linear issues")),
        })
    }

    fn status(&self) -> Result<TrackerStatus> {
        let configured = self.config.token.is_some();
        let mut connected = false;
        let mut message = "not configured".to_string();
        if configured {
            let query = "query Viewer { viewer { id name } }";
            match self.graphql(query, None) {
                Ok(response) => {
                    connected = response["data"]["viewer"]["id"].as_str().is_some();
                    message = format!("Linear viewer: {:?}", response["data"]["viewer"]["name"]);
                }
                Err(e) => message = format!("Linear API connection failed: {e}"),
            }
        }
        Ok(TrackerStatus {
            provider: "linear".to_string(),
            configured,
            connected,
            message,
        })
    }
}

fn map_linear_labels_to_kind(labels: &[String]) -> TicketKind {
    for label in labels {
        match label.to_lowercase().as_str() {
            "bug" => return TicketKind::Bug,
            "feature" | "enhancement" => return TicketKind::Feature,
            "epic" => return TicketKind::Epic,
            "chore" => return TicketKind::Chore,
            _ => {}
        }
    }
    TicketKind::Task
}

fn map_linear_labels_to_priority(labels: &[String]) -> TicketPriority {
    for label in labels {
        match label.to_lowercase().as_str() {
            "p0" | "critical" => return TicketPriority::P0,
            "p1" | "high" => return TicketPriority::P1,
            "p2" | "medium" => return TicketPriority::P2,
            "p3" | "low" => return TicketPriority::P3,
            "p4" => return TicketPriority::P4,
            _ => {}
        }
    }
    TicketPriority::P2
}

// ---------- Stubs for remaining providers ----------

// ---------- Jira ----------

pub struct JiraTracker {
    config: TrackerConfig,
}

impl JiraTracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent("rails-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")
    }

    fn base_url(&self) -> Result<String> {
        self.config.base_url.clone().context("missing Jira base_url")
    }

    fn auth_header(&self) -> Result<String> {
        let token = self.config.token.clone().context("missing Jira token")?;
        let user = self.config.owner.clone().unwrap_or_default();
        let creds = format!("{user}:{token}");
        Ok(format!("Basic {}", BASE64.encode(creds)))
    }

    fn get_json(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{}{path}", self.base_url()?);
        let response = self.client()?
            .get(&url)
            .header("Authorization", self.auth_header()?)
            .header("Accept", "application/json")
            .send()
            .context("failed to call Jira API")?;
        if !response.status().is_success() {
            anyhow::bail!("Jira API error: {}", response.status());
        }
        response.json().context("failed to parse Jira response")
    }
}

impl TrackerProvider for JiraTracker {
    fn name(&self) -> &'static str {
        "jira"
    }

    fn pull(&self, root: &std::path::Path) -> Result<SyncResult> {
        require_config(&self.config)?;
        let store = TicketStore::new(root)?;
        let mut created = 0;
        let mut updated = 0;

        let jql = self.config.extra.get("jql").and_then(|v| v.as_str()).unwrap_or("order by created DESC");
        let json = self.get_json(&format!("/rest/api/3/search?jql={}&maxResults=100", urlencoding::encode(jql)))?;
        let issues = json["issues"].as_array().cloned().unwrap_or_default();

        for issue in &issues {
            let key = issue["key"].as_str().unwrap_or("").to_string();
            let external_ref = format!("jira:{key}");
            let fields = &issue["fields"];
            let summary = fields["summary"].as_str().unwrap_or("").to_string();
            let description = extract_jira_text(&fields["description"]);
            let status_name = fields["status"]["name"].as_str().unwrap_or("").to_lowercase();
            let status = if status_name == "done" || status_name == "closed" || status_name == "resolved" {
                TicketStatus::Closed
            } else {
                TicketStatus::Open
            };

            let priority = fields["priority"]["name"].as_str().unwrap_or("").to_lowercase();
            let priority = match priority.as_str() {
                "highest" => TicketPriority::P0,
                "high" => TicketPriority::P1,
                "medium" => TicketPriority::P2,
                "low" => TicketPriority::P3,
                "lowest" => TicketPriority::P4,
                _ => TicketPriority::P2,
            };

            let labels: Vec<String> = fields["labels"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default();

            let existing = store.list()?.into_iter().find(|t| t.external_ref.as_ref() == Some(&external_ref));

            if let Some(existing) = existing {
                let mut update = TicketUpdate::default();
                if existing.title != summary {
                    update.title = Some(summary.clone());
                }
                if existing.description != description {
                    update.description = Some(description.clone());
                }
                if existing.status != status {
                    store.set_status(&existing.id, status, "jira-sync", None)?;
                }
                if update.title.is_some() || update.description.is_some() {
                    store.update(&existing.id, update)?;
                }
                updated += 1;
            } else {
                let id = TicketId::mint(format!("{}:{}", external_ref, Utc::now()).as_bytes());
                let ticket = Ticket {
                    id,
                    hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(summary.as_bytes())),
                    title: summary,
                    description,
                    design: None,
                    acceptance: None,
                    notes: Vec::new(),
                    status,
                    kind: map_jira_labels_to_kind(&labels),
                    priority,
                    assignee: fields["assignee"]["displayName"].as_str().map(|s| s.to_string()),
                    estimate_minutes: None,
                    due_at: None,
                    defer_until: None,
                    labels,
                    external_ref: Some(external_ref),
                    metadata: HashMap::new(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    closed_at: None,
                    close_reason: None,
                };
                store.create(ticket)?;
                created += 1;
            }
        }

        Ok(SyncResult {
            provider: "jira".to_string(),
            direction: "pull".to_string(),
            created,
            updated,
            unchanged: issues.len() - created - updated,
            errors: Vec::new(),
            message: Some(format!("synced {} Jira issues", issues.len())),
        })
    }

    fn push(&self, _root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult> {
        require_config(&self.config)?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for ticket in tickets {
            if ticket.status == TicketStatus::Closed {
                continue;
            }

            let body = serde_json::json!({
                "fields": {
                    "project": { "key": self.config.project.clone().unwrap_or_default() },
                    "summary": ticket.title,
                    "description": {
                        "type": "doc",
                        "version": 1,
                        "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": ticket.description }] }]
                    },
                    "issuetype": { "name": "Task" }
                }
            });

            match &ticket.external_ref {
                Some(external_ref) if external_ref.starts_with("jira:") => {
                    let key = external_ref.trim_start_matches("jira:");
                    let url = format!("{}/rest/api/3/issue/{key}", self.base_url()?);
                    let response = self.client()?
                        .put(&url)
                        .header("Authorization", self.auth_header()?)
                        .header("Accept", "application/json")
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => updated += 1,
                        Ok(r) => errors.push(format!("update {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("update {} failed: {}", ticket.id, e)),
                    }
                }
                _ => {
                    let url = format!("{}/rest/api/3/issue", self.base_url()?);
                    let response = self.client()?
                        .post(&url)
                        .header("Authorization", self.auth_header()?)
                        .header("Accept", "application/json")
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => created += 1,
                        Ok(r) => errors.push(format!("create {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("create {} failed: {}", ticket.id, e)),
                    }
                }
            }
        }

        Ok(SyncResult {
            provider: "jira".to_string(),
            direction: "push".to_string(),
            created,
            updated,
            unchanged: tickets.len() - created - updated - errors.len(),
            errors,
            message: Some(format!("pushed {created} created, {updated} updated")),
        })
    }

    fn status(&self) -> Result<TrackerStatus> {
        let configured = self.config.token.is_some() && self.config.base_url.is_some();
        let mut connected = false;
        let mut message = "not configured".to_string();
        if configured {
            match self.get_json("/rest/api/3/myself") {
                Ok(json) => {
                    connected = json["accountId"].as_str().is_some();
                    message = format!("Jira user: {:?}", json["displayName"]);
                }
                Err(e) => message = format!("Jira API connection failed: {e}"),
            }
        }
        Ok(TrackerStatus {
            provider: "jira".to_string(),
            configured,
            connected,
            message,
        })
    }
}

fn extract_jira_text(value: &serde_json::Value) -> String {
    // Simplified extraction of plain text from Atlassian Document Format.
    let mut out = String::new();
    if let Some(content) = value["content"].as_array() {
        for node in content {
            if let Some(children) = node["content"].as_array() {
                for child in children {
                    if let Some(text) = child["text"].as_str() {
                        out.push_str(text);
                    }
                }
                out.push('\n');
            }
        }
    }
    out.trim().to_string()
}

fn map_jira_labels_to_kind(labels: &[String]) -> TicketKind {
    for label in labels {
        match label.to_lowercase().as_str() {
            "bug" => return TicketKind::Bug,
            "feature" | "enhancement" => return TicketKind::Feature,
            "epic" => return TicketKind::Epic,
            "chore" => return TicketKind::Chore,
            _ => {}
        }
    }
    TicketKind::Task
}

// ---------- ADO ----------

pub struct AdoTracker {
    config: TrackerConfig,
}

impl AdoTracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent("rails-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")
    }

    fn base_url(&self) -> Result<String> {
        self.config.base_url.clone().context("missing ADO base_url")
    }

    fn auth_header(&self) -> Result<String> {
        let token = self.config.token.clone().context("missing ADO token")?;
        let creds = format!(":{token}");
        Ok(format!("Basic {}", BASE64.encode(creds)))
    }

    fn organization(&self) -> Result<String> {
        self.config.owner.clone().context("missing ADO organization (owner)")
    }

    fn project(&self) -> Result<String> {
        self.config.project.clone().context("missing ADO project")
    }
}

impl TrackerProvider for AdoTracker {
    fn name(&self) -> &'static str {
        "ado"
    }

    fn pull(&self, root: &std::path::Path) -> Result<SyncResult> {
        require_config(&self.config)?;
        let org = self.organization()?;
        let project = self.project()?;
        let url = format!("{}/{org}/{project}/_apis/wit/wiql?api-version=7.1-preview.2", self.base_url()?);
        let body = serde_json::json!({ "query": "SELECT [System.Id], [System.Title], [System.State], [System.Description] FROM workitems" });

        let response = self.client()?
            .post(&url)
            .header("Authorization", self.auth_header()?)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .context("failed to query ADO")?;

        if !response.status().is_success() {
            anyhow::bail!("ADO API error: {}", response.status());
        }

        let json: serde_json::Value = response.json().context("failed to parse ADO response")?;
        let work_items = json["workItems"].as_array().cloned().unwrap_or_default();
        let store = TicketStore::new(root)?;
        let mut created = 0;
        let mut updated = 0;

        // Batch fetch details in chunks of 50 IDs.
        for chunk in work_items.chunks(50) {
            let ids: Vec<String> = chunk.iter().filter_map(|w| w["id"].as_u64().map(|id| id.to_string())).collect();
            if ids.is_empty() {
                continue;
            }
            let ids_param = ids.join(",");
            let detail_url = format!("{}/{org}/{project}/_apis/wit/workitems?ids={ids_param}&$expand=all&api-version=7.1-preview.2", self.base_url()?);
            let detail_response = self.client()?
                .get(&detail_url)
                .header("Authorization", self.auth_header()?)
                .send()
                .context("failed to fetch ADO work items")?;
            if !detail_response.status().is_success() {
                anyhow::bail!("ADO API error: {}", detail_response.status());
            }
            let detail_json: serde_json::Value = detail_response.json()?;
            for item in detail_json["value"].as_array().cloned().unwrap_or_default() {
                let id = item["id"].as_u64().unwrap_or(0);
                let fields = &item["fields"];
                let title = fields["System.Title"].as_str().unwrap_or("").to_string();
                let state = fields["System.State"].as_str().unwrap_or("").to_lowercase();
                let description = fields["System.Description"].as_str().unwrap_or("").to_string();
                let external_ref = format!("ado:{id}");

                let status = if state == "closed" || state == "resolved" || state == "done" {
                    TicketStatus::Closed
                } else {
                    TicketStatus::Open
                };

                let existing = store.list()?.into_iter().find(|t| t.external_ref.as_ref() == Some(&external_ref));
                if let Some(existing) = existing {
                    let mut update = TicketUpdate::default();
                    if existing.title != title {
                        update.title = Some(title.clone());
                    }
                    if existing.description != description {
                        update.description = Some(description.clone());
                    }
                    if existing.status != status {
                        store.set_status(&existing.id, status, "ado-sync", None)?;
                    }
                    if update.title.is_some() || update.description.is_some() {
                        store.update(&existing.id, update)?;
                    }
                    updated += 1;
                } else {
                    let ticket_id = TicketId::mint(format!("{}:{}", external_ref, Utc::now()).as_bytes());
                    let ticket = Ticket {
                        id: ticket_id,
                        hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(title.as_bytes())),
                        title,
                        description,
                        design: None,
                        acceptance: None,
                        notes: Vec::new(),
                        status,
                        kind: TicketKind::Task,
                        priority: TicketPriority::P2,
                        assignee: None,
                        estimate_minutes: None,
                        due_at: None,
                        defer_until: None,
                        labels: Vec::new(),
                        external_ref: Some(external_ref),
                        metadata: HashMap::new(),
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                        closed_at: None,
                        close_reason: None,
                    };
                    store.create(ticket)?;
                    created += 1;
                }
            }
        }

        Ok(SyncResult {
            provider: "ado".to_string(),
            direction: "pull".to_string(),
            created,
            updated,
            unchanged: work_items.len() - created - updated,
            errors: Vec::new(),
            message: Some(format!("synced {} ADO work items", work_items.len())),
        })
    }

    fn push(&self, _root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult> {
        require_config(&self.config)?;
        let org = self.organization()?;
        let project = self.project()?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for ticket in tickets {
            if ticket.status == TicketStatus::Closed {
                continue;
            }

            let ops = serde_json::json!([
                { "op": "add", "path": "/fields/System.Title", "from": null, "value": ticket.title },
                { "op": "add", "path": "/fields/System.Description", "from": null, "value": ticket.description }
            ]);

            match &ticket.external_ref {
                Some(external_ref) if external_ref.starts_with("ado:") => {
                    let id = external_ref.trim_start_matches("ado:");
                    let url = format!("{}/{org}/{project}/_apis/wit/workitems/{id}?api-version=7.1-preview.2", self.base_url()?);
                    let response = self.client()?
                        .patch(&url)
                        .header("Authorization", self.auth_header()?)
                        .header("Content-Type", "application/json-patch+json")
                        .json(&ops)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => updated += 1,
                        Ok(r) => errors.push(format!("update {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("update {} failed: {}", ticket.id, e)),
                    }
                }
                _ => {
                    let url = format!("{}/{org}/{project}/_apis/wit/workitems/$Task?api-version=7.1-preview.2", self.base_url()?);
                    let response = self.client()?
                        .post(&url)
                        .header("Authorization", self.auth_header()?)
                        .header("Content-Type", "application/json-patch+json")
                        .json(&ops)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => created += 1,
                        Ok(r) => errors.push(format!("create {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("create {} failed: {}", ticket.id, e)),
                    }
                }
            }
        }

        Ok(SyncResult {
            provider: "ado".to_string(),
            direction: "push".to_string(),
            created,
            updated,
            unchanged: tickets.len() - created - updated - errors.len(),
            errors,
            message: Some(format!("pushed {created} created, {updated} updated")),
        })
    }

    fn status(&self) -> Result<TrackerStatus> {
        let configured = self.config.token.is_some() && self.config.base_url.is_some() && self.config.owner.is_some() && self.config.project.is_some();
        let mut connected = false;
        let mut message = "not configured".to_string();
        if configured {
            let org = self.organization()?;
            let url = format!("{}/{org}/_apis/projects?api-version=7.1-preview.4", self.base_url()?);
            match self.client()?.get(&url).header("Authorization", self.auth_header()?).send() {
                Ok(r) => {
                    connected = r.status().is_success();
                    message = format!("ADO API response: {}", r.status());
                }
                Err(e) => message = format!("ADO API connection failed: {e}"),
            }
        }
        Ok(TrackerStatus {
            provider: "ado".to_string(),
            configured,
            connected,
            message,
        })
    }
}

// ---------- GitLab ----------

pub struct GitlabTracker {
    config: TrackerConfig,
}

impl GitlabTracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent("rails-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")
    }

    fn base_url(&self) -> String {
        self.config.base_url.clone().unwrap_or_else(|| "https://gitlab.com".to_string())
    }

    fn project_id(&self) -> Result<String> {
        self.config.project.clone().context("missing GitLab project id")
    }

    fn auth_header(&self) -> Result<String> {
        self.config.token.clone().context("missing GitLab token")
    }
}

impl TrackerProvider for GitlabTracker {
    fn name(&self) -> &'static str {
        "gitlab"
    }

    fn pull(&self, root: &std::path::Path) -> Result<SyncResult> {
        require_config(&self.config)?;
        let project_id = self.project_id()?;
        let url = format!("{}/api/v4/projects/{}/issues?state=all&per_page=100", self.base_url(), urlencoding::encode(&project_id));
        let response = self.client()?
            .get(&url)
            .header("PRIVATE-TOKEN", self.auth_header()?)
            .send()
            .context("failed to fetch GitLab issues")?;

        if !response.status().is_success() {
            anyhow::bail!("GitLab API error: {}", response.status());
        }

        let issues: Vec<GitlabIssue> = response.json().context("failed to parse GitLab issues")?;
        let store = TicketStore::new(root)?;
        let mut created = 0;
        let mut updated = 0;

        for issue in &issues {
            let external_ref = format!("gitlab:{}/{}", project_id, issue.iid);
            let status = if issue.state == "closed" { TicketStatus::Closed } else { TicketStatus::Open };
            let existing = store.list()?.into_iter().find(|t| t.external_ref.as_ref() == Some(&external_ref));

            if let Some(existing) = existing {
                let mut update = TicketUpdate::default();
                if existing.title != issue.title {
                    update.title = Some(issue.title.clone());
                }
                if existing.description != issue.description.clone().unwrap_or_default() {
                    update.description = Some(issue.description.clone().unwrap_or_default());
                }
                if existing.status != status {
                    store.set_status(&existing.id, status, "gitlab-sync", None)?;
                }
                if update.title.is_some() || update.description.is_some() {
                    store.update(&existing.id, update)?;
                }
                updated += 1;
            } else {
                let id = TicketId::mint(format!("{}:{}", external_ref, Utc::now()).as_bytes());
                let parse_dt = |s: &Option<String>| {
                    s.as_ref().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()).map(|d| d.with_timezone(&Utc))
                };
                let ticket = Ticket {
                    id,
                    hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(issue.title.as_bytes())),
                    title: issue.title.clone(),
                    description: issue.description.clone().unwrap_or_default(),
                    design: None,
                    acceptance: None,
                    notes: Vec::new(),
                    status,
                    kind: map_gitlab_labels_to_kind(&issue.labels),
                    priority: map_gitlab_labels_to_priority(&issue.labels),
                    assignee: issue.assignee.clone().map(|a| a.username),
                    estimate_minutes: None,
                    due_at: None,
                    defer_until: None,
                    labels: issue.labels.clone(),
                    external_ref: Some(external_ref),
                    metadata: HashMap::new(),
                    created_at: parse_dt(&issue.created_at).unwrap_or_else(Utc::now),
                    updated_at: parse_dt(&issue.updated_at).unwrap_or_else(Utc::now),
                    closed_at: parse_dt(&issue.closed_at),
                    close_reason: None,
                };
                store.create(ticket)?;
                created += 1;
            }
        }

        Ok(SyncResult {
            provider: "gitlab".to_string(),
            direction: "pull".to_string(),
            created,
            updated,
            unchanged: issues.len() - created - updated,
            errors: Vec::new(),
            message: Some(format!("synced {} GitLab issues", issues.len())),
        })
    }

    fn push(&self, _root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult> {
        require_config(&self.config)?;
        let project_id = self.project_id()?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for ticket in tickets {
            if ticket.status == TicketStatus::Closed {
                continue;
            }

            match &ticket.external_ref {
                Some(external_ref) if external_ref.starts_with("gitlab:") => {
                    let iid = external_ref.split('/').last().and_then(|s| s.parse::<u64>().ok());
                    if let Some(iid) = iid {
                        let url = format!("{}/api/v4/projects/{}/issues/{iid}", self.base_url(), urlencoding::encode(&project_id));
                        let body = serde_json::json!({
                            "title": ticket.title,
                            "description": ticket.description,
                        });
                        let response = self.client()?
                            .put(&url)
                            .header("PRIVATE-TOKEN", self.auth_header()?)
                            .json(&body)
                            .send();
                        match response {
                            Ok(r) if r.status().is_success() => updated += 1,
                            Ok(r) => errors.push(format!("update {} failed: {}", ticket.id, r.status())),
                            Err(e) => errors.push(format!("update {} failed: {}", ticket.id, e)),
                        }
                    }
                }
                _ => {
                    let url = format!("{}/api/v4/projects/{}/issues", self.base_url(), urlencoding::encode(&project_id));
                    let body = serde_json::json!({
                        "title": ticket.title,
                        "description": ticket.description,
                    });
                    let response = self.client()?
                        .post(&url)
                        .header("PRIVATE-TOKEN", self.auth_header()?)
                        .json(&body)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => created += 1,
                        Ok(r) => errors.push(format!("create {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("create {} failed: {}", ticket.id, e)),
                    }
                }
            }
        }

        Ok(SyncResult {
            provider: "gitlab".to_string(),
            direction: "push".to_string(),
            created,
            updated,
            unchanged: tickets.len() - created - updated - errors.len(),
            errors,
            message: Some(format!("pushed {created} created, {updated} updated")),
        })
    }

    fn status(&self) -> Result<TrackerStatus> {
        let configured = self.config.token.is_some() && self.config.project.is_some();
        let mut connected = false;
        let mut message = "not configured".to_string();
        if configured {
            let url = format!("{}/api/v4/user", self.base_url());
            match self.client()?.get(&url).header("PRIVATE-TOKEN", self.auth_header()?).send() {
                Ok(r) => {
                    connected = r.status().is_success();
                    message = format!("GitLab API response: {}", r.status());
                }
                Err(e) => message = format!("GitLab API connection failed: {e}"),
            }
        }
        Ok(TrackerStatus {
            provider: "gitlab".to_string(),
            configured,
            connected,
            message,
        })
    }
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitlabIssue {
    iid: u64,
    title: String,
    description: Option<String>,
    state: String,
    #[serde(default)]
    labels: Vec<String>,
    assignee: Option<GitlabUser>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
    #[serde(default)]
    closed_at: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
struct GitlabUser {
    username: String,
}

fn map_gitlab_labels_to_kind(labels: &[String]) -> TicketKind {
    for label in labels {
        match label.to_lowercase().as_str() {
            "bug" => return TicketKind::Bug,
            "feature" | "enhancement" => return TicketKind::Feature,
            "epic" => return TicketKind::Epic,
            "chore" => return TicketKind::Chore,
            _ => {}
        }
    }
    TicketKind::Task
}

fn map_gitlab_labels_to_priority(labels: &[String]) -> TicketPriority {
    for label in labels {
        match label.to_lowercase().as_str() {
            "p0" | "critical" => return TicketPriority::P0,
            "p1" | "high" => return TicketPriority::P1,
            "p2" | "medium" => return TicketPriority::P2,
            "p3" | "low" => return TicketPriority::P3,
            "p4" => return TicketPriority::P4,
            _ => {}
        }
    }
    TicketPriority::P2
}

// ---------- Notion ----------

pub struct NotionTracker {
    config: TrackerConfig,
}

impl NotionTracker {
    pub fn new(config: TrackerConfig) -> Self {
        Self { config }
    }

    fn client(&self) -> Result<reqwest::blocking::Client> {
        reqwest::blocking::Client::builder()
            .user_agent("rails-cli/0.1.0")
            .build()
            .context("failed to build HTTP client")
    }

    fn token(&self) -> Result<String> {
        self.config.token.clone().context("missing Notion token")
    }

    fn database_id(&self) -> Result<String> {
        self.config.project.clone().context("missing Notion database_id")
    }
}

impl TrackerProvider for NotionTracker {
    fn name(&self) -> &'static str {
        "notion"
    }

    fn pull(&self, root: &std::path::Path) -> Result<SyncResult> {
        require_config(&self.config)?;
        let db_id = self.database_id()?;
        let url = format!("https://api.notion.com/v1/databases/{}/query", db_id);
        let response = self.client()?
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token()?))
            .header("Notion-Version", "2022-06-28")
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .context("failed to query Notion database")?;

        if !response.status().is_success() {
            anyhow::bail!("Notion API error: {}", response.status());
        }

        let json: serde_json::Value = response.json().context("failed to parse Notion response")?;
        let results = json["results"].as_array().cloned().unwrap_or_default();
        let store = TicketStore::new(root)?;
        let mut created = 0;
        let mut updated = 0;

        for page in &results {
            let page_id = page["id"].as_str().unwrap_or("").to_string();
            let external_ref = format!("notion:{page_id}");
            let props = &page["properties"];
            let title = extract_notion_title(props.get("Name").or_else(|| props.get("Title")));
            let status = if extract_notion_status(props.get("Status")).to_lowercase() == "done" {
                TicketStatus::Closed
            } else {
                TicketStatus::Open
            };
            let description = extract_notion_rich_text(props.get("Description"));

            let existing = store.list()?.into_iter().find(|t| t.external_ref.as_ref() == Some(&external_ref));
            if let Some(existing) = existing {
                let mut update = TicketUpdate::default();
                if existing.title != title {
                    update.title = Some(title.clone());
                }
                if existing.description != description {
                    update.description = Some(description.clone());
                }
                if existing.status != status {
                    store.set_status(&existing.id, status, "notion-sync", None)?;
                }
                if update.title.is_some() || update.description.is_some() {
                    store.update(&existing.id, update)?;
                }
                updated += 1;
            } else {
                let id = TicketId::mint(format!("{}:{}", external_ref, Utc::now()).as_bytes());
                let ticket = Ticket {
                    id,
                    hierarchical_id: crate::rails_id::HierarchicalId::root(TicketId::mint(title.as_bytes())),
                    title,
                    description,
                    design: None,
                    acceptance: None,
                    notes: Vec::new(),
                    status,
                    kind: TicketKind::Task,
                    priority: TicketPriority::P2,
                    assignee: None,
                    estimate_minutes: None,
                    due_at: None,
                    defer_until: None,
                    labels: Vec::new(),
                    external_ref: Some(external_ref),
                    metadata: HashMap::new(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    closed_at: None,
                    close_reason: None,
                };
                store.create(ticket)?;
                created += 1;
            }
        }

        Ok(SyncResult {
            provider: "notion".to_string(),
            direction: "pull".to_string(),
            created,
            updated,
            unchanged: results.len() - created - updated,
            errors: Vec::new(),
            message: Some(format!("synced {} Notion pages", results.len())),
        })
    }

    fn push(&self, _root: &std::path::Path, tickets: &[Ticket]) -> Result<SyncResult> {
        require_config(&self.config)?;
        let db_id = self.database_id()?;
        let mut created = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for ticket in tickets {
            if ticket.status == TicketStatus::Closed {
                continue;
            }

            let body = serde_json::json!({
                "parent": { "database_id": db_id },
                "properties": {
                    "Name": { "title": [{ "text": { "content": ticket.title } }] },
                    "Description": { "rich_text": [{ "text": { "content": ticket.description } }] },
                    "Status": { "status": { "name": "Not started" } }
                }
            });

            match &ticket.external_ref {
                Some(external_ref) if external_ref.starts_with("notion:") => {
                    let page_id = external_ref.trim_start_matches("notion:");
                    let url = format!("https://api.notion.com/v1/pages/{page_id}");
                    let update_body = serde_json::json!({
                        "properties": {
                            "Name": { "title": [{ "text": { "content": ticket.title } }] },
                            "Description": { "rich_text": [{ "text": { "content": ticket.description } }] }
                        }
                    });
                    let response = self.client()?
                        .patch(&url)
                        .header("Authorization", format!("Bearer {}", self.token()?))
                        .header("Notion-Version", "2022-06-28")
                        .header("Content-Type", "application/json")
                        .json(&update_body)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => updated += 1,
                        Ok(r) => errors.push(format!("update {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("update {} failed: {}", ticket.id, e)),
                    }
                }
                _ => {
                    let url = "https://api.notion.com/v1/pages";
                    let response = self.client()?
                        .post(url)
                        .header("Authorization", format!("Bearer {}", self.token()?))
                        .header("Notion-Version", "2022-06-28")
                        .header("Content-Type", "application/json")
                        .json(&body)
                        .send();
                    match response {
                        Ok(r) if r.status().is_success() => created += 1,
                        Ok(r) => errors.push(format!("create {} failed: {}", ticket.id, r.status())),
                        Err(e) => errors.push(format!("create {} failed: {}", ticket.id, e)),
                    }
                }
            }
        }

        Ok(SyncResult {
            provider: "notion".to_string(),
            direction: "push".to_string(),
            created,
            updated,
            unchanged: tickets.len() - created - updated - errors.len(),
            errors,
            message: Some(format!("pushed {created} created, {updated} updated")),
        })
    }

    fn status(&self) -> Result<TrackerStatus> {
        let configured = self.config.token.is_some() && self.config.project.is_some();
        let mut connected = false;
        let mut message = "not configured".to_string();
        if configured {
            let url = format!("https://api.notion.com/v1/databases/{}", self.database_id()?);
            match self.client()?
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.token()?))
                .header("Notion-Version", "2022-06-28")
                .send()
            {
                Ok(r) => {
                    connected = r.status().is_success();
                    message = format!("Notion API response: {}", r.status());
                }
                Err(e) => message = format!("Notion API connection failed: {e}"),
            }
        }
        Ok(TrackerStatus {
            provider: "notion".to_string(),
            configured,
            connected,
            message,
        })
    }
}

fn extract_notion_title(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|v| v["title"].as_array())
        .and_then(|arr| arr.iter().next())
        .and_then(|v| v["text"]["content"].as_str())
        .unwrap_or("")
        .to_string()
}

fn extract_notion_rich_text(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|v| v["rich_text"].as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v["text"]["content"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

fn extract_notion_status(value: Option<&serde_json::Value>) -> String {
    value
        .and_then(|v| v["status"]["name"].as_str())
        .unwrap_or("")
        .to_string()
}

