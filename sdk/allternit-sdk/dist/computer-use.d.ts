/**
 * @allternit/sdk/computer-use - Computer Use Engine Client
 *
 * @deprecated since 2026-10-09. Use `@allternit/computer-driver` for hosted
 * computers (one contract call per action on `/v1/computers/:id/toolset`).
 * This client is a thin shim over the ACU gateway runs API and is not a
 * drop-in for the driver: it drives gateway runs and receipts, not computers.
 * `executeCompatibilityAction` now uses the gateway's `/v1/execute` channel
 * (the old `/v1/computer` route is gone).
 */
export declare const COMPUTER_USE_CONTRACT_VERSION: "1.0.0-alpha.1";
export type ComputerExecutionMode = "background_strict" | "foreground_allowed" | "sandboxed";
export interface CanonicalComputerCapabilityManifest {
    provider_id: string;
    provider_version: string;
    contract_version: typeof COMPUTER_USE_CONTRACT_VERSION;
    invariant_version: string;
    invariants: string[];
    operating_systems: Array<"macos" | "windows" | "linux" | "android">;
    actions: string[];
    observation_channels: string[];
    execution_modes: ComputerExecutionMode[];
    strict_background: boolean;
    semantic_input: boolean;
    raw_input: boolean;
    streaming: boolean;
    clipboard: boolean;
    shell: boolean;
    files: boolean;
    audio: boolean;
    mobile: boolean;
    max_concurrency: number;
    limitations: string[];
    tools?: string[];
}
export interface CanonicalProviderDiagnostic {
    available: boolean;
    reason?: string;
    message?: string;
    executable?: string;
    version?: string;
    telemetry_enabled?: boolean;
    telemetry_managed_by_allternit?: boolean;
}
export interface CanonicalProviderCatalog {
    providers: CanonicalComputerCapabilityManifest[];
    diagnostics: Record<string, CanonicalProviderDiagnostic>;
}
export interface ComputerUseRequest {
    mode: 'intent' | 'direct' | 'assist';
    task: string;
    session_id?: string;
    run_id?: string;
    target_scope?: 'browser' | 'desktop' | 'hybrid' | 'auto';
    options?: Record<string, unknown>;
    context?: Record<string, unknown>;
    metadata?: Record<string, unknown>;
}
export interface ComputerUseResponse {
    run_id: string;
    session_id: string;
    status: string;
    mode: string;
    target_scope: string;
    summary?: string;
    result?: Record<string, unknown> | null;
    error?: string | null;
}
export interface WatchOptions {
    runId: string;
    signal?: AbortSignal;
}
export interface WatchRunOptions {
    intervalMs?: number;
    signal?: AbortSignal;
}
export interface WaitForRunOptions {
    intervalMs?: number;
    signal?: AbortSignal;
}
export interface ApprovalOptions {
    approver_id?: string;
    comment?: string;
    [key: string]: unknown;
}
export interface CancelOptions {
    approver_id?: string;
    comment?: string;
    [key: string]: unknown;
}
export interface ResumeOptions {
    approver_id?: string;
    comment?: string;
    [key: string]: unknown;
}
export interface RequestOptions {
    baseUrl?: string;
    fetch?: typeof fetch;
    headers?: Record<string, string>;
}
export interface CompatibilityComputerActionRequest {
    [key: string]: unknown;
    action: string;
    session_id: string;
    run_id?: string;
    parameters?: Record<string, unknown>;
    coordinate?: [number, number];
    text?: string;
    key?: string;
    target?: string;
    goal?: string;
    adapter_preference?: string;
}
export interface BrowserSkillSpecSummary {
    skill_id: string;
    source: string;
    valid: boolean;
    error: string | null;
    workflowId?: string;
    title?: string;
    provider?: string;
    stepCount?: number;
    hasNetworkTrace?: boolean;
    networkTraceEntries?: number;
}
export interface BrowserSkillNetworkTraceEntry {
    method: string;
    host: string;
    pathTemplate: string;
    payloadKeysHash: string | null;
    verifiable: boolean;
}
export interface BrowserSkillNetworkTrace {
    version: number;
    entries: BrowserSkillNetworkTraceEntry[];
}
export interface BrowserSkillSpecDetail {
    workflowId?: string;
    title?: string;
    provider?: string;
    schemaVersion?: string;
    sourceRunId?: string;
    inputCount?: number;
    steps: Array<{
        id?: string;
        kind?: string;
        target?: string;
        reason?: string;
    }>;
    stepCount?: number;
    safety: {
        requiresApprovalFor: string[];
        redactionCount: number;
    };
    networkTrace: BrowserSkillNetworkTrace | null;
}
export interface BrowserSkillDeviation {
    kind: string;
    index: number;
    live_index: number | null;
    expected: Partial<BrowserSkillNetworkTraceEntry>;
    actual: Partial<BrowserSkillNetworkTraceEntry> | null;
}
export interface BrowserSkillVerifyResult {
    verify_id: string;
    status: string;
    mode: string;
    workflow_id?: string;
    target_url?: string;
    network?: {
        status: string;
        deviations: BrowserSkillDeviation[];
    };
    a11y?: {
        added: number;
        removed: number;
        modified: number;
    } | {
        status: "unverifiable";
    };
    workflow_status?: string;
    approvals?: string[];
    grant_requests?: number;
    receipt_id?: string;
    receipt_hash?: string;
    trace?: BrowserSkillNetworkTrace;
    error?: string | null;
}
export interface BrowserSkillReceiptCheck {
    verify_id: string;
    receipt_id?: string;
    valid: boolean;
    stored_hash: string;
    recomputed_hash: string;
    tampered: boolean;
}
export interface StartBrowserSkillVerifyOptions {
    /** Skill package id; the spec is read + validated server-side. */
    skillId?: string;
    /** Explicit spec object (server validates + distills). */
    workflow?: Record<string, unknown>;
    /** Absolute http(s) URL. Omit for the canned deterministic self-check. */
    targetUrl?: string;
}
/** @deprecated Use `@allternit/computer-driver`. */
export declare class AllternitComputerUseClient {
    readonly baseUrl: string;
    readonly fetch: typeof fetch;
    readonly headers: Record<string, string>;
    constructor(config?: RequestOptions);
    execute(request: ComputerUseRequest): Promise<ComputerUseResponse>;
    executeStream(request: ComputerUseRequest): Promise<Response>;
    /**
     * One browser-session action on the gateway's `/v1/execute` channel
     * (`action`, `session_id`, optional `target`/`text`/`parameters`). Returns the
     * execute response (`status`, `summary`, `artifacts`, `extracted_content`).
     * The old `/v1/computer` route this used was removed on 2026-10-09.
     */
    executeCompatibilityAction(request: CompatibilityComputerActionRequest): Promise<Record<string, unknown>>;
    /** Compatibility-only physical browser session creation; logical ownership remains canonical. */
    createCompatibilitySession(): Promise<{
        session_id: string;
    }>;
    listCanonicalProviders(): Promise<CanonicalComputerCapabilityManifest[]>;
    /**
     * Canonical provider catalog. Since the D0 cleanup (2026-10-09) the gateway
     * registers no per-backend canonical providers, so this returns an empty
     * catalog. The observe / roots / transactions / approvals / environment /
     * lease / history methods were removed with their routes; use
     * `@allternit/computer-driver` for hosted computers.
     */
    getCanonicalProviderCatalog(): Promise<CanonicalProviderCatalog>;
    getCanonicalEvents(sessionId: string, afterSequence?: number): Promise<unknown>;
    getCanonicalTrajectory(sessionId: string): Promise<Record<string, unknown>>;
    watch(options: WatchOptions): Promise<Response>;
    getReceipts(runId: string): Promise<unknown>;
    getSnapshot(runId: string): Promise<unknown>;
    approveRun(runId: string, options?: ApprovalOptions): Promise<unknown>;
    denyRun(runId: string, options?: ApprovalOptions): Promise<unknown>;
    /** Guidance for a running planning loop; it reads it before its next step. */
    steerRun(runId: string, text: string): Promise<{
        run_id: string;
        accepted: boolean;
    }>;
    cancelRun(runId: string, options?: CancelOptions): Promise<unknown>;
    captureRunScreenshot(runId: string): Promise<{
        screenshot_b64?: string;
    }>;
    pauseRun(runId: string, options?: CancelOptions): Promise<unknown>;
    resumeRun(runId: string, options?: ResumeOptions): Promise<unknown>;
    watchRun(runId: string, options?: WatchRunOptions): AsyncGenerator<any, void, unknown>;
    /**
     * List compiled browser-workflow specs (distilled summaries, shapes only).
     * GET /v1/browser-skills
     */
    listBrowserSkills(): Promise<{
        specs: BrowserSkillSpecSummary[];
        count: number;
        skills_dir: string;
    }>;
    /** Inspect one spec's distilled shape, incl. its taught NetworkTrace. */
    getBrowserSkill(skillId: string): Promise<{
        skill_id: string;
        workflow: BrowserSkillSpecDetail;
    }>;
    /**
     * Run the deterministic record → teach → batch → verify chain. Without a
     * targetUrl this is the canned self-check; with one, the spec'd workflow is
     * batch-verified against that URL. Poll with getBrowserSkillVerify.
     */
    startBrowserSkillVerify(options?: StartBrowserSkillVerifyOptions): Promise<{
        verify_id: string;
        status: string;
        mode?: string;
        poll?: string;
    }>;
    /** Fetch a stored verify verdict (network deviations, a11y, receipts). */
    getBrowserSkillVerify(verifyId: string): Promise<BrowserSkillVerifyResult>;
    /** Recompute the content-derived receipt hash (tamper check). */
    checkBrowserSkillVerifyReceipt(verifyId: string): Promise<BrowserSkillReceiptCheck>;
    waitForRun(runId: string, options?: WaitForRunOptions): Promise<{
        status?: string;
    }>;
}
/** @deprecated Use `@allternit/computer-driver`. */
export declare function createComputerUseClient(config?: RequestOptions): AllternitComputerUseClient;
export declare function resolveComputerUseBaseUrl(url?: string): string;
export type EngineEventBatch = unknown;
export type EngineEventRecord = unknown;
export type EngineExecutionRequestInput = ComputerUseRequest;
export type EngineReceiptsResponse = unknown;
export type EngineRunSnapshot = unknown;
