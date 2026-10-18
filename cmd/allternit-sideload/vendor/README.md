# Vendored crates

`isideload/` is isideload 0.4.6 (MIT, https://github.com/nab138/isideload) from
crates.io, used through `[patch.crates-io]` in `../Cargo.toml`. The only change
is `AnisetteData::new` in `src/anisette/mod.rs` (marked "Allternit patch"), so
`src/mac_anisette.rs` can give isideload headers made by this Mac's AuthKit.
Drop the copy once upstream exposes a constructor.
