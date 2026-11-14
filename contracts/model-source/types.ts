// Model source contract (v1). Hand-kept in sync with model-source.schema.json
// and the Rust types in services/model-source (`Verification`, `VerificationState`,
// `SourceKind`). fixtures.json is the shared check.

/** How downloaded model bytes were checked. Never blank. */
export type VerificationState =
  | 'freeweights_signed' // sha256 from a minisign-verified Freeweights manifest
  | 'publisher_hash' // sha256 from the publisher (Hugging Face lfs.oid); catalog not used
  | 'pinned' // sha256 the caller supplied
  | 'unverified' // no hash; the caller explicitly allowed it

/** Where the bytes came from, in resolve order. */
export type SourceKind =
  | 'freeweights_worker' // https://freeweights.allternit.com/{org}/{repo}/resolve/{commit}/{path}
  | 'manifest_hosted' // manifest sources[] type "hosted"
  | 'manifest_source' // manifest sources[] type "source" (upstream at the pinned commit)
  | 'publisher' // https://huggingface.co/{repo}/resolve/{rev}/{path}, catalog unreachable

/** The `verification` object on local-engine / allternit-api model responses. */
export interface ModelVerification {
  state: VerificationState
  source_used: SourceKind
  /** Base URL of the source, before redirects. File URL = source_url + encoded path. */
  source_url: string
  /** 40-hex upstream commit of the signed manifest; set when state is freeweights_signed. */
  manifest_commit?: string
  /** UTC, YYYY-MM-DDTHH:MM:SSZ. */
  checked_at: string
}

export const VERIFICATION_STATES: readonly VerificationState[] = [
  'freeweights_signed',
  'publisher_hash',
  'pinned',
  'unverified',
]

/** Short user-facing label for a state (Register 1: plain, no guarantee language). */
export const VERIFICATION_LABELS: Record<VerificationState, string> = {
  freeweights_signed: 'Checked against the signed Freeweights manifest',
  publisher_hash: "Checked against the publisher's hash",
  pinned: 'Checked against a pinned hash',
  unverified: 'Not checked (no hash available)',
}
