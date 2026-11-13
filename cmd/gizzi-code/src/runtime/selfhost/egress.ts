/**
 * Worker-side egress policy for self-hosted environments (plan step 12).
 *
 * A work claim carries the environment's policy (selfhost-v1.yaml
 * `EnvironmentPolicy`). The worker applies it to every sandboxed command:
 *
 *   network: none     → the sandbox gets no network at all (bwrap
 *                       --unshare-net / Seatbelt network deny).
 *   network: limited  → only `allowed_hosts`, exact match, through the
 *                       CONNECT allowlist proxy (network-proxy.ts). An empty
 *                       list is the same as `none`.
 *   network: full     → open.
 *   git.allowed_hostnames / git.allowed_push_refs → a `git` shim first on the
 *                       sandbox PATH plus a pre-push hook. Clone, fetch, pull
 *                       and push to other hosts, and pushes to other refs,
 *                       are refused with a clear message.
 *
 * An unknown `network` value is treated as `none` (fail closed).
 *
 * The git shim is a guard against mistakes, not a wall: a command that calls
 * the real git binary by path skips it. Host limits are enforced by the
 * network rules above; ref limits should also be set as branch protection
 * on the git host. The operator guide says so.
 */
import fs from "node:fs/promises"
import path from "node:path"
import type { SandboxPolicy } from "@/runtime/integrations/shell/sandbox"

export type NetworkMode = "none" | "limited" | "full"

export interface EnvironmentPolicy {
  network: NetworkMode | string
  allowed_hosts?: string[]
  git?: { allowed_hostnames?: string[]; allowed_push_refs?: string[] }
  mcp?: { allowed_hostnames?: string[] }
  image?: string | null
  packages?: { apt?: string[]; pip?: string[]; npm?: string[] }
}

const HOST_RE = /^[a-z0-9.-]+(:\d{1,5})?$/
const REF_RE = /^[A-Za-z0-9._\/*+-]+$/

/** Lowercase, trim, drop scheme/path, keep an explicit port; drop anything that isn't a hostname. */
export function normalizeHosts(hosts: readonly string[] | undefined): string[] {
  const out = new Set<string>()
  for (const raw of hosts ?? []) {
    let h = String(raw ?? "").trim().toLowerCase()
    if (!h) continue
    h = h.replace(/^[a-z][a-z0-9+.-]*:\/\//, "").split("/")[0]!.replace(/\.$/, "")
    if (HOST_RE.test(h)) out.add(h)
  }
  return [...out]
}

export function effectiveNetwork(policy: Pick<EnvironmentPolicy, "network" | "allowed_hosts">): NetworkMode {
  if (policy.network === "full") return "full"
  if (policy.network === "limited") return normalizeHosts(policy.allowed_hosts).length ? "limited" : "none"
  return "none"
}

/** The sandbox policy (SessionSandbox.enable input) for an environment policy. */
export function sandboxPolicyFor(policy: EnvironmentPolicy, workdir: string, readPaths: string[] = []): Partial<SandboxPolicy> {
  const base: Partial<SandboxPolicy> = { allowWritePaths: [workdir], allowReadPaths: readPaths }
  switch (effectiveNetwork(policy)) {
    case "full":
      return { ...base, allowNetwork: true }
    case "limited":
      return {
        ...base,
        allowNetwork: true,
        allowedDomains: normalizeHosts(policy.allowed_hosts),
        allowedDomainsExact: true,
      }
    default:
      return { ...base, allowNetwork: false }
  }
}

/** Host of a git remote URL (https, ssh, git, scp-like `user@host:path`); undefined for a local path. */
export function gitHostOf(url: string): string | undefined {
  const u = url.trim()
  if (!u) return undefined
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(u)
  if (scheme) {
    if (scheme[1]!.toLowerCase() === "file") return undefined
    let rest = u.slice(scheme[0].length).split("/")[0]!
    rest = rest.slice(rest.lastIndexOf("@") + 1)
    if (rest.startsWith("[")) return rest.slice(1, rest.indexOf("]")).toLowerCase()
    return rest.split(":")[0]!.toLowerCase() || undefined
  }
  const colon = u.indexOf(":")
  if (colon > 0) {
    const before = u.slice(0, colon)
    if (before.includes("/")) return undefined
    const host = before.slice(before.lastIndexOf("@") + 1)
    return host.toLowerCase() || undefined
  }
  return undefined
}

/** Glob match for push refs: `*` matches any run of characters (slashes too). */
export function refAllowed(ref: string, patterns: readonly string[]): boolean {
  return patterns.some((pattern) => {
    const re = new RegExp("^" + pattern.split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$")
    return re.test(ref)
  })
}

const shq = (s: string) => `'${s.replace(/'/g, `'\\''`)}'`

function prelude(policy: EnvironmentPolicy): string {
  const hostsGiven = Array.isArray(policy.git?.allowed_hostnames)
  const hosts = normalizeHosts(policy.git?.allowed_hostnames).map((h) => h.split(":")[0]!)
  const refsGiven = Array.isArray(policy.git?.allowed_push_refs)
  const refs = (policy.git?.allowed_push_refs ?? []).map((r) => r.trim()).filter((r) => REF_RE.test(r))
  return `set -f
GIZZI_GIT_HOSTS_SET=${hostsGiven ? 1 : 0}
GIZZI_GIT_HOSTS=${shq(hosts.join(" "))}
GIZZI_GIT_REFS_SET=${refsGiven ? 1 : 0}
GIZZI_GIT_REFS=${shq(refs.join(" "))}
gizzi_deny() {
  echo "gizzi git policy: $1" >&2
  exit 128
}
gizzi_url_host() {
  case "$1" in
    file://*) ;;
    *://*)
      h=\${1#*://}; h=\${h%%/*}; h=\${h##*@}
      case "$h" in \\[*) h=\${h#\\[}; h=\${h%%]*};; *) h=\${h%%:*};; esac
      printf '%s' "$h" | tr 'A-Z' 'a-z';;
    *:*)
      p=\${1%%:*}
      case "$p" in */*) ;; *) printf '%s' "\${p##*@}" | tr 'A-Z' 'a-z';; esac;;
  esac
}
gizzi_host_ok() {
  [ "$GIZZI_GIT_HOSTS_SET" = 1 ] || return 0
  [ -n "$1" ] || return 0
  case " $GIZZI_GIT_HOSTS " in *" $1 "*) return 0;; esac
  return 1
}
gizzi_check_url() {
  h=$(gizzi_url_host "$1")
  gizzi_host_ok "$h" || gizzi_deny "host_not_allowed: $h is not in this environment's git.allowed_hostnames"
}
`
}

export function gitShimScript(policy: EnvironmentPolicy, realGit: string, hooksDir: string): string {
  return `#!/bin/sh
# Written by gizzi worker: this environment's git policy (self-hosted Agents Server).
${prelude(policy)}
GIZZI_REAL_GIT=${shq(realGit)}
for a in "$@"; do
  case "$a" in *core.hooksPath*|*core.hookspath*|*core.HooksPath*) gizzi_deny "overriding core.hooksPath is not allowed in this environment";; esac
done
sub=""; skip=0
for a in "$@"; do
  if [ "$skip" = 1 ]; then skip=0; continue; fi
  case "$a" in
    -c|-C|--git-dir|--work-tree|--namespace|--exec-path|--config-env|--super-prefix) skip=1;;
    -*) ;;
    *) sub="$a"; break;;
  esac
done
case "$sub" in
  clone)
    seen=0
    for a in "$@"; do
      if [ "$seen" = 0 ]; then [ "$a" = clone ] && seen=1; continue; fi
      case "$a" in -*) ;; *://*|*@*:*) gizzi_check_url "$a";; *:*) case "\${a%%:*}" in */*) ;; *) gizzi_check_url "$a";; esac;; esac
    done
    ;;
  fetch|pull|ls-remote|push)
    seen=0; remote=""
    for a in "$@"; do
      if [ "$seen" = 0 ]; then [ "$a" = "$sub" ] && seen=1; continue; fi
      case "$a" in --no-verify) [ "$sub" = push ] && gizzi_deny "push --no-verify is not allowed in this environment";; -*) ;; *) [ -z "$remote" ] && remote="$a";; esac
    done
    [ -n "$remote" ] || remote=origin
    case "$remote" in
      *://*|*@*:*) gizzi_check_url "$remote";;
      *) url=$("$GIZZI_REAL_GIT" remote get-url "$remote" 2>/dev/null) && [ -n "$url" ] && gizzi_check_url "$url";;
    esac
    if [ "$sub" = push ]; then
      exec "$GIZZI_REAL_GIT" -c core.hooksPath=${shq(hooksDir)} "$@"
    fi
    ;;
esac
exec "$GIZZI_REAL_GIT" "$@"
`
}

export function prePushHookScript(policy: EnvironmentPolicy): string {
  return `#!/bin/sh
# Written by gizzi worker: refuses pushes outside git.allowed_hostnames / git.allowed_push_refs.
${prelude(policy)}
gizzi_check_url "$2"
[ "$GIZZI_GIT_REFS_SET" = 1 ] || exit 0
while read local_ref local_sha remote_ref remote_sha; do
  [ -n "$remote_ref" ] || continue
  ok=0
  for pat in $GIZZI_GIT_REFS; do
    case "$remote_ref" in $pat) ok=1;; esac
  done
  [ "$ok" = 1 ] || gizzi_deny "push to $remote_ref is refused: it does not match git.allowed_push_refs ($GIZZI_GIT_REFS)"
done
exit 0
`
}

export interface InstalledGitPolicy {
  /** Put first on the sandbox PATH. */
  binDir: string
  hooksDir: string
  /** Directories the sandbox must be able to read. */
  readPaths: string[]
}

/** Write the git shim and pre-push hook under `dir`. Returns undefined when the policy has no git rules or git is missing. */
export async function installGitPolicy(input: {
  dir: string
  policy: EnvironmentPolicy
  realGit: string | null | undefined
}): Promise<InstalledGitPolicy | undefined> {
  const git = input.policy.git
  if (!git || (!Array.isArray(git.allowed_hostnames) && !Array.isArray(git.allowed_push_refs))) return undefined
  if (!input.realGit) return undefined
  const binDir = path.join(input.dir, "bin")
  const hooksDir = path.join(input.dir, "hooks")
  await fs.mkdir(binDir, { recursive: true, mode: 0o755 })
  await fs.mkdir(hooksDir, { recursive: true, mode: 0o755 })
  await fs.writeFile(path.join(binDir, "git"), gitShimScript(input.policy, input.realGit, hooksDir), { mode: 0o755 })
  await fs.writeFile(path.join(hooksDir, "pre-push"), prePushHookScript(input.policy), { mode: 0o755 })
  await fs.chmod(path.join(binDir, "git"), 0o755)
  await fs.chmod(path.join(hooksDir, "pre-push"), 0o755)
  return { binDir, hooksDir, readPaths: [input.dir] }
}
