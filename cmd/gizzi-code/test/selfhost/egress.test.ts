import { describe, expect, test } from "bun:test"
import { $ } from "bun"
import path from "path"
import { gitHostOf, installGitPolicy, refAllowed, sandboxPolicyFor } from "../../src/runtime/selfhost/egress"
import { hostAllowed } from "../../src/runtime/integrations/shell/network-proxy"
import { tmpdir } from "../fixture/fixture"

// Environment egress policy on the worker (selfhost-v1.yaml EnvironmentPolicy).

describe("sandbox policy from an environment policy", () => {
  test("none blocks the network; an unknown mode fails closed", () => {
    expect(sandboxPolicyFor({ network: "none" }, "/workspace").allowNetwork).toBe(false)
    expect(sandboxPolicyFor({ network: "open-sesame" }, "/workspace").allowNetwork).toBe(false)
  })

  test("limited allows only the listed hosts, exact match; an empty list means no network", () => {
    const p = sandboxPolicyFor({ network: "limited", allowed_hosts: ["PyPI.org", "https://files.example.com/x", "git.example.com:8443"] }, "/w")
    expect(p.allowNetwork).toBe(true)
    expect(p.allowedDomainsExact).toBe(true)
    expect(p.allowedDomains).toEqual(["pypi.org", "files.example.com", "git.example.com:8443"])
    expect(sandboxPolicyFor({ network: "limited", allowed_hosts: [] }, "/w").allowNetwork).toBe(false)
  })

  test("full opens the network with no allowlist", () => {
    const p = sandboxPolicyFor({ network: "full" }, "/w")
    expect(p.allowNetwork).toBe(true)
    expect(p.allowedDomains).toBeUndefined()
  })
})

describe("allowlist proxy, exact mode", () => {
  test("refuses subdomains and wrong ports", () => {
    const hosts = ["pypi.org", "git.example.com:8443"]
    expect(hostAllowed("pypi.org", hosts, { exact: true }, 443)).toBe(true)
    expect(hostAllowed("files.pypi.org", hosts, { exact: true }, 443)).toBe(false)
    expect(hostAllowed("git.example.com", hosts, { exact: true }, 8443)).toBe(true)
    expect(hostAllowed("git.example.com", hosts, { exact: true }, 443)).toBe(false)
    expect(hostAllowed("evil.com", hosts, { exact: true }, 443)).toBe(false)
    // The default (suffix) mode is unchanged.
    expect(hostAllowed("files.pypi.org", hosts, {}, 443)).toBe(true)
  })
})

describe("git policy", () => {
  test("remote URL hosts", () => {
    expect(gitHostOf("https://user:pw@GitHub.com/org/repo.git")).toBe("github.com")
    expect(gitHostOf("ssh://git@gitlab.example.com:2222/org/repo.git")).toBe("gitlab.example.com")
    expect(gitHostOf("git@github.com:org/repo.git")).toBe("github.com")
    expect(gitHostOf("/srv/repos/repo.git")).toBeUndefined()
    expect(gitHostOf("./relative/path:with-colon")).toBeUndefined()
    expect(gitHostOf("file:///srv/repo.git")).toBeUndefined()
  })

  test("push ref patterns", () => {
    expect(refAllowed("refs/heads/agent/fix-1", ["refs/heads/agent/*"])).toBe(true)
    expect(refAllowed("refs/heads/main", ["refs/heads/agent/*"])).toBe(false)
    expect(refAllowed("refs/heads/agent.x", ["refs/heads/agent/*"])).toBe(false)
  })

  test("the git shim refuses a push to a ref outside allowed_push_refs and allows a matching one", async () => {
    await using tmp = await tmpdir()
    const bare = path.join(tmp.path, "origin.git")
    const work = path.join(tmp.path, "work")
    await $`git init --bare -q ${bare}`.quiet()
    await $`git init -q ${work}`.quiet()
    await $`git -C ${work} -c user.email=t@example.com -c user.name=t commit -q --allow-empty -m root`.quiet()
    await $`git -C ${work} remote add origin ${bare}`.quiet()

    const realGit = Bun.which("git")
    const installed = await installGitPolicy({
      dir: path.join(tmp.path, "policy"),
      policy: { network: "none", git: { allowed_push_refs: ["refs/heads/agent/*"] } },
      realGit,
    })
    expect(installed).toBeDefined()
    const shim = path.join(installed!.binDir, "git")

    const refused = await $`${shim} -C ${work} push origin HEAD:refs/heads/main`.nothrow().quiet()
    expect(refused.exitCode).not.toBe(0)
    expect(refused.stderr.toString()).toContain("refs/heads/main is refused")

    const noVerify = await $`${shim} -C ${work} push --no-verify origin HEAD:refs/heads/main`.nothrow().quiet()
    expect(noVerify.exitCode).not.toBe(0)

    const allowed = await $`${shim} -C ${work} push origin HEAD:refs/heads/agent/fix-1`.nothrow().quiet()
    expect(allowed.exitCode).toBe(0)
    const format = "--format=%(refname)"
    const refs = await $`git -C ${bare} for-each-ref ${format}`.quiet().text()
    expect(refs.trim().split("\n")).toEqual(["refs/heads/agent/fix-1"])
  })

  test("the git shim refuses clone from a host outside allowed_hostnames", async () => {
    await using tmp = await tmpdir()
    const realGit = Bun.which("git")
    const installed = await installGitPolicy({
      dir: path.join(tmp.path, "policy"),
      policy: { network: "none", git: { allowed_hostnames: ["git.example.com"] } },
      realGit,
    })
    const shim = path.join(installed!.binDir, "git")
    const refused = await $`${shim} clone https://github.com/org/repo.git ${path.join(tmp.path, "c")}`.nothrow().quiet()
    expect(refused.exitCode).toBe(128)
    expect(refused.stderr.toString()).toContain("host_not_allowed: github.com")
  })
})
