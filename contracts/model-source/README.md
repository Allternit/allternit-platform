# Model source contract (v1)

Where Allternit apps get model bytes from, and how sure they are that the bytes are right.
Every surface that downloads or reports a model (local-engine, allternit-api, allternit-ai
Desktop/web/PWA, Gizzi Code, iOS) builds against this folder.

- **Rust implementation:** `services/model-source` (crate `allternit-model-source`).
- **Response shape:** `model-source.schema.json` — the `verification` object.
- **TS types:** `types.ts` (allternit-ai and gizzi-code copy or import these).
- **Fixtures:** `fixtures.json` — valid and invalid `verification` objects plus the source order.
  The Rust tests deserialize every fixture; other surfaces should check theirs the same way.

## The `verification` object

```json
{
  "verification": {
    "state": "freeweights_signed",
    "source_used": "freeweights_worker",
    "source_url": "https://freeweights.allternit.com/Qwen/Qwen3-Embedding-0.6B/resolve/97b0c614…/",
    "manifest_commit": "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3",
    "checked_at": "2026-11-14T21:36:41Z"
  }
}
```

## States

`state` is never blank. It says what the file's sha256 was checked against.

| State | Checked against | When |
| --- | --- | --- |
| `freeweights_signed` | sha256 + size in the Freeweights manifest, whose minisign signature verified under key `2DC28CC7B26A283B` | catalog reachable, repo listed |
| `publisher_hash` | the publisher's hash from the Hugging Face tree API (`/api/models/{repo}/tree/{rev}?recursive=1`, or `/api/datasets/…`): sha256 `lfs.oid` for LFS files, the git blob id `oid` for non-LFS files | catalog unreachable, or repo not listed |
| `pinned` | sha256 the caller supplied | no catalog or publisher hash for the file, caller passed one |
| `unverified` | nothing | no hash anywhere and the caller explicitly allowed it |

Data with no hash is an **error** unless the caller sets `allow_unverified`.

Non-LFS files at the publisher (small JSON/config/tokenizer files) have no sha256 in the tree API.
They are checked against the tree entry's `oid`, the git blob SHA-1:
`sha1("blob {size}\0" + content)`, computed while streaming over the downloaded bytes. A match is
`publisher_hash`; a mismatch is handled exactly like a sha256 mismatch (delete, next source). So with
the catalog down, every file the tree lists verifies without a pin.

## Source order

`resolve(repo, rev)` returns sources in this order. A download tries them in turn; any transport
failure, size mismatch or hash mismatch moves to the next one, and a mismatch deletes the partial file.

1. **Freeweights worker** — `{base}/{org}/{repo}/resolve/{commit}/{path}`. It 302s to the best
   source; HEAD returns 200.
2. **Manifest `sources[]`** — by `priority` (lower first): `hosted` entries, `source` entries
   (upstream at the pinned commit). `seeded` (magnet-only) entries are not HTTP sources and are skipped.
3. **Publisher fallback, only when the catalog is unreachable** — `https://huggingface.co/{repo}/resolve/{rev}/{path}`
   (`rev` defaults to `main`). If the models tree API does not know the repo, the datasets one is tried
   and the URL becomes `https://huggingface.co/datasets/{repo}/resolve/{rev}/{path}`. Checked against
   `lfs.oid` (LFS) or the git blob `oid` (non-LFS).

`{base}` is `https://freeweights.allternit.com`, overridden by env `ALLTERNIT_FREEWEIGHTS_URL`.
The manifest is `{base}/manifests/{org}/{repo}.json` plus `.json.minisig`; the catalog is
`{base}/api/catalog`. Catalog, manifest and tree requests time out after 5 s; downloads use a
5 s connect and per-read timeout.

## The fallback rule

- Catalog **unreachable** (network error, timeout, 5xx) or the repo **not listed** (404):
  fall back to the publisher. State `publisher_hash`. `fallback_reason` says why.
- A `rev` other than `main` or the manifest's commit: the manifest does not cover it; publisher at that rev.
- Manifest **reachable but bad** (signature fails, wrong key id, missing `.minisig`, bytes not
  canonical, `id` not the requested repo): **error**. Never fall back silently — a bad
  manifest is a signal, not an outage.

Signatures are minisign `ED` (prehashed BLAKE2b-512, Ed25519). Legacy non-prehashed signatures are
rejected. The public key is embedded from `freeweights/spec/keys/freeweights-v0.pub`.

## Downloads

`fetch_verified` writes `{dest}.part`, resumes it with a `Range` request when it exists, hashes while
streaming, and renames to `{dest}` only after size and sha256 match. A resumed file that mismatches
is retried once from byte 0 on the same source before moving on.

## Live check

```sh
cargo run -p allternit-model-source --example live_check                     # smallest catalog model, smallest file
cargo run -p allternit-model-source --example live_check -- Qwen/Qwen3-Embedding-0.6B config.json
ALLTERNIT_FREEWEIGHTS_URL=http://127.0.0.1:1 cargo run -p allternit-model-source --example live_check -- Qwen/Qwen3-Embedding-0.6B tokenizer.json   # publisher fallback
```
