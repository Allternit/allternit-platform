# factory-bridge (box-side client)

A single-file Python 3.8+ stdlib client that lets a remote agent (Chief, on
the shared box) use the Allternit Factory bridge on Eoj's Mac. Spec and threat
model: [`factory/engine/spec/BRIDGE.md`](../../factory/engine/spec/BRIDGE.md).

It can create and read plans and send/read coordination mail. It cannot pick
up, close, lease, or resolve anything: the bridge answers those routes 403
regardless of what the token claims.

## Install on the box

```bash
install -m 0755 factory-bridge ~/.local/bin/factory-bridge
mkdir -p ~/.config/allternit-factory-bridge && chmod 700 ~/.config/allternit-factory-bridge
# paste the token printed once by `allternit-factory internal core identity add` on the Mac:
( umask 077; cat > ~/.config/allternit-factory-bridge/token )
export ALLTERNIT_FACTORY_BRIDGE_URL=http://100.x.y.z:7433   # the Mac's mesh address
```

Config (flags override env): `--url` / `ALLTERNIT_FACTORY_BRIDGE_URL`,
`--token-file` / `ALLTERNIT_FACTORY_BRIDGE_TOKEN_FILE` (default
`~/.config/allternit-factory-bridge/token`), or the token itself in
`ALLTERNIT_FACTORY_BRIDGE_TOKEN` (the file is preferred). A box set up before the Factory
rename still works: the client falls back to the old env names and token path
and prints a one-time deprecation on stderr. Move to the new names when you
next touch the box (see the [migration page](../../surfaces/docs/factory/migration.mdx)).

The client refuses a token file readable by group/other, and refuses plain
http to anything except loopback and Tailscale mesh addresses
(`100.64.0.0/10`, `fd7a:115c:a1e0::/48`, `*.ts.net`).

On the Mac, the bridge reads its identities from
`$ALLTERNIT_FACTORY_BRIDGE_IDENTITIES` (default
`~/.allternit/factory/bridge/identities.json`).

## Commands

```bash
factory-bridge whoami
factory-bridge plan-new "Draft the P0-2 brief" --decision-ref chief-dec-42
factory-bridge templates
factory-bridge plan-from-template <template_id> --param topic=Jev [--text "..."]
factory-bridge plan-show <dag_id> [--json]
factory-bridge mail-send dag:<dag_id> --body "status: drafted" --subject update
echo "long body" | factory-bridge mail-send mail:chief
factory-bridge mail-read --thread dag:<dag_id> [--limit 20] [--json]
factory-bridge mirror <dag_id> [/workspace/runs]
```

Exit codes: 0 ok, 1 local error, 2 bridge refused (4xx/5xx), 3 auth or
permission refused (401/403).

## Mirror layout

`mirror <dag_id>` writes a read-only snapshot to `/workspace/runs/<dag_id>/`:

```
plan.json            {"mirror": true, "authoritative": false, taken_at, source, actor, dag}
nodes/<node>.out.md  recorded node outputs (sha256-checked against the ledger receipt)
README.md            "MIRROR — NOT TRUTH"
```

Files are 0444 and directories 0555. Re-running `mirror` replaces the snapshot.
Edits in the mirror are never synced back; change the plan through the bridge.
To delete a mirror by hand: `chmod -R u+w /workspace/runs/<dag_id> && rm -rf /workspace/runs/<dag_id>`.

## Enabling

The bridge is off by default. Turning it on (`allternit-factory internal core
bridge serve` on the mesh address, Tailscale ACL, launchd plist) is Eoj's
decision. The steps are in
[`BRIDGE.md` → Enabling](../../factory/engine/spec/BRIDGE.md#enabling-eojs-call--not-done-by-the-build)
and have not been applied.

## Testing

`cargo test -p allternit-factory --test bridge` runs this script against a
loopback bridge (`client_mirror_writes_a_read_only_snapshot`): plan-new,
plan-show, mail-send, mail-read, mirror (twice), and the post-revoke 401.
