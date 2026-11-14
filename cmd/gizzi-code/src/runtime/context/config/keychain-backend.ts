import { execFile } from "child_process"
import { getMacOsKeychainStorageServiceName, getUsername } from "@/shared/utils/secureStorage/macOsKeychainHelpers"
import type { KeyringBackend } from "./credential-store"

/**
 * Service-name suffix for the auth-profile keychain entry. Distinct from the
 * OAuth credentials entry (`CREDENTIALS_SERVICE_SUFFIX`) so the two never
 * share (and corrupt) the same keychain blob.
 */
const PROFILES_SERVICE_SUFFIX = "-profiles"

const SECURITY_TIMEOUT_MS = 10_000

type ExecFileImpl = typeof execFile

/**
 * The blob as `security find-generic-password -w` prints it. The entry is
 * written with `-X <hex>`, but `-w` prints a password whose bytes are
 * printable as plain text (JSON is), and only non-printable data as hex.
 * Reading only hex made every read empty, so each write replaced the whole
 * blob and every stored credential read back as missing.
 */
export function decodeBlob(out: string): Record<string, string> {
  for (const text of [out, /^[0-9a-f]+$/i.test(out) ? Buffer.from(out, "hex").toString("utf-8") : undefined]) {
    if (text === undefined) continue
    try {
      const parsed = JSON.parse(text)
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) return parsed as Record<string, string>
    } catch {}
  }
  return {}
}

/**
 * Keyring backend backed by the macOS Keychain (`security` CLI).
 *
 * Each credential `service` maps to one generic-password entry whose payload
 * is a hex-encoded JSON object `{ [account]: secret }`. Writes are read-
 * modify-write; concurrent CLI processes may interleave, which is acceptable
 * at login/logout frequency.
 *
 * All operations throw when not on darwin or when the `security` command
 * fails, so `"auto"` mode degrades to the marked plaintext fallback.
 */
export function createMacOSKeychainBackend(
  execFileImpl: ExecFileImpl = execFile,
): KeyringBackend {
  async function run(args: string[]): Promise<string | null> {
    if (process.platform !== "darwin") {
      throw new Error("macOS Keychain backend is only available on darwin")
    }
    return new Promise((resolve, reject) => {
      execFileImpl(
        "security",
        args,
        { encoding: "utf-8", timeout: SECURITY_TIMEOUT_MS },
        (err, stdout) => {
          // `security` exits 44 when the entry does not exist — a valid
          // "empty blob" result, not an error.
          if (err && (err as NodeJS.ErrnoException & { code?: number }).code === 44) {
            resolve("")
            return
          }
          if (err) {
            reject(err)
            return
          }
          resolve(stdout?.trim() || null)
        },
      )
    })
  }

  function serviceName(service: string): string {
    return `${getMacOsKeychainStorageServiceName(PROFILES_SERVICE_SUFFIX)}:${service}`
  }

  async function readBlob(service: string): Promise<Record<string, string>> {
    const out = await run([
      "find-generic-password",
      "-a",
      getUsername(),
      "-w",
      "-s",
      serviceName(service),
    ])
    if (!out) return {}
    return decodeBlob(out)
  }

  async function writeBlob(service: string, blob: Record<string, string>): Promise<void> {
    const hex = Buffer.from(JSON.stringify(blob), "utf-8").toString("hex")
    await run([
      "add-generic-password",
      "-U",
      "-a",
      getUsername(),
      "-s",
      serviceName(service),
      "-X",
      hex,
    ])
  }

  return {
    async write(service, account, secret) {
      const blob = await readBlob(service)
      blob[account] = secret
      await writeBlob(service, blob)
    },
    async read(service, account) {
      const blob = await readBlob(service)
      return blob[account] ?? null
    },
    async remove(service, account) {
      const blob = await readBlob(service)
      if (!(account in blob)) return
      delete blob[account]
      if (Object.keys(blob).length === 0) {
        await run([
          "delete-generic-password",
          "-a",
          getUsername(),
          "-s",
          serviceName(service),
        ])
        return
      }
      await writeBlob(service, blob)
    },
  }
}
