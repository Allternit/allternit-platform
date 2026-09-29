import { describe, expect, test } from "bun:test"
import os from "os"
import { Catastrophic } from "../../src/runtime/tools/guard/permission/catastrophic"
import { PermissionNext } from "../../src/runtime/tools/guard/permission/next"

const HOME = os.homedir()

describe("catastrophic floor: denied", () => {
  const denied: string[] = [
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "rm -rf ~/",
    "rm -rf ~/*",
    'rm -rf "$HOME"',
    "rm -rf $HOME/",
    "rm -rf ${HOME}",
    `rm -rf ${HOME}`,
    "rm -fr /",
    "rm -r -f /",
    "rm --recursive --force /",
    "rm -rf --no-preserve-root /",
    "rm -rf /tmp/..",
    "rm -rf //",
    "sudo rm -rf /",
    "sudo -u root rm -rf /",
    "\\rm -rf ~",
    "/bin/rm -rf ~",
    "env FOO=1 rm -rf ~",
    "FOO=1 rm -rf ~",
    "nohup rm -rf / &",
    "cd /tmp && rm -rf ~",
    "echo hi; rm -rf /",
    'bash -c "rm -rf ~"',
    "sh -lc 'rm -rf /'",
    "eval 'rm -rf ~'",
    "echo $(rm -rf ~)",
    "mkfs.ext4 /dev/sda1",
    "mkfs -t ext4 /dev/sdb",
    "sudo diskutil eraseDisk APFS X /dev/disk2",
    "dd if=/dev/zero of=/dev/disk2 bs=1m",
    "dd if=/dev/zero of=/dev/rdisk2",
    "sudo dd if=image.iso of=/dev/sda",
    ":(){ :|:& };:",
    "bomb() { bomb | bomb & }; bomb",
    "shutdown -h now",
    "sudo reboot",
    "halt",
    "poweroff",
    "systemctl reboot",
    "git push --force origin main",
    "git push -f origin main",
    "git push -f origin master",
    "git push origin +main",
    "git push --force-with-lease origin main",
    "git push --force-with-lease=main:abc123 origin main",
    "git push origin +HEAD:main",
    "git push -f origin HEAD:refs/heads/main",
    "git -C repo push -f origin main",
    "git push -f",
    "git push --force origin",
    "git push -f origin HEAD",
    "git push --mirror origin",
    "git push origin :main",
    "git push origin --delete master",
    "security find-generic-password -s foo -w",
    "security find-internet-password -s github.com",
    "security dump-keychain",
    // Unparseable text that names a guarded program: deny, don't guess.
    'rm -rf "~',
    "git push -f origin 'main",
  ]
  for (const command of denied) {
    test(command, () => {
      expect(Catastrophic.check(command)).toBeDefined()
    })
  }
})

describe("catastrophic floor: allowed", () => {
  const allowed: string[] = [
    "rm -rf node_modules",
    "rm -rf ./dist",
    "rm -rf /tmp/build",
    "rm -rf ~/Library/Caches/foo",
    'rm -rf "$HOME/.cache/gizzi"',
    "rm -rf /tmp/{a,b}",
    "rm ~",
    "rm -f /",
    "echo rm -rf /",
    "echo 'rm -rf /'",
    "grep -r 'rm -rf /' .",
    "git push origin main",
    "git push origin feature",
    "git push -f origin feature/x",
    "git push --force-with-lease origin my-branch",
    "git push -u origin HEAD",
    "git status",
    "dd if=/dev/zero of=./disk.img bs=1m count=1",
    "security find-certificate -a",
    "find . -name '*.tmp' -exec rm {} \\;",
    "ls -la 2>&1 | head",
    "echo 'unbalanced",
    "npm test && npm run build",
  ]
  for (const command of allowed) {
    test(command, () => {
      expect(Catastrophic.check(command)).toBeUndefined()
    })
  }

  test("a tree-sitter parse error only denies when a guarded program is named", () => {
    expect(Catastrophic.check("echo ${", { parseError: true })).toBeUndefined()
    expect(Catastrophic.check("rm -rf build ${", { parseError: true })).toBeDefined()
  })

  test("home is resolved against the given home directory", () => {
    expect(Catastrophic.check("rm -rf /Users/someone", { home: "/Users/someone" })).toBeDefined()
    expect(Catastrophic.check("rm -rf /Users/someone/project", { home: "/Users/someone" })).toBeUndefined()
  })
})

describe("evaluatePolicy: floor and configured deny beat bypass", () => {
  test("rm -rf ~ is denied in bypassPermissions", () => {
    expect(PermissionNext.evaluatePolicy("bash", "rm -rf ~", { configured: [], mode: "bypassPermissions" }).action).toBe(
      "deny",
    )
  })

  test("the floor holds under GIZZI_SKIP_PERMISSIONS, yolo, auto, approvals and a configured allow", () => {
    const allowAll: PermissionNext.Ruleset = [{ permission: "*", pattern: "*", action: "allow" }]
    const approvals: PermissionNext.Ruleset = [{ permission: "bash", pattern: "*", action: "allow" }]
    for (const mode of ["default", "yolo", "auto", "acceptEdits", "bypassPermissions"]) {
      expect(
        PermissionNext.evaluatePolicy("bash", "git push -f origin main", { configured: allowAll, approvals, mode }).action,
      ).toBe("deny")
    }
    expect(
      PermissionNext.evaluatePolicy("bash", "sudo rm -rf /", { configured: allowAll, skipPermissions: true }).action,
    ).toBe("deny")
  })

  test("a configured deny is honored in bypassPermissions", () => {
    const configured = PermissionNext.fromConfig({ bash: { "*": "allow", "npm publish*": "deny" } })
    expect(
      PermissionNext.evaluatePolicy("bash", "npm publish --tag latest", { configured, mode: "bypassPermissions" }).action,
    ).toBe("deny")
    expect(
      PermissionNext.evaluatePolicy("bash", "npm publish --tag latest", { configured, skipPermissions: true }).action,
    ).toBe("deny")
  })

  test("a benign command is still allowed in bypassPermissions", () => {
    expect(PermissionNext.evaluatePolicy("bash", "ls -la", { configured: [], mode: "bypassPermissions" }).action).toBe(
      "allow",
    )
    expect(PermissionNext.evaluatePolicy("bash", "rm -rf node_modules", { configured: [], mode: "bypassPermissions" }).action).toBe(
      "allow",
    )
  })

  test("the floor only reads shell commands", () => {
    expect(PermissionNext.evaluatePolicy("edit", "rm -rf ~", { configured: [], mode: "bypassPermissions" }).action).toBe(
      "allow",
    )
  })

  test("FloorError explains the floor instead of blaming a user rule", () => {
    const error = new PermissionNext.FloorError("recursive rm of ~", "rm -rf ~")
    expect(error).toBeInstanceOf(PermissionNext.DeniedError)
    expect(error.message).toContain("safety floor")
    expect(error.message).toContain("recursive rm of ~")
  })
})
