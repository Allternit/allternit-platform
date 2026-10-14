# Happy native session adapter

Connects the user's own Happy account (their Claude Code / Codex sessions, as run through `happy-cli`) to Allternit.

Pinned upstream: `slopus/happy-cli` @ `c2bacb37369264dabc24afa697985039b762a5d6` (`src/api/api.ts`, `src/api/apiSession.ts`, `src/api/encryption.ts`, `src/utils/deriveKey.ts`, `src/ui/auth.ts`), and `slopus/happy-server` @ `922a62fab5a6563ec0b636f3b26520e6bbec6af4` (`sources/app/api/routes/authRoutes.ts`, `sessionRoutes.ts`, `sources/app/api/socket/sessionUpdateHandler.ts`, `sources/app/api/socket.ts`).

## Credential

Per account, JSON:

```json
{ "secret": "<your Happy account secret, the 32-byte restore secret>", "serverUrl": "https://api.cluster-fluster.com" }
```

- `secret` is the same value the Happy app shows for restoring an account (base64url or base64 of 32 bytes; the `handy://` link carries it base64url-encoded). It stays in memory for one call and is not persisted.
- `serverUrl` is optional. Default `https://api.cluster-fluster.com` (happy-cli's default). Must be an https origin, or loopback http.
- `SUBS_GATEWAY_HAPPY_URL` overrides the default server for the deployment.

## Route

- Sign-in: `POST /v1/auth` with an ed25519 signature over a random 32-byte challenge, signed with the key pair seeded by the secret.
- Discovery: `GET /v2/sessions/active`. Each active session is an agent, `happy:<sessionId>`. Names come from the decrypted session metadata.
- Send: the user message `{role:"user", content:{type:"text", text}, localKey, meta}` is encrypted with the session key and emitted as a Socket.IO `message` event on `/v1/updates` (`{sid, message, localId}`). The adapter speaks the minimal engine.io/Socket.IO v4 framing itself, so no socket client dependency is added.
- Reply: `GET /v1/sessions/:id/messages` is polled until this send's `localId` appears; agent messages after it are decrypted and joined. A turn ends on a Claude `result` or ACP/Codex `task_complete`, or after the reply goes quiet for `settleMs`.

## Encryption

- Legacy accounts: the account secret is the session key (tweetnacl secretbox).
- Data-key accounts: each session's `dataEncryptionKey` is `0x00 | ephemeralPk(32) | nonce(24) | box` sealed to the account content key. The content key is `deriveKey(secret, "Happy EnCoder", ["content"])`, boxed with `sha512(seed)[0:32]`, which matches the commented-out derivation in happy-cli `api.ts`. Session payloads are AES-256-GCM with version byte `0`.

## Not verified live

Nothing here has been run against a live Happy account. The data-key content-key derivation is taken from the commented code in happy-cli, not from an exercised path. Check the first live send before relying on it.

## Unsupported

No streaming, approvals, cancel, or tool visibility. Contexts do not survive gateway restarts. A send with no agent reply inside the timeout returns VENDOR_UNAVAILABLE and must be checked in the session before retrying.
