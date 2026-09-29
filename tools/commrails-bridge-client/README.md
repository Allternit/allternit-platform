# commrails-bridge (box-side client)

A single-file Python 3.8+ stdlib client that lets a remote agent (Chief, on
the shared box) use the CommRails bridge on Eoj's Mac. Spec and threat model:
[`commrails/spec/BRIDGE.md`](../../commrails/spec/BRIDGE.md).

It can create and read plans and send/read coordination mail. It cannot pick
up, close, lease, or resolve anything: the bridge answers those routes 403
regardless of what the token claims.

## Install on the box

```bash
install -m 0755 commrails-bridge ~/.local/bin/commrails-bridge
mkdir -p ~/.config/commrails-bridge && chmod 700 ~/.config/commrails-bridge
# paste the token printed once by `allternit-commrails identity add` on the Mac:
( umask 077; cat > ~/.config/commrails-bridge/token )
export COMMRAILS_BRIDGE_URL=http://100.x.y.z:7433   # the Mac's mesh address
```

The client refuses a token file readable by group/other, and refuses plain
http to anything except loopback and Tailscale mesh addresses
(`100.64.0.0/10`, `fd7a:115c:a1e0::/48`, `*.ts.net`).

## Commands

```bash
commrails-bridge whoami
commrails-bridge plan-new "Draft the P0-2 brief" --decision-ref chief-dec-42
commrails-bridge templates
commrails-bridge plan-from-template <template_id> --param topic=Jev [--text "..."]
commrails-bridge plan-show <dag_id> [--json]
commrails-bridge mail-send dag:<dag_id> --body "status: drafted" --subject update
echo "long body" | commrails-bridge mail-send mail:chief
commrails-bridge mail-read --thread dag:<dag_id> [--limit 20] [--json]
commrails-bridge mirror <dag_id> [/workspace/runs]
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

## Enabling

The bridge is off by default. Turning it on (Mac listener on the mesh address,
Tailscale ACL, launchd plist) is Eoj's decision. The steps are in
[`BRIDGE.md` → Enabling](../../commrails/spec/BRIDGE.md#enabling-eojs-call--not-done-by-the-build)
and have not been applied.

## Testing

`cargo test -p allternit-commrails --test bridge` runs this script against a
loopback bridge (`client_mirror_writes_a_read_only_snapshot`): plan-new,
plan-show, mail-send, mail-read, mirror (twice), and the post-revoke 401.
