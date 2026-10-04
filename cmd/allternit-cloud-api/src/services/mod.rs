//! Core services for Cowork Runtime

pub mod api_keys;
pub mod audit;
pub mod contabo_runtime_service;
pub mod cost_service;
pub mod customer_emails;
pub mod event_store;
pub mod fabric_ledger;
pub mod executor_service;
pub mod hosted_runtime_lifecycle;
pub mod inference_keys;
pub mod inference_pool;
pub mod inference_settlement;
pub mod node_resolution;
pub mod ops_alert;
pub mod provisioning;
pub mod quota_service;
pub mod r2;
pub mod run_service;
pub mod scheduler_service;
pub mod task_service;
pub mod user_files;
pub mod user_trust;
pub mod voice_billing;
pub mod voice_usage;

pub use contabo_runtime_service::{
    ContaboContainerState, ContaboRuntimeService, HostedInstanceRow, ProvisionedContaboRuntime,
};
pub use cost_service::{
    finalize_run_cost_tracking, init_run_cost_tracking, start_cost_tracking_task, AlertType,
    CostAlert, CostBreakdown, CostRate, CostService, CostServiceImpl, RunCost, RunCostSummary,
    SetCostRateRequest, UpdateBudgetRequest, UserCostBudget, UserCostSummary,
};
pub use event_store::{event_utils, EventStore, EventStoreImpl};
pub use executor_service::{
    start_executor_service, AllowAllGate, ApprovalGate, ExecutorConfig, ExecutorDeps,
    ExecutorService,
};
pub use hosted_runtime_lifecycle::{
    hosted_usage_summary, hosted_wake_decision, hosted_wake_target, mark_hosted_instance_starting,
    open_session_accrued_cost, record_runtime_started, record_runtime_stopped,
    start_hosted_runtime_lifecycle_task, touch_instance_activity, touch_runtime_activity,
    wake_hosted_runtime_for_device, HostedUsageSummary, HostedWakeDecision, HostedWakeOutcome,
    HostedWakeTarget,
};
pub use inference_settlement::{
    check_inference_allowed, credit_balance_row, meter_json_response, meter_stream_response,
    settle_inference, StreamSettlement, UsageMeteringBody,
};
pub use inference_pool::{
    check_free_tier_pool, free_tier_pool_policy, FreeTierPoolPolicy, InferencePoolService, PoolRow,
};
pub use inference_keys::{
    byok_base_url, byok_enabled, mask_key, should_route_byok, InferenceKeyInfo,
    InferenceKeyService, KeyValidator,
};
pub use node_resolution::{
    resolve_continuation_node, resolve_default_node, resolve_default_node_preferring, NodeCandidate, NodeKind, NodeStore,
    PgNodeStore, ResolvedNode,
};
pub use provisioning::{
    activate_registered_device, bind_device_slot, build_user_data, can_transition,
    incus_name_for, record_instance_started, record_instance_stopped, select_host, snapshot_alias_for,
    start_provisioning_lifecycle_task, start_provisioning_reconcile_task, start_waiting_computer_task, BootstrapContract, ComputerSize,
    usage_summary, validate_provisioned_bootstrap, BackendRegistry, BackendStatus, HostCapacity,
    IncusBackendRegistry, IncusHttpBackend, InstanceRow, InstanceView, ProvisionBackend,
    ProvisionDefaults, ProvisionError, ProvisionSpec, ProvisioningService, SuspendReason, INIT_SCRIPT,
    free_incus_name_for, note_runtime_attached, start_free_computer_task,
    touch_provisioned_activity, FreeDefaults, ProvisionedWakeOutcome, WakeReason, WakeResult,
    TIER_FREE, TIER_PAID,
};
pub use quota_service::{QuotaService, SharedQuotaService, UserQuota};
pub use run_service::{RunListFilter, RunService, RunServiceImpl};
pub use scheduler_service::{
    start_scheduler_service, MisfirePolicy, SchedulerConfig, SchedulerService,
};
pub use task_service::TaskService;
