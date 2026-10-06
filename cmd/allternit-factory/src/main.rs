//! `allternit-factory`: the Allternit Factory engine binary.
//!
//! Internal. Users run `gizzi agents|orchestration|workflows|workspace …`, and
//! Gizzi runs this binary with `--json`. One process tree replaces the old
//! CommRails binaries and the agent-orchestrator engine:
//!
//! - `serve`          the engine's HTTP service (+ `serve uhp`, `serve fabric`)
//! - `pane …`         the pane engine (the Herdr fork); no args opens the TUI
//! - `agents | orchestration | workflows | workspace <verb>`   SPEC §7
//! - `internal …`     hidden maintenance commands (ledger, index, vault, …)
//!
//! Exit codes and the `--json` error shape follow API.md §2 (see `exec.rs`).

mod bots;
mod exec;
mod part;
mod tree;
mod work;

use std::process::ExitCode;

use clap::Parser;

use crate::exec::{exit_usage, Ctx};
use crate::tree::Cli;

fn main() -> ExitCode {
    // The pane engine re-executes itself (server daemon, client, handoff); in
    // this binary it lives under `pane`, so it must put that word back first.
    allternit_factory_pane::factory_host::set_argv_prefix(["pane"]);
    // Every engine spawn, send, capture and kill goes through the pane engine
    // (no tmux path): install it as the engine's pane backend.
    allternit_factory_pane::factory_backend::install();

    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let wants_json = argv.iter().any(|a| a == "--json");
    let cli = match Cli::try_parse_from(&argv) {
        Ok(cli) => cli,
        Err(err) => return ExitCode::from(exit_usage(err, wants_json)),
    };
    // Verbs that take trailing arguments (passthrough groups, not-built verbs)
    // keep `--json` out of clap's sight, so honor it wherever it appears.
    let ctx = Ctx::new(cli.root.clone(), cli.json || wants_json);
    ExitCode::from(tree::dispatch(&ctx, cli.command))
}
