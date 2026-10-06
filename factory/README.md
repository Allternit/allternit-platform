# Allternit Factory

The engine behind Gizzi and Allternit Desktop (SPEC: Allternit Factory). One binary,
`allternit-factory` (`cmd/allternit-factory`), built from two crates here:

| Crate | Path | What it is |
|---|---|---|
| `allternit-factory-engine` | `factory/engine` | Ledger, Gate, the four parts (`agents`, `orchestration`, `workflows`, `workspace`), the HTTP service, the remote bridge. Formerly CommRails. |
| `allternit-factory-pane` | `factory/pane` | The pane engine: live agent terminals, the TUI wall, the socket API, UHP and fabric surfaces. A fork of Herdr v0.9.0 (Apache-2.0, see `pane/LICENSE` and `pane/NOTICE`), formerly the `ao` engine. Its internal layout stays Herdr's so upstream fixes still merge. |

`allternit-factory` is internal; people run `gizzi agents|orchestration|workflows|workspace …`.
The command tree, exit codes and `--json` contract are in `allternit-factory --help`.
The pane engine's own commands are under `allternit-factory pane …`; the engine maintenance
commands (ledger, index, lease, vault, replay, hook, …) are under the hidden
`allternit-factory internal rails …`.

`factory/pane` also builds `allternit-factory-pane`, a dev/test harness with the same entry
point as `allternit-factory pane`. It is never shipped.
