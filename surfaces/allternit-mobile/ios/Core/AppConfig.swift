import Foundation

/// Runtime configuration, sourced from Info.plist so environments can be
/// swapped via build settings / xcconfig without code changes.
///
/// - `ALLTERNIT_API_BASE_URL` — Allternit gateway base URL (no trailing slash).
/// - `CLERK_PUBLISHABLE_KEY` — Clerk publishable key (`pk_test_…` / `pk_live_…`).
///
/// Unset or unresolved `$(VAR)` values fall back to the dev defaults below.
/// There is intentionally no hardcoded production URL (the public gateway host
/// for mobile is still an open Phase-0 question).
enum AppConfig {
    /// Default: the local dev gateway.
    static let apiBaseURL: URL = {
        #if DEBUG
        // `-api-url <url>` (DEBUG only, mirrors `-cloud-url`): point every
        // API call at a locally running allternit-api — used by the D3 live
        // verification harness against a throwaway data dir + port.
        if let override = UserDefaults.standard.string(forKey: "api-url"),
           let url = URL(string: override) {
            return url
        }
        #endif
        if let value = infoPlistValue("ALLTERNIT_API_BASE_URL"),
           let url = URL(string: value) {
            return url
        }
        return URL(string: "http://127.0.0.1:8013/api/v1")!
    }()

    /// Base URL of the ACI (Agent-Computer-Interface) endpoints —
    /// `POST /api/aci/run`, `GET /api/aci/stream/:id`, `POST /api/aci/stop/:id`,
    /// `POST /api/aci/approve/:id`.
    ///
    /// The ACI router is mounted directly under `/api` on allternit-api
    /// (cmd/allternit-api/src/main.rs: `.nest("/api", aci_router())`) — NOT under
    /// the `/api/v1` router — so it can't be reached by appending a path to
    /// `apiBaseURL`. Derived by stripping a trailing `v1` path component when
    /// present, then appending `aci`.
    static let aciBaseURL: URL = {
        let apiBase = apiBaseURL.lastPathComponent == "v1"
            ? apiBaseURL.deletingLastPathComponent()
            : apiBaseURL
        return apiBase.appendingPathComponent("aci")
    }()

    /// Base URL of the Factory mail / ledger endpoints —
    /// `GET /api/factory/mail/threads`, `GET /api/factory/mail/thread/:id`,
    /// `POST /api/factory/mail/send`, `POST /api/factory/mail/decide`,
    /// `POST /api/factory/mail/share`, `POST /api/factory/ledger/tail`.
    ///
    /// Like ACI, the Factory core router is mounted directly under `/api` on
    /// allternit-api (cmd/allternit-api/src/main.rs:
    /// `.nest("/api/factory", factory_core_router())`) — NOT under the
    /// `/api/v1` router — so it can't be reached by appending a path to
    /// `apiBaseURL`. Derived the same way as `aciBaseURL`.
    static let factoryBaseURL: URL = {
        let apiBase = apiBaseURL.lastPathComponent == "v1"
            ? apiBaseURL.deletingLastPathComponent()
            : apiBaseURL
        return apiBase.appendingPathComponent("factory")
    }()

    /// Base URL of a standalone gizzi-code server (`gizzi serve`) hosting the
    /// pty REST + WebSocket routes (`/v1/pty/...`, see
    /// cmd/gizzi-code/docs/pty-websocket-protocol.md). Default: the local dev
    /// server. Terminal connections always go DIRECTLY here — the cloud relay
    /// envelope cannot carry a WebSocket upgrade.
    static let gizziCodeBaseURL: URL = {
        #if DEBUG
        // `-gizzi-url <url>` (DEBUG only): point terminal/session calls at a
        // specific gizzi-code server without a registered instance.
        if let override = UserDefaults.standard.string(forKey: "gizzi-url"),
           let url = URL(string: override) {
            return url
        }
        #endif
        if let value = infoPlistValue("ALLTERNIT_GIZZI_CODE_URL"),
           let url = URL(string: value) {
            return url
        }
        return URL(string: "http://127.0.0.1:4096")!
    }()

    /// Whether `gizziCodeBaseURL` is a real target: true when the build
    /// setting resolved, plus DEBUG builds (which legitimately fall back to
    /// the local dev server). Release ships `ALLTERNIT_GIZZI_CODE_URL`
    /// empty, so there the static default is just a placeholder — callers
    /// must treat the terminal host as unresolved (InstanceStore is the
    /// only real source) instead of connecting to the dev fallback.
    static let hasUsableGizziCodeURL: Bool = {
        if let value = infoPlistValue("ALLTERNIT_GIZZI_CODE_URL") {
            return URL(string: value) != nil
        }
        #if DEBUG
        return true
        #else
        return false
        #endif
    }()

    /// Headscale control-plane URL for the embedded mesh (tsnet) node
    /// (Core/Mesh/MeshClient.swift). Set via `ALLTERNIT_MESH_CONTROL_URL`;
    /// falls back to the live Headscale instance.
    static let meshControlURL: String = {
        if let value = infoPlistValue("ALLTERNIT_MESH_CONTROL_URL") {
            return value
        }
        return "https://allternit-headscale.fly.dev"
    }()

    /// Empty until a real publishable key is provided — AuthManager treats an
    /// empty key as "Clerk not configured" instead of crashing.
    static let clerkPublishableKey: String = infoPlistValue("CLERK_PUBLISHABLE_KEY") ?? ""

    /// Allternit Cloud API host — the runtime-device registry and the relay
    /// entry point. Default mirrors the web's `cloudApiBaseUrl()` fallback
    /// (surfaces/ai.allternit.com/src/lib/fetch-interceptor.ts:85-87);
    /// overridable via `ALLTERNIT_CLOUD_API_URL` in Info.plist.
    static let cloudAPIBaseURL: URL = {
        #if DEBUG
        // `-cloud-url <url>` (DEBUG only): point cloud calls (handoff claim,
        // runtime relay) at a locally running allternit-cloud-api.
        if let override = UserDefaults.standard.string(forKey: "cloud-url"),
           let url = URL(string: override) {
            return url
        }
        #endif
        if let value = infoPlistValue("ALLTERNIT_CLOUD_API_URL"),
           let url = URL(string: value) {
            return url
        }
        return URL(string: "https://api.allternit.com")!
    }()

    /// Relay proxy URL for a paired runtime device:
    /// `{cloud}/api/v1/runtime-devices/<id>/proxy`
    /// (fetch-interceptor.ts:450-453).
    ///
    /// NOTE: the original request path is NOT appended after `/proxy` — the
    /// relay is always a POST whose JSON body carries the original call as
    /// `{ method, path, headers, body, bodyEncoding }`
    /// (fetch-interceptor.ts:454-463). See APIClient.relayRequest(for:proxyURL:).
    static func runtimeProxyURL(runtimeId: String) -> URL {
        let escaped = runtimeId.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? runtimeId
        return cloudAPIBaseURL.appendingPathComponent("api/v1/runtime-devices/\(escaped)/proxy")
    }

    /// Returns nil for missing, empty, or unresolved-`$(VARIABLE)` values.
    private static func infoPlistValue(_ key: String) -> String? {
        guard let value = Bundle.main.object(forInfoDictionaryKey: key) as? String,
              !value.isEmpty,
              !value.hasPrefix("$(") else {
            return nil
        }
        return value
    }
}
