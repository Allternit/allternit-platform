// Types copied from contracts/model-source/types.ts (gizzi-code's tsconfig does
// not include files outside cmd/gizzi-code). Keep in sync with that file.
export type VerificationState = "freeweights_signed" | "publisher_hash" | "pinned" | "unverified"
export type SourceKind = "freeweights_worker" | "manifest_hosted" | "manifest_source" | "publisher"
export interface ModelVerification {
  state: VerificationState
  source_used: SourceKind
  source_url: string
  manifest_commit?: string
  checked_at: string
}

export interface VerificationLine {
  text: string
  /** "ok" prints a check mark; "warn" prints "!". */
  level: "ok" | "warn"
  /** True only when the engine reported state `unverified`. */
  unverified: boolean
}

/** One user-facing line per verification result. A missing field means an older engine. */
export function describeVerification(v: ModelVerification | undefined | null): VerificationLine {
  if (!v || typeof v.state !== "string") {
    return { text: "Not checked by this engine version", level: "warn", unverified: false }
  }
  switch (v.state) {
    case "freeweights_signed": {
      const c = v.manifest_commit ? ` (commit ${v.manifest_commit.slice(0, 7)})` : ""
      return { text: `✓ Verified by Freeweights${c}`, level: "ok", unverified: false }
    }
    case "publisher_hash":
      return { text: "✓ Matches the publisher's hash", level: "ok", unverified: false }
    case "pinned":
      return { text: "✓ Matches the pinned hash", level: "ok", unverified: false }
    default:
      return {
        text: "! Not verified: no hash available from Freeweights or the publisher",
        level: "warn",
        unverified: true,
      }
  }
}

/**
 * Exit-code rule: non-zero when the engine reported `unverified` and the caller did not pass --allow-unverified.
 */
export function pullExitCode(v: ModelVerification | undefined | null, allowUnverified: boolean): number {
  const line = describeVerification(v)
  return line.unverified && !allowUnverified ? 2 : 0
}
