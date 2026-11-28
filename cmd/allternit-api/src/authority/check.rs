//! `check`: one kernel decision before every tool execution (CONTRACT.md 2.1, 4, 5.1, 6.2; plan step 9).
//!
//! Called inside the functions that run tools, not in the HTTP layer, because some callers skip
//! HTTP (the routine scheduler, the MCP server):
//!
//! | Choke point | What it checks |
//! |---|---|
//! | `tool_routes::execute_tool_internal` | Native tools (HTTP `/tools/execute`, MCP `tools/call`, routine monitors) |
//! | `server_tool_routes::execute_server_tool` | Org server tools, which take precedence over native tools of the same name |
//! | `mcp_dispatcher::McpDispatcher::dispatch_call` | Tools on attached MCP servers (MCP `tools/call`, commerce fulfilment) |
//! | `computer_v2::execute_v2` | Structured computer members (`read_ui`, `act`, `run_batch`, `use_credential`, ...) |
//!
//! Each tool call gets exactly one `check`: a server tool is checked in its own path and the
//! native check runs only when no server tool matched.
//!
//! **Steps.** Classify the call into an action class (section 4.1) with the table below, apply the
//! autonomy level (`policy::class_rule_in`), and for an `ask` or human-only class require an
//! approval token bound to `sha256:<hex>` of the canonical action `{tool, args}`, redeemed through
//! [`SqlAuthority::redeem`]. The `check` receipt is written before the answer is returned (write
//! before act). A tool that is not in the table is `ask`, never allow.
//!
//! **Modes** (`ALLTERNIT_KERNEL_CHECK`):
//!
//! | Value | Receipt | Blocks |
//! |---|---|---|
//! | `off` | no | no (the old permission gate alone) |
//! | `shadow` (default) | yes, with what enforce would have done | no |
//! | `enforce` (the target) | yes | yes: ask and human-only calls without a redeemed token are refused and an approval request is created |
//!
//! In shadow mode a kernel failure (no signing key, store unreachable) is logged and the call
//! runs; in enforce mode it is a denial (`kernel_unavailable`, fail closed).
//!
//! **Autonomy.** No onboarding consent receipt exists yet (CONTRACT.md 4.4), so the level is
//! `ask_first` for every account.
//!
//! The older `permission_policy` gate in `tool_routes::execute_tool` still runs first on the HTTP
//! route (its stores move onto the kernel in step 11). A call runs only when both allow it, so a
//! kernel denial always wins.

use std::path::PathBuf;
use std::sync::Arc;

use allternit_authority::policy::{class_rule_in, Condition, Rule};
use allternit_authority::{
    ActionClass, ApprovalRedeemBody, ApprovalRequestV1, AuthorityError, AutonomyLevel, Caller, CallerKind, ReceiptKind,
    SourceStore,
};
use allternit_factory_engine::receipts::sign::ReceiptSigner;
use chrono::{DateTime, Duration, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::approvals::{SqlAuthority, REQUEST_SCHEMA_ID};
use super::audit::{append, ReceiptInput, DEFAULT_TENANT};
use super::tokens;
use crate::db::DbHandle;

/// Env switch: `off`, `shadow` (default) or `enforce`.
pub const MODE_ENV: &str = "ALLTERNIT_KERNEL_CHECK";
/// `schema_id` of a check receipt.
pub const CHECK_SCHEMA_ID: &str = "allternit.os.PolicyCheckV1";
/// `code` in the refusal a tool caller gets when an approval is needed.
pub const APPROVAL_REQUIRED: &str = "approval_required";
/// How long a person has to answer a request `check` creates.
const REQUEST_DEADLINE_MINS: i64 = 60;
/// The level used until onboarding writes a consent receipt (CONTRACT.md 4.4).
const AUTONOMY: AutonomyLevel = AutonomyLevel::AskFirst;
/// The class recorded for a tool the table does not know. The enum is closed, so an unknown tool
/// is written down as `file_write` and its rule is forced to `ask` (never allow).
const DEFAULT_ASK_CLASS: ActionClass = ActionClass::FileWrite;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Shadow,
    Enforce,
}

impl Mode {
    pub fn from_env() -> Self {
        Self::parse(std::env::var(MODE_ENV).ok().as_deref())
    }

    /// `off` and `enforce` by name; anything else (unset, empty, a typo) is `shadow`.
    pub fn parse(v: Option<&str>) -> Self {
        match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("off") => Mode::Off,
            Some("enforce") => Mode::Enforce,
            _ => Mode::Shadow,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Shadow => "shadow",
            Mode::Enforce => "enforce",
        }
    }
}

/// Which execution path is asking. Picks the classification table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `execute_tool_internal`'s own tools.
    Native,
    /// An org server tool (admin-registered code in the platform sandbox).
    ServerTool,
    /// A tool on an attached MCP server, named `<server>.<tool>`.
    Mcp,
    /// A structured computer member (`computer_v2`).
    Computer,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Native => "tool",
            Source::ServerTool => "server_tool",
            Source::Mcp => "mcp",
            Source::Computer => "computer_v2",
        }
    }
}

/// Who is acting. Set by the code that received the call (the transport), never from tool
/// arguments, so a model cannot pick its own caller kind or class.
#[derive(Clone)]
pub struct CheckCtx {
    pub db: DbHandle,
    /// The authenticated account: owner of any approval request and of the receipt chain.
    pub user_id: String,
    /// Chain scope with `user_id` (`default` when none).
    pub tenant: Option<String>,
    pub caller_kind: CallerKind,
    /// The run the call belongs to, when the caller knows it.
    pub run_id: Option<String>,
    /// An `ApprovalTokenV1.token` presented with the call (outside the hashed arguments).
    pub approval_token: Option<String>,
    /// Host code only, never from a request: a fixed class for a call whose human step happened
    /// elsewhere, with the reason written to the receipt.
    pub class_override: Option<(ActionClass, &'static str)>,
    mode: Option<Mode>,
}

impl CheckCtx {
    /// A run's tool call for `user_id`.
    pub fn new(db: DbHandle, user_id: &str, tenant: Option<&str>) -> Self {
        Self {
            db,
            user_id: user_id.to_string(),
            tenant: tenant.filter(|t| !t.is_empty()).map(str::to_string),
            caller_kind: CallerKind::Run,
            run_id: None,
            approval_token: None,
            class_override: None,
            mode: None,
        }
    }

    pub fn with_run_id(mut self, run_id: Option<String>) -> Self {
        self.run_id = run_id.filter(|r| !r.trim().is_empty());
        self
    }

    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.approval_token = token.filter(|t| !t.trim().is_empty());
        self
    }

    /// Host code acting on its own (not a model's tool call), with a fixed class and its reason.
    pub fn service(mut self, class: ActionClass, reason: &'static str) -> Self {
        self.caller_kind = CallerKind::Service;
        self.class_override = Some((class, reason));
        self
    }

    /// Fix the mode instead of reading the env (tests; the env is process-wide).
    #[cfg(test)]
    pub fn with_mode(mut self, mode: Mode) -> Self {
        self.mode = Some(mode);
        self
    }

    pub(crate) fn mode(&self) -> Mode {
        self.mode.unwrap_or_else(Mode::from_env)
    }

    pub(crate) fn data_dir(&self) -> PathBuf {
        self.db.path().parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."))
    }
}

/// One tool call to check.
#[derive(Clone, Debug)]
pub struct Action {
    pub source: Source,
    pub tool: String,
    /// The arguments the tool will run with (hashed; never written to a receipt).
    pub args: Value,
    /// Computer members: the contract's `risk` (`reversible`, `risky`, `irreversible`).
    pub risk: Option<String>,
}

impl Action {
    pub fn new(source: Source, tool: &str, args: &Value) -> Self {
        Self { source, tool: tool.to_string(), args: args.clone(), risk: None }
    }

    pub fn with_risk(mut self, risk: Option<&str>) -> Self {
        self.risk = risk.map(str::to_string);
        self
    }

    /// The canonical action the token is bound to: `{tool, args}`. The same call through another
    /// front door hashes the same.
    pub fn payload(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("tool".into(), json!(self.tool));
        m.insert("args".into(), self.args.clone());
        m
    }
}

/// The class and where it came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Classified {
    pub class: ActionClass,
    pub cond: Condition,
    /// `native_table`, `computer_contract`, `mcp_table`, `host_override` or `default_ask`.
    pub by: &'static str,
    /// The tool is not in any table: `ask`, whatever the class row would say.
    pub forced_ask: bool,
}

impl Classified {
    fn known(class: ActionClass, by: &'static str) -> Self {
        Self { class, cond: Condition::NONE, by, forced_ask: false }
    }
    fn unknown() -> Self {
        Self { class: DEFAULT_ASK_CLASS, cond: Condition::NONE, by: "default_ask", forced_ask: true }
    }
}

/// `execute_tool_internal`'s tools. Conditions stay `NONE` (nothing here knows a path is in the
/// profile scope or a host is trusted), so `file_write` and `network` ask.
fn native_class(tool: &str) -> Option<ActionClass> {
    use ActionClass::*;
    Some(match tool {
        "file.read" | "file.list" | "file.exists" | "system.info" | "system.env" | "time.now" | "echo"
        | "document_to_markdown" | "computer_screenshot" | "computer_file_read" | "computer_list" => Read,
        "api_capture_record" | "api_capture_stop" | "computer_start" | "computer_stop" => RunControl,
        // Shell and code can write anything the account can, so never "in profile scope".
        "file.write" | "computer_file_write" | "computer_mouse" | "computer_keyboard" | "shell.exec" | "shell.eval"
        | "bash" | "code_execution" | "computer_shell" => FileWrite,
        "http.get" | "http.post" | "url_to_markdown" | "api_capture_replay" => Network,
        "file.remove" => Irreversible,
        // computer_create / clone / resize spend compute: ask until budgets land (step 12).
        _ => return None,
    })
}

/// Structured computer members, from the computer contract's `risk` field.
fn computer_class(member: &str, risk: Option<&str>) -> Classified {
    use ActionClass::*;
    let class = match member {
        "read_ui" | "verify" | "skills" | "resolve_access" => Read,
        "request_human" => RunControl,
        // Classified by `secrets::classify_credential_use` in `classify` (known pair: read, new: human-only).
        "use_credential" => SecretNewHost,
        _ => match risk {
            Some("irreversible") => Irreversible,
            Some(_) => FileWrite,
            None => return Classified::unknown(),
        },
    };
    Classified::known(class, "computer_contract")
}

pub fn classify(ctx: &CheckCtx, action: &Action) -> Classified {
    if let Some((class, _)) = ctx.class_override {
        return Classified::known(class, "host_override");
    }
    match action.source {
        Source::Native => native_class(&action.tool).map_or_else(Classified::unknown, |c| Classified::known(c, "native_table")),
        Source::Computer if action.tool == "use_credential" => super::secrets::classify_credential_use(ctx, &action.args),
        Source::Computer => computer_class(&action.tool, action.risk.as_deref()),
        Source::Mcp => match action.tool.split_once('.').map(|(_, t)| t) {
            // An app's checkout fulfilment moves an order to paid: money, human-only.
            Some("complete_checkout") => Classified::known(ActionClass::Money, "mcp_table"),
            _ => Classified::unknown(),
        },
        Source::ServerTool => Classified::unknown(),
    }
}

/// What the tool path does next.
#[derive(Clone, Debug)]
pub enum Decision {
    /// Run the tool. `receipt_id` is `None` only in `off` mode or when shadow mode could not write.
    Allow { receipt_id: Option<String> },
    /// Do not run it.
    Deny(AuthorityError),
    /// Do not run it: a person has to approve this exact action first.
    NeedsToken { request_id: Option<String>, action_hash: String, action_class: ActionClass, receipt_id: String },
}

fn enum_str<T: Serialize>(v: T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

impl Decision {
    pub fn allowed(&self) -> bool {
        matches!(self, Decision::Allow { .. })
    }

    /// The refusal as a tool error: a JSON string with `code`, `message`, `receipt_id` and, when an
    /// approval is needed, `approval_request_id` and `action_hash`.
    pub fn refusal(&self) -> Option<Value> {
        match self {
            Decision::Allow { .. } => None,
            Decision::Deny(e) => Some(json!({
                "code": enum_str(e.code), "error": "denied by the authority kernel", "message": e.message,
                "receipt_id": e.receipt_id,
            })),
            Decision::NeedsToken { request_id, action_hash, action_class, receipt_id } => {
                let message = match request_id {
                    Some(id) => format!("This action needs a person's approval. Approve request {id} in Allternit, then retry the same call with approval_token set to the token it returns."),
                    None => "This action needs a person's approval. Request one with POST /api/v1/kernel/approvals/request for this action_hash, then retry with approval_token.".to_string(),
                };
                Some(json!({
                    "code": APPROVAL_REQUIRED, "error": APPROVAL_REQUIRED, "message": message,
                    "approval_request_id": request_id, "action_hash": action_hash, "action_class": enum_str(*action_class),
                    "receipt_id": receipt_id, "retry_with": "approval_token",
                }))
            }
        }
    }

    /// `Ok(())` when the tool may run, else the refusal as a JSON string.
    pub fn into_tool_result(self) -> Result<(), String> {
        match self.refusal() {
            None => Ok(()),
            Some(v) => Err(v.to_string()),
        }
    }
}

/// A kernel failure: deny in enforce mode, log and run in shadow mode.
fn failure(mode: Mode, e: AuthorityError) -> Decision {
    match mode {
        Mode::Enforce => Decision::Deny(e),
        _ => {
            tracing::warn!(code = ?e.code, error = %e.message, "kernel check unavailable (shadow mode: tool runs)");
            Decision::Allow { receipt_id: None }
        }
    }
}

fn rule_str(rule: Rule) -> &'static str {
    match rule {
        Rule::Allow => "allow",
        Rule::Ask => "ask",
        Rule::HumanOnly => "human_only",
    }
}

/// `check(ctx, action)`: classify, apply autonomy, redeem a token if one came with the call, write
/// the receipt, answer. Runs the store work on a blocking thread.
pub async fn check(ctx: &CheckCtx, action: &Action) -> Decision {
    let mode = ctx.mode();
    if mode == Mode::Off {
        return Decision::Allow { receipt_id: None };
    }
    let signer = match tokens::kernel_signer(&ctx.data_dir()) {
        Ok(s) => s,
        Err(e) => return failure(mode, e),
    };
    let (ctx, action) = (ctx.clone(), action.clone());
    match tokio::task::spawn_blocking(move || check_blocking(&ctx, &action, mode, signer, Utc::now())).await {
        Ok(d) => d,
        Err(e) => failure(mode, AuthorityError::kernel_unavailable(format!("kernel check task: {e}"))),
    }
}

/// The decision itself, with mode, signer and clock supplied.
pub fn check_blocking(ctx: &CheckCtx, action: &Action, mode: Mode, signer: Arc<ReceiptSigner>, now: DateTime<Utc>) -> Decision {
    if mode == Mode::Off {
        return Decision::Allow { receipt_id: None };
    }
    let cl = classify(ctx, action);
    let rule = if cl.forced_ask { Rule::Ask } else { class_rule_in(cl.class, AUTONOMY, cl.cond) };
    let payload = action.payload();
    let hash = match tokens::action_hash(&payload) {
        Ok(h) => h,
        Err(e) => return failure(mode, AuthorityError::kernel_unavailable(format!("action payload is not canonicalisable: {e}"))),
    };
    let check_id = format!("chk_{}", uuid::Uuid::new_v4().simple());
    let run_id = ctx.run_id.clone().unwrap_or_else(|| format!("tool:{}", action.source.as_str()));
    let tenant = ctx.tenant.clone().unwrap_or_else(|| DEFAULT_TENANT.to_string());
    let caller = Caller { kind: ctx.caller_kind, principal: ctx.user_id.clone() };
    let kernel = SqlAuthority::new(ctx.db.clone(), signer.clone()).with_tenant(ctx.tenant.clone()).at(now);

    // Never the arguments: a receipt holds no secret value.
    let mut receipt = json!({
        "event": "check", "check_id": check_id, "tool": action.tool, "surface": action.source.as_str(),
        "action_class": enum_str(cl.class), "classified_by": cl.by, "rule": rule_str(rule),
        "autonomy_level": enum_str(AUTONOMY), "action_hash": hash, "mode": mode.as_str(),
        "caller_kind": enum_str(ctx.caller_kind), "token_presented": ctx.approval_token.is_some(),
    });
    if let Some((_, reason)) = ctx.class_override {
        receipt["override_reason"] = json!(reason);
    }
    let write = |decision: &str, mut payload: Value| -> Result<String, AuthorityError> {
        payload["decision"] = json!(decision);
        let mut conn = ctx.db.connect().map_err(|e| AuthorityError::kernel_unavailable(format!("kernel store: {e}")))?;
        append(
            &mut conn,
            &signer,
            ReceiptInput {
                run_id: &run_id,
                principal: &ctx.user_id,
                tenant: &tenant,
                kind: ReceiptKind::Check,
                schema_id: CHECK_SCHEMA_ID,
                decision,
                payload,
                idempotency_key: None,
            },
            now,
        )
    };
    // Write before act: no receipt, no allow (enforce); shadow logs and runs.
    let allow = |decision: &str, payload: Value| match write(decision, payload) {
        Ok(rid) => Decision::Allow { receipt_id: Some(rid) },
        Err(e) => failure(mode, e),
    };

    if rule == Rule::Allow {
        return allow("allow", receipt);
    }

    // A person's standing per-tool Allow (Customize policy, synced to `kernel_standing_grants`)
    // answers an `ask` rule. `HumanOnly` classes never read it: they always need a fresh token.
    if rule == Rule::Ask && ctx.approval_token.is_none() && super::standing::is_granted(&ctx.db, &ctx.user_id, &action.tool) {
        receipt["approved_by"] = json!("standing_grant");
        return allow("allow", receipt);
    }

    // ask or human-only: a token for this exact payload, redeemed now, or nothing runs.
    if let Some(token) = ctx.approval_token.clone() {
        return match kernel.redeem(&caller, ApprovalRedeemBody { token, action_hash: hash.clone() }) {
            Ok(r) => {
                receipt["approval_request_id"] = json!(r.request_id);
                receipt["spend_receipt_id"] = json!(r.spend_receipt_id);
                allow("allow", receipt)
            }
            // The redeem already wrote its denial receipt (wrong hash, replay, expiry, ...).
            Err(e) if mode == Mode::Enforce => Decision::Deny(e),
            Err(e) => {
                receipt["would_deny"] = json!(enum_str(e.code));
                receipt["redeem_receipt_id"] = json!(e.receipt_id);
                allow("shadow_deny", receipt)
            }
        };
    }

    if mode == Mode::Shadow {
        receipt["would_require"] = json!(if rule == Rule::HumanOnly { "human_approval" } else { "approval" });
        return allow("shadow_would_ask", receipt);
    }

    // Enforce, no token: open an approval request for this exact action and refuse.
    let summary: String = format!("Run {} ({})", action.tool, enum_str(cl.class)).chars().take(280).collect();
    let request = ApprovalRequestV1 {
        schema_id: REQUEST_SCHEMA_ID.into(),
        schema_version: tokens::SCHEMA_VERSION.into(),
        request_id: None,
        source_store: SourceStore::Kernel,
        source_ref: Some(check_id.clone()),
        run_id: run_id.clone(),
        // The acting run, not the account: the account's person is the one who approves.
        principal: format!("run:{run_id}"),
        assignee: "owner".into(),
        action_class: cl.class,
        action_summary: summary,
        action_payload: payload,
        action_hash: None,
        deadline: tokens::ts(now + Duration::minutes(REQUEST_DEADLINE_MINS)),
        on_expiry: "reject".into(),
        attention_id: None,
        check_id: check_id.clone(),
        status: None,
        created_at: None,
        resolved_at: None,
        resolved_by: None,
        evidence_refs: None,
        extensions: None,
    };
    let request_id = match kernel.request(&caller, request) {
        Ok(r) => r.request_id,
        Err(e) => {
            tracing::warn!(code = ?e.code, error = %e.message, "kernel check could not open an approval request");
            receipt["request_error"] = json!(enum_str(e.code));
            None
        }
    };
    receipt["approval_request_id"] = json!(request_id);
    match write(APPROVAL_REQUIRED, receipt) {
        Ok(receipt_id) => Decision::NeedsToken { request_id, action_hash: hash, action_class: cl.class, receipt_id },
        Err(e) => Decision::Deny(e),
    }
}

/// The `code` of a refusal string from [`Decision::into_tool_result`], if it is one.
pub fn refusal_code(err: &str) -> Option<String> {
    serde_json::from_str::<Value>(err).ok()?.get("code")?.as_str().map(str::to_string)
}

/// Same, for code that only needs to know an approval is pending.
pub fn is_approval_required(err: &str) -> bool {
    refusal_code(err).as_deref() == Some(APPROVAL_REQUIRED)
}

#[cfg(test)]
mod tests {
    use super::super::approvals::{Decision as Resolve, Resolution, ResolveBody};
    use super::*;
    use allternit_authority::ErrorCode;

    const USER: &str = "user_owner";

    fn t0() -> DateTime<Utc> {
        tokens::parse_ts("2026-11-28T12:00:00Z").unwrap()
    }

    fn signer() -> Arc<ReceiptSigner> {
        Arc::new(ReceiptSigner::from_seed([7u8; 32]))
    }

    fn ctx(db: &DbHandle) -> CheckCtx {
        CheckCtx::new(db.clone(), USER, None).with_run_id(Some("run_1".into()))
    }

    fn count(db: &DbHandle, sql: &str) -> i64 {
        db.connect().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn last_decision(db: &DbHandle) -> String {
        db.connect()
            .unwrap()
            .query_row("SELECT decision FROM kernel_receipts WHERE kind = 'check' ORDER BY idx DESC LIMIT 1", [], |r| r.get(0))
            .unwrap()
    }

    fn write_action() -> Action {
        Action::new(Source::Native, "file.write", &json!({ "path": "/etc/hosts", "content": "x" }))
    }

    #[test]
    fn mode_parses_and_defaults_to_shadow() {
        assert_eq!(Mode::parse(None), Mode::Shadow);
        assert_eq!(Mode::parse(Some("")), Mode::Shadow);
        assert_eq!(Mode::parse(Some("enforced")), Mode::Shadow);
        assert_eq!(Mode::parse(Some(" ENFORCE ")), Mode::Enforce);
        assert_eq!(Mode::parse(Some("off")), Mode::Off);
    }

    #[test]
    fn unknown_tools_ask_and_human_only_classes_are_mapped() {
        let db = DbHandle::new_memory().unwrap();
        let c = ctx(&db);
        let unknown = classify(&c, &Action::new(Source::Native, "computer_create", &json!({})));
        assert!(unknown.forced_ask, "an unknown tool is ask, never allow");
        assert_eq!(classify(&c, &Action::new(Source::Native, "file.remove", &json!({}))).class, ActionClass::Irreversible);
        assert!(classify(&c, &Action::new(Source::ServerTool, "echo", &json!({}))).forced_ask, "a server tool named echo is not native echo");
        assert_eq!(classify(&c, &Action::new(Source::Mcp, "shop.complete_checkout", &json!({}))).class, ActionClass::Money);
        assert!(classify(&c, &Action::new(Source::Mcp, "srv.anything", &json!({}))).forced_ask);
        assert_eq!(classify(&c, &Action::new(Source::Computer, "use_credential", &json!({}))).class, ActionClass::SecretNewHost);
        assert_eq!(
            classify(&c, &Action::new(Source::Computer, "act", &json!({})).with_risk(Some("irreversible"))).class,
            ActionClass::Irreversible
        );
    }

    #[test]
    fn enforce_ask_class_without_token_is_denied_with_a_receipt_and_a_request() {
        let db = DbHandle::new_memory().unwrap();
        let d = check_blocking(&ctx(&db), &write_action(), Mode::Enforce, signer(), t0());
        let Decision::NeedsToken { request_id, action_hash, action_class, receipt_id } = d.clone() else {
            panic!("expected approval_required, got {d:?}")
        };
        assert_eq!(action_class, ActionClass::FileWrite);
        assert!(tokens::is_content_hash(&action_hash));
        assert!(!receipt_id.is_empty(), "the denial carries its receipt");
        let request_id = request_id.expect("check opened an approval request");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM approval_requests WHERE status = 'open'"), 1);
        assert_eq!(last_decision(&db), APPROVAL_REQUIRED);
        let refusal = d.into_tool_result().unwrap_err();
        assert!(is_approval_required(&refusal));
        assert!(refusal.contains(&request_id), "the caller is told which request to approve");
    }

    #[test]
    fn enforce_human_only_class_without_token_is_denied() {
        let db = DbHandle::new_memory().unwrap();
        let rm = Action::new(Source::Native, "file.remove", &json!({ "path": "/tmp/x" }));
        let d = check_blocking(&ctx(&db), &rm, Mode::Enforce, signer(), t0());
        assert!(matches!(d, Decision::NeedsToken { action_class: ActionClass::Irreversible, .. }));
    }

    #[test]
    fn a_standing_allow_answers_ask_but_never_a_human_only_class() {
        let db = DbHandle::new_memory().unwrap();
        let conn = db.connect().unwrap();
        crate::authority::standing::set(&conn, USER, "file.write", true).unwrap();
        crate::authority::standing::set(&conn, USER, "stripe.refund", true).unwrap();
        let c = ctx(&db);
        let d = check_blocking(&c, &write_action(), Mode::Enforce, signer(), t0());
        assert!(d.allowed(), "{d:?}");
        assert_eq!(last_decision(&db), "allow");
        // A human-only class still needs a token, grant or not.
        let money = CheckCtx::new(db.clone(), USER, None).with_run_id(Some("run_1".into())).service(ActionClass::Money, "test");
        let a = Action::new(Source::Native, "stripe.refund", &json!({}));
        assert!(!check_blocking(&money, &a, Mode::Enforce, signer(), t0()).allowed());
    }

    #[test]
    fn allow_class_passes_with_a_receipt() {
        let db = DbHandle::new_memory().unwrap();
        let read = Action::new(Source::Native, "file.read", &json!({ "path": "/tmp/x" }));
        let d = check_blocking(&ctx(&db), &read, Mode::Enforce, signer(), t0());
        assert!(matches!(d, Decision::Allow { receipt_id: Some(_) }), "{d:?}");
        assert_eq!(last_decision(&db), "allow");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM approval_requests"), 0);
    }

    #[test]
    fn shadow_mode_passes_but_writes_a_receipt() {
        let db = DbHandle::new_memory().unwrap();
        let d = check_blocking(&ctx(&db), &write_action(), Mode::Shadow, signer(), t0());
        assert!(matches!(d, Decision::Allow { receipt_id: Some(_) }), "{d:?}");
        assert_eq!(last_decision(&db), "shadow_would_ask");
        assert_eq!(count(&db, "SELECT COUNT(*) FROM approval_requests"), 0, "shadow mode opens no requests");
    }

    #[test]
    fn off_mode_writes_nothing() {
        let db = DbHandle::new_memory().unwrap();
        let d = check_blocking(&ctx(&db), &write_action(), Mode::Off, signer(), t0());
        assert!(matches!(d, Decision::Allow { receipt_id: None }));
        assert_eq!(count(&db, "SELECT COUNT(*) FROM kernel_receipts"), 0);
    }

    #[test]
    fn approved_token_for_the_same_payload_runs_once() {
        let db = DbHandle::new_memory().unwrap();
        let k = SqlAuthority::new(db.clone(), signer()).at(t0());
        let Decision::NeedsToken { request_id, .. } = check_blocking(&ctx(&db), &write_action(), Mode::Enforce, signer(), t0()) else {
            panic!("expected approval_required")
        };
        let human = Caller { kind: CallerKind::Human, principal: USER.into() };
        let Resolution::Approved(token) =
            k.resolve(&human, ResolveBody { request_id: request_id.unwrap(), decision: Resolve::Approve, note: None }).unwrap()
        else {
            panic!("expected a token")
        };

        // A different payload with this token is refused (hash mismatch).
        let other = Action::new(Source::Native, "file.write", &json!({ "path": "/etc/passwd", "content": "x" }));
        let with = ctx(&db).with_token(Some(token.token.clone()));
        let d = check_blocking(&with, &other, Mode::Enforce, signer(), t0());
        assert!(matches!(&d, Decision::Deny(e) if e.code == ErrorCode::HashMismatch && e.receipt_id.is_some()), "{d:?}");

        // The approved payload runs, once.
        let d = check_blocking(&with, &write_action(), Mode::Enforce, signer(), t0());
        assert!(matches!(d, Decision::Allow { receipt_id: Some(_) }), "{d:?}");
        let d = check_blocking(&with, &write_action(), Mode::Enforce, signer(), t0());
        assert!(matches!(&d, Decision::Deny(e) if e.code == ErrorCode::TokenReplayed), "{d:?}");
    }

    #[tokio::test]
    async fn mcp_dispatch_is_checked_before_the_remote_call() {
        use crate::mcp_dispatcher::{McpAttachedServer, McpDispatcher, McpToolDescriptor};
        let db = DbHandle::new_memory().unwrap();
        let dispatcher = McpDispatcher::new();
        dispatcher
            .attach(McpAttachedServer {
                id: "srv".into(),
                // Never contacted: the check refuses first.
                url: "http://127.0.0.1:9/mcp".into(),
                headers: Default::default(),
                tools: vec![McpToolDescriptor { name: "echo".into(), description: String::new(), input_schema: json!({}) }],
            })
            .await;
        let c = ctx(&db).with_mode(Mode::Enforce);
        let err = dispatcher.dispatch_call(&c, "srv.echo", json!({ "message": "hi" })).await.unwrap_err();
        assert!(is_approval_required(&err), "{err}");
        let surface: String = db
            .connect()
            .unwrap()
            .query_row("SELECT json_extract(payload, '$.surface') FROM kernel_receipts WHERE kind = 'check'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(surface, "mcp");
    }
}
