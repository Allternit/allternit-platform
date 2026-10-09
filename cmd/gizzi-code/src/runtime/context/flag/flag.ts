import { CLOUD_URLS, envOr } from "@/shared/constants/cloudUrls"

function truthy(key: string) {
  const value = process.env[key]?.toLowerCase()
  return value === "true" || value === "1"
}

function env(key: string) {
  return process.env[key]
}

export namespace Flag {
  export const GIZZI_AUTO_SHARE = truthy("GIZZI_AUTO_SHARE")
  export const GIZZI_GIT_BASH_PATH = env("GIZZI_GIT_BASH_PATH")
  export const GIZZI_CONFIG = env("GIZZI_CONFIG")
  // Dynamic getter below (defined after the namespace) — tests and CLI flags
  // set this mid-process, after this module has already been imported, so it
  // can't be a frozen const evaluated once at import time.
  export declare const GIZZI_CONFIG_DIR: string | undefined
  export const GIZZI_CONFIG_CONTENT = env("GIZZI_CONFIG_CONTENT")
  export const GIZZI_DISABLE_AUTOUPDATE = truthy("GIZZI_DISABLE_AUTOUPDATE")
  export const GIZZI_DISABLE_PRUNE = truthy("GIZZI_DISABLE_PRUNE")
  export const GIZZI_DISABLE_TERMINAL_TITLE = truthy("GIZZI_DISABLE_TERMINAL_TITLE")
  export const GIZZI_PERMISSION = env("GIZZI_PERMISSION")
  export const GIZZI_DISABLE_DEFAULT_PLUGINS = truthy("GIZZI_DISABLE_DEFAULT_PLUGINS")
  export const GIZZI_DISABLE_LSP_DOWNLOAD = truthy("GIZZI_DISABLE_LSP_DOWNLOAD")
  export const GIZZI_ENABLE_EXPERIMENTAL_MODELS = truthy("GIZZI_ENABLE_EXPERIMENTAL_MODELS")
  export const GIZZI_DISABLE_AUTOCOMPACT = truthy("GIZZI_DISABLE_AUTOCOMPACT")
  export const GIZZI_DISABLE_MODELS_FETCH = truthy("GIZZI_DISABLE_MODELS_FETCH")
  export const GIZZI_DISABLE_LEGACY_INSTRUCTIONS = truthy("GIZZI_DISABLE_LEGACY_INSTRUCTIONS")
  export const GIZZI_DISABLE_LEGACY_PROMPT =
    GIZZI_DISABLE_LEGACY_INSTRUCTIONS || truthy("GIZZI_DISABLE_LEGACY_PROMPT")
  export const GIZZI_DISABLE_LEGACY_SKILLS =
    GIZZI_DISABLE_LEGACY_INSTRUCTIONS || truthy("GIZZI_DISABLE_LEGACY_SKILLS")
  export const GIZZI_DISABLE_EXTERNAL_SKILLS =
    GIZZI_DISABLE_LEGACY_SKILLS || truthy("GIZZI_DISABLE_EXTERNAL_SKILLS")
  export declare const GIZZI_DISABLE_PROJECT_CONFIG: boolean
  export const GIZZI_FAKE_VCS = env("GIZZI_FAKE_VCS")
  export declare const GIZZI_CLIENT: string
  export const GIZZI_SERVER_PASSWORD = env("GIZZI_SERVER_PASSWORD")
  export const GIZZI_SERVER_USERNAME = env("GIZZI_SERVER_USERNAME")
  // Clerk JWT auth for the standalone server (phase 1 iOS direct-connect).
  // GIZZI_REQUIRE_CLERK_AUTH makes Bearer-token validation mandatory for every
  // request (except OPTIONS preflight); without it, JWT is only checked when a
  // Bearer token is presented, and password/loopback rules apply otherwise.
  export const GIZZI_CLERK_JWKS_URL = env("GIZZI_CLERK_JWKS_URL")
  export const GIZZI_CLERK_ISSUER = env("GIZZI_CLERK_ISSUER")
  export const GIZZI_REQUIRE_CLERK_AUTH = truthy("GIZZI_REQUIRE_CLERK_AUTH")
  // Override for the allternit-cloud-api token-validation endpoint used to
  // authenticate durable `alt_` gateway tokens (default
  // https://api.allternit.com/api/v1/auth/validate).
  export const GIZZI_TOKEN_VALIDATE_URL = env("GIZZI_TOKEN_VALIDATE_URL")
  // Development-only: reflect any Origin in Access-Control-Allow-Origin.
  // Never set in production — the server CORS policy is an allowlist
  // (loopback, tauri, *.gizzi.dev, --cors/config entries) by default.
  // Dynamic getter (defined after the namespace) — tests toggle this
  // mid-process, so it can't be a frozen const evaluated once at import time.
  export declare const GIZZI_DEV_CORS: boolean
  // Path override for the cloudflared binary used by `gizzi serve --tunnel`.
  export const GIZZI_CLOUDFLARED_BIN = env("GIZZI_CLOUDFLARED_BIN")
  // Named-tunnel token (`cloudflared tunnel token <name>` / Zero Trust
  // dashboard). When set, `gizzi serve` runs a named tunnel on the user's own
  // Cloudflare account instead of a quick tunnel.
  export const GIZZI_TUNNEL_TOKEN = env("GIZZI_TUNNEL_TOKEN")
  // Public hostname mapped to the named tunnel. Only used for logs and
  // instance registration — cloudflared learns the routing from the token.
  export const GIZZI_TUNNEL_HOSTNAME = env("GIZZI_TUNNEL_HOSTNAME")
  // Path overrides for the tailscale binaries used by `gizzi serve --mesh`.
  export const GIZZI_TAILSCALE_BIN = env("GIZZI_TAILSCALE_BIN")
  export const GIZZI_TAILSCALED_BIN = env("GIZZI_TAILSCALED_BIN")
  // Path override for the mesh-node tsnet sidecar used by `gizzi serve --mesh`
  // (built by infrastructure/mesh/tsnet-ios/build-sidecar.sh into
  // vendor/mesh-node/<platform>-<arch>/).
  export const GIZZI_MESH_NODE_BIN = env("GIZZI_MESH_NODE_BIN")
  // Tailscale/Headscale preauth key for `gizzi serve --mesh` (secret — prefer
  // this env var over storing it in a config file). Implies mesh mode.
  export const GIZZI_MESH_AUTH_KEY = env("GIZZI_MESH_AUTH_KEY")
  // Headscale coordination server URL for `gizzi serve --mesh`. The default
  // (https://mail.news.allternit.com:8444) lives in Mesh.DEFAULT_CONTROL_URL.
  export const GIZZI_MESH_CONTROL_URL = env("GIZZI_MESH_CONTROL_URL")
  // Platform instance registry base URL. `gizzi serve --tunnel` PUTs its public
  // tunnel URL here so signed-in clients (iOS app) can discover the instance.
  export const GIZZI_PLATFORM_API_URL = envOr("GIZZI_PLATFORM_API_URL", CLOUD_URLS.api)
  export const GIZZI_ENABLE_QUESTION_TOOL = truthy("GIZZI_ENABLE_QUESTION_TOOL")
  // Emergency rollback switches for the Kimi-parity runtime rollout. New
  // installations keep these capabilities enabled; flags only suppress the
  // corresponding projection/surface without deleting durable data.
  export const GIZZI_DISABLE_DURABLE_TRACE = truthy("GIZZI_DISABLE_DURABLE_TRACE")
  export const GIZZI_DISABLE_CONTEXT_PROJECTION = truthy("GIZZI_DISABLE_CONTEXT_PROJECTION")
  export const GIZZI_DISABLE_ACP_CONFIG_OPTIONS = truthy("GIZZI_DISABLE_ACP_CONFIG_OPTIONS")
  export const GIZZI_DYNAMIC_TOOL_SELECTION = truthy("GIZZI_DYNAMIC_TOOL_SELECTION")
  export const GIZZI_DISABLE_SCRATCHPAD = truthy("GIZZI_DISABLE_SCRATCHPAD")
  // Hides artifact_create/update/read (Artifacts v2 model tools).
  export const GIZZI_DISABLE_ARTIFACT_TOOLS = truthy("GIZZI_DISABLE_ARTIFACT_TOOLS")

  // Permission modes (set from CLI flags)
  export let GIZZI_PERMISSION_MODE: string | undefined = env("GIZZI_PERMISSION_MODE")
  export let GIZZI_SKIP_PERMISSIONS: boolean = truthy("GIZZI_SKIP_PERMISSIONS")

  // Worktree override (set from --worktree CLI flag)
  export let GIZZI_WORKTREE: string | undefined = env("GIZZI_WORKTREE")

  // Fallback model (set from --fallback-model CLI flag)
  export let GIZZI_FALLBACK_MODEL: string | undefined = env("GIZZI_FALLBACK_MODEL")

  // Experimental
  export const GIZZI_EXPERIMENTAL = truthy("GIZZI_EXPERIMENTAL")
  export const GIZZI_EXPERIMENTAL_FILEWATCHER = truthy("GIZZI_EXPERIMENTAL_FILEWATCHER")
  export const GIZZI_EXPERIMENTAL_DISABLE_FILEWATCHER = truthy("GIZZI_EXPERIMENTAL_DISABLE_FILEWATCHER")
  export const GIZZI_EXPERIMENTAL_ICON_DISCOVERY =
    GIZZI_EXPERIMENTAL || truthy("GIZZI_EXPERIMENTAL_ICON_DISCOVERY")

  const copy = env("GIZZI_EXPERIMENTAL_DISABLE_COPY_ON_SELECT")
  export const GIZZI_EXPERIMENTAL_DISABLE_COPY_ON_SELECT =
    copy === undefined ? process.platform === "win32" : truthy("GIZZI_EXPERIMENTAL_DISABLE_COPY_ON_SELECT")
  export const GIZZI_ENABLE_EXA =
    truthy("GIZZI_ENABLE_EXA") || GIZZI_EXPERIMENTAL || truthy("GIZZI_EXPERIMENTAL_EXA")
  export const GIZZI_EXPERIMENTAL_BASH_DEFAULT_TIMEOUT_MS = number("GIZZI_EXPERIMENTAL_BASH_DEFAULT_TIMEOUT_MS")
  export const GIZZI_EXPERIMENTAL_OUTPUT_TOKEN_MAX = number("GIZZI_EXPERIMENTAL_OUTPUT_TOKEN_MAX")
  export const GIZZI_EXPERIMENTAL_OXFMT = GIZZI_EXPERIMENTAL || truthy("GIZZI_EXPERIMENTAL_OXFMT")
  export const GIZZI_EXPERIMENTAL_LSP_TY = truthy("GIZZI_EXPERIMENTAL_LSP_TY")
  // LSP navigation tool (goToDefinition/findReferences/documentSymbol/etc.) ships
  // enabled by default; set this to opt out (mirrors GIZZI_DISABLE_BROWSER_TOOL).
  export const GIZZI_ENABLE_LSP_TOOL = !truthy("GIZZI_DISABLE_LSP_TOOL")
  export const GIZZI_ENABLE_BROWSER_TOOL = !truthy("GIZZI_DISABLE_BROWSER_TOOL")
  // Computer toolset `computer` tool (screen/mouse/keyboard of GIZZI_COMPUTER_ID)
  // plus the `computers` lifecycle/files tool.
  export const GIZZI_ENABLE_COMPUTER_TOOL = truthy("GIZZI_ENABLE_COMPUTER_TOOL")
  export const GIZZI_DISABLE_FILETIME_CHECK = truthy("GIZZI_DISABLE_FILETIME_CHECK")
  export const GIZZI_EXPERIMENTAL_PLAN_MODE = GIZZI_EXPERIMENTAL || truthy("GIZZI_EXPERIMENTAL_PLAN_MODE")
  export const GIZZI_EXPERIMENTAL_MARKDOWN = truthy("GIZZI_EXPERIMENTAL_MARKDOWN")
  export const GIZZI_MODELS_URL = env("GIZZI_MODELS_URL")
  export const GIZZI_MODELS_PATH = env("GIZZI_MODELS_PATH")

  // Sandbox — OS-level subprocess isolation (bwrap on Linux, sandbox-exec on macOS)
  // is on by default for every Bash tool call. GIZZI_SANDBOX is kept as an
  // explicit "force on" signal for other call sites (cowork runtime, onboarding
  // UI); it is no longer required to enable sandboxing in bash.ts.
  export let GIZZI_SANDBOX = truthy("GIZZI_SANDBOX")
  // Explicit, intentional opt-out — mirrors --dangerously-skip-permissions.
  // Set this (or pass --dangerously-skip-sandbox) to run Bash fully unsandboxed.
  export let GIZZI_SANDBOX_DISABLE: boolean = truthy("GIZZI_SANDBOX_DISABLE")
  // When sandbox is on, allow outbound network. Default: denied — agents that
  // need npm/pip/cargo must opt in explicitly (matches gizzi-code's default-deny).
  export let GIZZI_SANDBOX_ALLOW_NETWORK = truthy("GIZZI_SANDBOX_ALLOW_NETWORK")
  // Comma-separated hostname allowlist. When set (and network is allowed),
  // outbound traffic is restricted to these domains via a local proxy instead
  // of a wholesale allow — e.g. "registry.npmjs.org,pypi.org".
  export let GIZZI_SANDBOX_ALLOWED_DOMAINS = list("GIZZI_SANDBOX_ALLOWED_DOMAINS")

  // Cowork VM runtime endpoint (allternit-api POST /sandbox/execute)
  export const GIZZI_SANDBOX_RUNTIME_URL = env("GIZZI_SANDBOX_RUNTIME_URL")

  // VM session mode — provision a full VM per gizzi-code session (like CC cloud sessions).
  // When set, every new agent session gets a dedicated VM. All Bash tool calls execute
  // inside the VM rather than on the host. Project dir is shared in via bind mount / VirtioFS.
  export const GIZZI_VM_SESSIONS = truthy("GIZZI_VM_SESSIONS")
  // allternit-api base URL for VM session API (POST /vm-session etc.)
  // Defaults to GIZZI_SANDBOX_RUNTIME_URL if not set separately.
  export const GIZZI_VM_API_URL = env("GIZZI_VM_API_URL") ?? env("GIZZI_SANDBOX_RUNTIME_URL")

  function number(key: string) {
    const value = env(key)
    if (!value) return undefined
    const parsed = Number(value)
    return Number.isInteger(parsed) && parsed > 0 ? parsed : undefined
  }

  function list(key: string): string[] | undefined {
    const value = env(key)
    if (!value) return undefined
    const entries = value
      .split(",")
      .map((entry) => entry.trim())
      .filter(Boolean)
    return entries.length ? entries : undefined
  }
}

// Dynamic getter for GIZZI_CONFIG_DIR
Object.defineProperty(Flag, "GIZZI_CONFIG_DIR", {
  get() {
    return env("GIZZI_CONFIG_DIR")
  },
  enumerable: true,
  configurable: false,
})

// Dynamic getter for GIZZI_DISABLE_PROJECT_CONFIG
Object.defineProperty(Flag, "GIZZI_DISABLE_PROJECT_CONFIG", {
  get() {
    return truthy("GIZZI_DISABLE_PROJECT_CONFIG")
  },
  enumerable: true,
  configurable: false,
})

// Dynamic getter for GIZZI_CLIENT
Object.defineProperty(Flag, "GIZZI_CLIENT", {
  get() {
    return env("GIZZI_CLIENT") ?? "cli"
  },
  enumerable: true,
  configurable: false,
})

// Dynamic getter for GIZZI_DEV_CORS
Object.defineProperty(Flag, "GIZZI_DEV_CORS", {
  get() {
    return truthy("GIZZI_DEV_CORS")
  },
  enumerable: true,
  configurable: false,
})
