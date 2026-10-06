# Contributing

Gizzi Code lives at `cmd/gizzi-code` in the
[Allternit/allternit-platform](https://github.com/Allternit/allternit-platform)
monorepo (pnpm workspace).

## Setup

```bash
# from the repo root
pnpm install
cd cmd/gizzi-code
node packages/sdk/scripts/build.mjs   # build the SDK dist (gitignored, required for typecheck)
```

## Development loop

```bash
bun run dev                    # run from source
bun run typecheck              # tsc --noEmit (must be exit 0)
bash script/ci-smoke-test.sh   # empirically-green test subset (must be 0 fail)
bun run build                  # production binary -> dist/
bash script/check-command-exits.sh  # one-shot commands must exit (see #exithang)
```

Every change must pass typecheck and the smoke suite before merging. Release
workflows re-run both gates on the tagged commit.

## Conventions

- Bun runtime; ESM; TypeScript strict. Match the style of the file you touch.
- Cloud URLs: never hardcode hosts — use `src/shared/constants/cloudUrls.ts`
  (canonical hosts) and `src/shared/constants/allternitGateway.ts` (gateway).
- Tokens: classify with `src/shared/utils/allternitToken.ts` before persisting
  anything; durable `alt_` keys may be stored (0600), Clerk JWTs must not be.
- One-shot commands must terminate: the central exit guarantee lives in the
  CLI entry (`src/cli/main.ts`) — long-lived commands are on the denylist there.

## Reporting issues / security

- Bugs: https://github.com/Allternit/allternit-platform/issues
- Security: see [SECURITY.md](./SECURITY.md) (GitHub Security Advisories;
  security@allternit.com for escalation).
