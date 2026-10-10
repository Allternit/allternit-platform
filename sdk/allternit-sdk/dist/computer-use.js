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
export const COMPUTER_USE_CONTRACT_VERSION = "1.0.0-alpha.1";
/** @deprecated Use `@allternit/computer-driver`. */
export class AllternitComputerUseClient {
    baseUrl;
    fetch;
    headers;
    constructor(config = {}) {
        this.baseUrl = resolveComputerUseBaseUrl(config.baseUrl);
        this.fetch = config.fetch || globalThis.fetch;
        this.headers = config.headers || {};
    }
    async execute(request) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/execute`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify(request),
        });
        if (!response.ok) {
            throw new Error(`Computer use execution failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async executeStream(request) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/execute?stream=true`, {
            method: "POST",
            headers: { "Content-Type": "application/json", ...this.headers },
            body: JSON.stringify(request),
        });
        if (!response.ok)
            throw new Error(`Computer use stream failed: ${response.status} ${response.statusText}`);
        return response;
    }
    /**
     * One browser-session action on the gateway's `/v1/execute` channel
     * (`action`, `session_id`, optional `target`/`text`/`parameters`). Returns the
     * execute response (`status`, `summary`, `artifacts`, `extracted_content`).
     * The old `/v1/computer` route this used was removed on 2026-10-09.
     */
    async executeCompatibilityAction(request) {
        const { action, session_id, run_id, parameters, coordinate, text, key, target, goal, adapter_preference } = request;
        const params = { ...(parameters ?? {}) };
        if (coordinate)
            params.coordinate = coordinate;
        if (key !== undefined)
            params.key = key;
        const response = await this.fetch(`${this.baseUrl}/v1/execute`, {
            method: "POST",
            headers: { "Content-Type": "application/json", ...this.headers },
            body: JSON.stringify({
                action,
                session_id,
                run_id: run_id ?? `sdk-${globalThis.crypto?.randomUUID?.() ?? Date.now()}`,
                ...(target !== undefined ? { target } : {}),
                ...(goal !== undefined ? { goal } : {}),
                ...(text !== undefined ? { text } : {}),
                ...(adapter_preference !== undefined ? { adapter_preference } : {}),
                parameters: params,
            }),
        });
        if (!response.ok)
            throw new Error(`Compatibility action failed: ${response.status} ${response.statusText}`);
        return response.json();
    }
    /** Compatibility-only physical browser session creation; logical ownership remains canonical. */
    async createCompatibilitySession() {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/sessions`, {
            method: "POST",
            headers: { "Content-Type": "application/json", ...this.headers },
        });
        if (!response.ok)
            throw new Error(`Compatibility session creation failed: ${response.status} ${response.statusText}`);
        return response.json();
    }
    async listCanonicalProviders() {
        return (await this.getCanonicalProviderCatalog()).providers;
    }
    /**
     * Canonical provider catalog. Since the D0 cleanup (2026-10-09) the gateway
     * registers no per-backend canonical providers, so this returns an empty
     * catalog. The observe / roots / transactions / approvals / environment /
     * lease / history methods were removed with their routes; use
     * `@allternit/computer-driver` for hosted computers.
     */
    async getCanonicalProviderCatalog() {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/canonical/providers`, {
            method: "GET",
            headers: this.headers,
        });
        if (!response.ok)
            throw new Error(`Provider discovery failed: ${response.status} ${response.statusText}`);
        return response.json();
    }
    async getCanonicalEvents(sessionId, afterSequence = 0) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/canonical/sessions/${encodeURIComponent(sessionId)}/events?after_sequence=${afterSequence}`, { method: "GET", headers: this.headers });
        if (!response.ok)
            throw new Error(`Canonical event query failed: ${response.status} ${response.statusText}`);
        return response.json();
    }
    async getCanonicalTrajectory(sessionId) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/canonical/sessions/${encodeURIComponent(sessionId)}/trajectory`, { method: "GET", headers: this.headers });
        if (!response.ok)
            throw new Error(`Canonical trajectory failed: ${response.status} ${response.statusText}`);
        return response.json();
    }
    async watch(options) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${options.runId}/events`, {
            method: "GET",
            headers: this.headers,
            signal: options.signal,
        });
        if (!response.ok) {
            throw new Error(`Watch failed: ${response.status} ${response.statusText}`);
        }
        return response;
    }
    async getReceipts(runId) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}`, {
            method: "GET",
            headers: this.headers,
        });
        if (!response.ok) {
            throw new Error(`Get receipts failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async getSnapshot(runId) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}`, {
            method: "GET",
            headers: this.headers,
        });
        if (!response.ok) {
            throw new Error(`Get snapshot failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async approveRun(runId, options = {}) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/approve`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify({
                decision: "approve",
                ...options,
            }),
        });
        if (!response.ok) {
            throw new Error(`Approve run failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async denyRun(runId, options = {}) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/approve`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify({
                decision: "deny",
                ...options,
            }),
        });
        if (!response.ok) {
            throw new Error(`Deny run failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    /** Guidance for a running planning loop; it reads it before its next step. */
    async steerRun(runId, text) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/steer`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify({ text }),
        });
        if (!response.ok) {
            throw new Error(`Steer run failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async cancelRun(runId, options = {}) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/cancel`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify(options),
        });
        if (!response.ok) {
            throw new Error(`Cancel run failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async captureRunScreenshot(runId) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${encodeURIComponent(runId)}/screenshot`, {
            method: "POST", headers: this.headers,
        });
        if (!response.ok)
            throw new Error(`Screenshot failed: ${response.status} ${response.statusText}`);
        return response.json();
    }
    async pauseRun(runId, options = {}) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/pause`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify(options),
        });
        if (!response.ok) {
            throw new Error(`Pause run failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async resumeRun(runId, options = {}) {
        const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/resume`, {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                ...this.headers,
            },
            body: JSON.stringify(options),
        });
        if (!response.ok) {
            throw new Error(`Resume run failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async *watchRun(runId, options = {}) {
        const { intervalMs = 1000, signal } = options;
        let nextIndex = 0;
        while (!signal?.aborted) {
            const response = await this.fetch(`${this.baseUrl}/v1/computer-use/runs/${runId}/events?after_index=${nextIndex}`, {
                method: "GET",
                headers: this.headers,
                signal,
            });
            if (!response.ok) {
                throw new Error(`Watch run failed: ${response.status} ${response.statusText}`);
            }
            const batch = await response.json();
            yield batch;
            if (batch.completed)
                break;
            nextIndex = batch.next_index ?? nextIndex + 1;
            if (intervalMs > 0) {
                await new Promise((resolve) => setTimeout(resolve, intervalMs));
            }
        }
    }
    /**
     * List compiled browser-workflow specs (distilled summaries, shapes only).
     * GET /v1/browser-skills
     */
    async listBrowserSkills() {
        const response = await this.fetch(`${this.baseUrl}/v1/browser-skills`, {
            method: "GET",
            headers: this.headers,
        });
        if (!response.ok) {
            throw new Error(`List browser skills failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    /** Inspect one spec's distilled shape, incl. its taught NetworkTrace. */
    async getBrowserSkill(skillId) {
        const response = await this.fetch(`${this.baseUrl}/v1/browser-skills/${encodeURIComponent(skillId)}`, { method: "GET", headers: this.headers });
        if (!response.ok) {
            throw new Error(`Get browser skill failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    /**
     * Run the deterministic record → teach → batch → verify chain. Without a
     * targetUrl this is the canned self-check; with one, the spec'd workflow is
     * batch-verified against that URL. Poll with getBrowserSkillVerify.
     */
    async startBrowserSkillVerify(options = {}) {
        const body = {};
        if (options.skillId)
            body.skill_id = options.skillId;
        if (options.workflow)
            body.workflow = options.workflow;
        if (options.targetUrl)
            body.target_url = options.targetUrl;
        const response = await this.fetch(`${this.baseUrl}/v1/browser-skills/verify`, {
            method: "POST",
            headers: { "Content-Type": "application/json", ...this.headers },
            body: JSON.stringify(body),
        });
        if (!response.ok) {
            throw new Error(`Start browser skill verify failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    /** Fetch a stored verify verdict (network deviations, a11y, receipts). */
    async getBrowserSkillVerify(verifyId) {
        const response = await this.fetch(`${this.baseUrl}/v1/browser-skills/verify/${encodeURIComponent(verifyId)}`, { method: "GET", headers: this.headers });
        if (!response.ok) {
            throw new Error(`Get browser skill verify failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    /** Recompute the content-derived receipt hash (tamper check). */
    async checkBrowserSkillVerifyReceipt(verifyId) {
        const response = await this.fetch(`${this.baseUrl}/v1/browser-skills/verify/${encodeURIComponent(verifyId)}/receipt/check`, { method: "GET", headers: this.headers });
        if (!response.ok) {
            throw new Error(`Check browser skill receipt failed: ${response.status} ${response.statusText}`);
        }
        return response.json();
    }
    async waitForRun(runId, options = {}) {
        const { intervalMs = 1000, signal } = options;
        while (!signal?.aborted) {
            const snapshot = await this.getSnapshot(runId);
            if (snapshot.status === "needs_approval" ||
                snapshot.status === "paused" ||
                snapshot.status === "completed" ||
                snapshot.status === "failed" ||
                snapshot.status === "cancelled") {
                return snapshot;
            }
            if (intervalMs > 0) {
                await new Promise((resolve) => setTimeout(resolve, intervalMs));
            }
        }
        throw new Error("Wait for run was aborted");
    }
}
/** @deprecated Use `@allternit/computer-driver`. */
export function createComputerUseClient(config) {
    return new AllternitComputerUseClient(config);
}
export function resolveComputerUseBaseUrl(url) {
    if (!url) {
        return (process.env.ALLTERNIT_BASE_URL || process.env.GIZZI_SERVER_URL || "http://localhost:4096").replace(/\/+$/g, "");
    }
    return String(url).replace(/\/+$/g, "");
}
