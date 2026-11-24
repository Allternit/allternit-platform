# Files drive helper for Windows (`allternit-files-win`)

Plan step 24 of the Files drive (`contracts/files-drive/SPEC.md`). Productized
from the spike on `spike/files-win-cfapi` (CI run 43738022182).

One Cloud Files (cfapi) sync root per Allternit computer at
`%USERPROFILE%\Allternit Files\<computer name>`. Allternit Desktop starts one
`serve` process per computer and passes a config file (see `src/config.rs`);
the bearer token comes in `ALLTERNIT_FILES_TOKEN` and `{"type":"token"}` lines
on stdin. Events go out as JSON lines on stdout.

| cfapi event | What the helper does | files/2 call |
|---|---|---|
| FETCH_PLACEHOLDERS (folder opened) | lists the folder once, then the change feed keeps it current | `GET /roots`, `GET /roots/:id/list` (snapshot when asleep, never wakes) |
| FETCH_DATA (file opened) | reads 4096-aligned ranges of at most 4 MiB | `GET /items/:id/content` with `Range` |
| CANCEL_FETCH_DATA | stops the transfer | |
| NOTIFY_DELETE / NOTIFY_RENAME | trash / move on the server, or veto | `POST /delete`, `POST /move` with `base_rev` |
| NOTIFY_DEHYDRATE, "Free up space" | allows it, unpins | `POST /pin` |
| local edit or new file (watcher) | upload session, keep-both on a stale rev, mirror the conflict copy | `POST /uploads` ... `/commit` |
| change feed | create, update (dehydrate on new content), rename, delete placeholders | `GET /roots/:id/changes` |

Content of a sleeping computer: `wake_consent_required` -> ask (message box),
`auto` or `never` (`src/wake.rs`). Browsing never wakes; background readers
(indexer, Defender, preview hosts) never wake.

## Registration and identity

`StorageProviderSyncRootManager.Register` is tried first: it gives the
navigation-pane entry, the location icon and the colour badges (design pick 6A).
Microsoft documents it for packaged apps. If it refuses an unpackaged process,
the helper falls back to `CfRegisterSyncRoot` (what the spike proved unpackaged)
plus a per-user shell-folder entry for the navigation pane; Explorer's own
status column still works, the custom badges do not. MSIX packaging (package
identity) would make the first path certain, and it needs the code-signing
certificate: gate G-W, Eoj's. The exe ships unsigned until then.

## Test

Windows only (CI: `.github/workflows/files-win-helper.yml`, and the
`build-windows` job of `release-desktop.yml` before packaging):

```
cargo test --manifest-path surfaces/allternit-desktop/native/files-win/Cargo.toml -- --nocapture --test-threads=1
```

`tests/sync_root.rs` registers a real sync root against an in-process files/2
runtime (`tests/support`) and checks listing, hydration ranges, free up space,
write-back, a keep-both conflict, the change feed, ask-before-wake and
unregister. Unit tests in `src/` cover range alignment, names, the index and
conflict-name parsing and run on any OS.

Icons: `icons/make_icons.py` writes the badge `.ico` files; `allternit.ico` is
the app tile (design pick 5A), copied from `build/icon.ico`.
