/**
 * Saved self-hosted Agents Server profiles (`gizzi runtime register
 * --self-hosted`).
 *
 * The profile (name, URL) lives in `<data>/selfhost-servers.json` (0600).
 * The server API key (`ask_...`) never goes in that file: it goes to the
 * credential store (macOS Keychain, Windows DPAPI, else the marked 0600
 * fallback file), under service `gizzi-selfhost`, account = profile name.
 * Nothing here logs the key.
 */
import path from "node:path"
import { GlobalPaths } from "@/runtime/context/global/paths"
import { Filesystem } from "@/shared/util/filesystem"
import { createCredentialWriter, type CredentialWriter } from "@/runtime/context/config/credential-store"

export const CREDENTIAL_SERVICE = "gizzi-selfhost"

export interface SelfHostProfile {
  name: string
  url: string
  createdAt: number
  /** First characters of the key, for display. */
  keyPrefix: string
}

interface Store {
  default?: string
  profiles: Record<string, SelfHostProfile>
}

export interface ProfileStoreOptions {
  file?: string
  writer?: CredentialWriter
}

export function profilesFile(): string {
  return path.join(GlobalPaths.data, "selfhost-servers.json")
}

export function keyPrefix(key: string): string {
  return key.length > 12 ? `${key.slice(0, 8)}…` : "ask_…"
}

export function createProfileStore(opts: ProfileStoreOptions = {}) {
  const file = opts.file ?? profilesFile()
  const writer = () => opts.writer ?? createCredentialWriter("auto")

  async function read(): Promise<Store> {
    const data = await Filesystem.readJson<Store>(file).catch(() => undefined)
    return { default: data?.default, profiles: data?.profiles ?? {} }
  }

  return {
    file,
    async list(): Promise<{ default?: string; profiles: SelfHostProfile[] }> {
      const store = await read()
      return { default: store.default, profiles: Object.values(store.profiles) }
    },

    /** By name or URL; the default profile when `ref` is empty. */
    async get(ref?: string): Promise<SelfHostProfile | undefined> {
      const store = await read()
      if (!ref) return store.default ? store.profiles[store.default] : Object.values(store.profiles)[0]
      const trimmed = ref.replace(/\/+$/, "")
      return store.profiles[ref] ?? Object.values(store.profiles).find((p) => p.url === trimmed)
    },

    async save(input: { name: string; url: string; apiKey: string; makeDefault?: boolean }): Promise<SelfHostProfile> {
      await writer().write(CREDENTIAL_SERVICE, input.name, input.apiKey)
      const store = await read()
      const profile: SelfHostProfile = {
        name: input.name,
        url: input.url.replace(/\/+$/, ""),
        createdAt: store.profiles[input.name]?.createdAt ?? Date.now(),
        keyPrefix: keyPrefix(input.apiKey),
      }
      store.profiles[input.name] = profile
      if (input.makeDefault !== false || !store.default) store.default = input.name
      await Filesystem.writeJson(file, store, 0o600)
      return profile
    },

    async apiKey(profile: SelfHostProfile): Promise<string | null> {
      return writer().read(CREDENTIAL_SERVICE, profile.name)
    },

    async remove(name: string): Promise<boolean> {
      const store = await read()
      if (!store.profiles[name]) return false
      delete store.profiles[name]
      if (store.default === name) store.default = Object.keys(store.profiles)[0]
      await Filesystem.writeJson(file, store, 0o600)
      await writer().remove(CREDENTIAL_SERVICE, name).catch(() => {})
      return true
    },
  }
}
