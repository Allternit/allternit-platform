/**
 * The environment an agent-run shell command gets.
 *
 * gizzi's own process holds every provider credential it was started with
 * (ANTHROPIC_API_KEY, OPENROUTER_API_KEY, GITHUB_TOKEN, …). Handing all of
 * `process.env` to each bash tool call gives every command, and anything it
 * downloads and runs, those keys. Instead the child gets:
 *
 *   1. an allowlist of the variables shells and toolchains need (PATH, HOME,
 *      locale, TERM, TMPDIR, SSH agent, proxies/CA bundles, and the
 *      node/nvm/pnpm/bun, rust/cargo, go, python, ruby, java, homebrew, git
 *      and XDG families),
 *   2. minus anything that looks like a credential (…_KEY, …_TOKEN,
 *      …_SECRET, …PASSWORD, …_PAT, …CREDENTIALS) even inside an allowed
 *      family — e.g. HOMEBREW_GITHUB_API_TOKEN,
 *   3. plus the variables the user opted in to, by name or `PREFIX_*` glob,
 *      via `bash.env_passthrough` in gizzi config or the comma-separated
 *      GIZZI_BASH_ENV_PASSTHROUGH env var. An opt-in beats the credential
 *      filter: `"env_passthrough": ["NPM_TOKEN"]` passes NPM_TOKEN,
 *   4. plus whatever `shell.env` plugins set (they chose to set it).
 */
export namespace ShellEnv {
  /** Exact names always passed through. */
  const NAMES = new Set([
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "COLORTERM",
    "NO_COLOR",
    "FORCE_COLOR",
    "CLICOLOR",
    "TMPDIR",
    "TMP",
    "TEMP",
    "TZ",
    "PWD",
    "OLDPWD",
    "SHLVL",
    "HOSTNAME",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "LESS",
    "MANPATH",
    "INFOPATH",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "GPG_TTY",
    "CI",
    "__CF_USER_TEXT_ENCODING",
    "COMMAND_MODE",
    // Proxies and CA bundles: without them network tools fail behind a proxy.
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "NODE_EXTRA_CA_CERTS",
    // Toolchain homes.
    "PNPM_HOME",
    "BUN_INSTALL",
    "VOLTA_HOME",
    "DENO_DIR",
    "DENO_INSTALL",
    "COREPACK_HOME",
    "JAVA_HOME",
    "ANDROID_HOME",
    "ANDROID_SDK_ROOT",
    "GRADLE_USER_HOME",
    "MAVEN_HOME",
    "VIRTUAL_ENV",
    "PYTHONPATH",
    "PYTHONHOME",
    "PIPX_HOME",
    "PIPX_BIN_DIR",
    "UV_CACHE_DIR",
    "POETRY_HOME",
    "DOCKER_HOST",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    "KUBECONFIG",
    "SDKROOT",
    "DEVELOPER_DIR",
    "CC",
    "CXX",
    "CFLAGS",
    "CXXFLAGS",
    "LDFLAGS",
    "CPPFLAGS",
    "PKG_CONFIG_PATH",
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "GOPATH",
    "GOROOT",
    "GOBIN",
    "GOFLAGS",
    "GOPROXY",
    "GOPRIVATE",
    "GONOPROXY",
    "GONOSUMDB",
    "GOSUMDB",
    "GOINSECURE",
    "GOCACHE",
    "GOMODCACHE",
    "GOENV",
    "GOTOOLCHAIN",
    "GOOS",
    "GOARCH",
  ])

  /** Families passed through by prefix (credential-looking names still stripped). */
  const PREFIXES = [
    "LC_",
    "XDG_",
    "NVM_",
    "NODE_",
    "npm_config_",
    "NPM_CONFIG_",
    "PNPM_",
    "BUN_",
    "YARN_",
    "CARGO_",
    "RUSTUP_",
    "RUST",
    "PYENV_",
    "CONDA_",
    "PIP_",
    "RBENV_",
    "GEM_",
    "BUNDLE_",
    "ASDF_",
    "MISE_",
    "SDKMAN_",
    "HOMEBREW_",
    "GIT_",
    "GIZZI_",
    // gizzi exports its CommRails peer name / inbox for commands to use.
    "ALLTERNIT_",
  ]

  /** Credential-looking names: stripped unless explicitly opted in. */
  const SECRET = /(_KEY|_TOKEN|_SECRET|PASSWORD|PASSWD|_PAT|_CREDENTIALS?|_AUTH|AUTHTOKEN)$|SECRET|TOKEN|API_?KEY|PRIVATE_KEY/i

  export function isSecret(name: string): boolean {
    return SECRET.test(name)
  }

  function allowed(name: string): boolean {
    if (NAMES.has(name)) return true
    return PREFIXES.some((p) => name.startsWith(p))
  }

  function matcher(passthrough: string[]): (name: string) => boolean {
    const exact = new Set<string>()
    const prefixes: string[] = []
    for (const raw of passthrough) {
      const entry = raw.trim()
      if (!entry) continue
      if (entry.endsWith("*")) prefixes.push(entry.slice(0, -1))
      else exact.add(entry)
    }
    return (name) => exact.has(name) || prefixes.some((p) => name.startsWith(p))
  }

  /** Opt-ins from GIZZI_BASH_ENV_PASSTHROUGH (comma separated). */
  export function fromFlag(source: NodeJS.ProcessEnv = process.env): string[] {
    return (source.GIZZI_BASH_ENV_PASSTHROUGH ?? "").split(",").map((s) => s.trim()).filter(Boolean)
  }

  /**
   * Build a child command's environment from the host env, the opt-in list
   * and plugin-provided `shell.env` values.
   */
  export function child(
    source: NodeJS.ProcessEnv,
    extra: Record<string, string | undefined> = {},
    passthrough: string[] = [],
  ): Record<string, string> {
    const optedIn = matcher([...passthrough, ...fromFlag(source)])
    const env: Record<string, string> = {}
    for (const [name, value] of Object.entries(source)) {
      if (value === undefined) continue
      if (optedIn(name) || (allowed(name) && !isSecret(name))) env[name] = value
    }
    for (const [name, value] of Object.entries(extra)) {
      if (value !== undefined) env[name] = value
    }
    return env
  }
}
