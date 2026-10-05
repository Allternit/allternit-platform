use serde::Serialize;

use crate::client::endpoint::{EndpointCatalog, ProfileId};

const HELP: &str = "Usage:
  ao machine list [--json]
  ao machine add <ssh-target> --label <label> [--remote-session <name>]
  ao machine connect <profile-id> [--keybindings local|server]
  ao machine rename <profile-id> --label <label>
  ao machine remove <profile-id>
  ao machine enable <profile-id>
  ao machine disable <profile-id>

Add prepares the remote ao installation and starts its server before saving.
Missing or incompatible installations require approval in an interactive terminal.
connect attaches through the saved SSH profile and its remote session.
Changes apply automatically to open local ao clients.
Removing or disabling a machine leaves its remote sessions running.
Saved machines contain only a label, SSH target, explicit ao session, and enabled state.
SSH credentials and key material remain owned by OpenSSH.";

#[derive(Serialize)]
struct MachineListRow<'a> {
    id: &'a str,
    label: &'a str,
    target: &'a str,
    session: &'a str,
    enabled: bool,
    selected: bool,
}

pub(super) fn run_machine_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("list") => list(&args[1..]),
        Some("add") => add(&args[1..]),
        Some("connect") => connect(&args[1..]),
        Some("rename") => rename(&args[1..]),
        Some("remove") => remove(&args[1..]),
        Some("enable") => set_enabled(&args[1..], true),
        Some("disable") => set_enabled(&args[1..], false),
        Some("help" | "--help" | "-h") => {
            println!("{HELP}");
            Ok(0)
        }
        _ => {
            eprintln!("{HELP}");
            Ok(2)
        }
    }
}

fn list(args: &[String]) -> std::io::Result<i32> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => {
            eprintln!("usage: ao machine list [--json]");
            return Ok(2);
        }
    };
    let catalog = load_catalog()?;
    let rows = catalog
        .ssh
        .iter()
        .map(|profile| MachineListRow {
            id: profile.id.as_str(),
            label: &profile.label,
            target: &profile.target,
            session: &profile.session,
            enabled: profile.enabled,
            selected: catalog.selected_profile.as_ref() == Some(&profile.id),
        })
        .collect::<Vec<_>>();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(std::io::Error::other)?
        );
        return Ok(0);
    }
    if rows.is_empty() {
        println!("No saved SSH machines.");
        return Ok(0);
    }
    for row in rows {
        let state = if row.enabled { "enabled" } else { "disabled" };
        println!(
            "{}\t{}\t{}\t{}\t{}",
            row.id, row.label, row.target, row.session, state
        );
    }
    Ok(0)
}

fn add(args: &[String]) -> std::io::Result<i32> {
    let args = super::expand_equals_args(args, &["--label", "--remote-session"]);
    let Some(target) = args.first().filter(|value| !value.starts_with('-')) else {
        eprintln!(
            "usage: ao machine add <ssh-target> --label <label> [--remote-session <name>]"
        );
        return Ok(2);
    };
    let mut label = None;
    let mut session = None;
    let mut index = 1;
    while index < args.len() {
        let (name, value) = match args[index].as_str() {
            "--label" | "--remote-session" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for {}", args[index]);
                    return Ok(2);
                };
                index += 2;
                (args[index - 2].as_str(), value.clone())
            }
            unknown => {
                eprintln!("unknown machine add option: {unknown}");
                return Ok(2);
            }
        };
        match name {
            "--label" if label.is_none() => label = Some(value),
            "--remote-session" if session.is_none() => session = Some(value),
            "--remote-session" => {
                eprintln!("--remote-session can only be specified once");
                return Ok(2);
            }
            "--label" => {
                eprintln!("--label can only be specified once");
                return Ok(2);
            }
            _ => unreachable!("validated machine add option"),
        }
    }
    let Some(label) = label else {
        eprintln!("--label is required");
        return Ok(2);
    };
    let session = session.unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_owned());
    let mut catalog = load_catalog()?;
    match catalog.add_ssh(label.clone(), target, session.clone()) {
        Ok(_) => {}
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    }
    if let Err(error) = crate::remote::prepare_saved_ssh(target, &session) {
        eprintln!("error: {error}; machine was not saved");
        crate::remote::print_remote_error_hint(&error, target);
        return Ok(1);
    }
    // Setup can wait for human approval. Do not overwrite catalog edits made meanwhile.
    let mut catalog = load_catalog().map_err(|error| {
        std::io::Error::other(format!(
            "remote prepared, but machine was not saved: {error}"
        ))
    })?;
    let id = match catalog.add_ssh(label, target, session) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    };
    store_catalog(&catalog).map_err(|error| {
        std::io::Error::other(format!(
            "remote prepared, but machine was not saved: {error}"
        ))
    })?;
    println!("Saved SSH machine {id}. Remote server is ready.");
    println!("Open ao clients connect automatically.");
    Ok(0)
}

fn connect(args: &[String]) -> std::io::Result<i32> {
    const USAGE: &str = "usage: ao machine connect <profile-id> [--keybindings local|server]";
    let args = super::expand_equals_args(args, &["--keybindings"]);
    let mut keybindings = crate::remote::RemoteKeybindings::Local;
    let mut positional = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--keybindings" {
            let Some(value) = args.get(index + 1) else {
                eprintln!("missing value for --keybindings");
                eprintln!("{USAGE}");
                return Ok(2);
            };
            keybindings = match value.as_str() {
                "local" => crate::remote::RemoteKeybindings::Local,
                "server" => crate::remote::RemoteKeybindings::Server,
                _ => {
                    eprintln!("--keybindings must be 'local' or 'server'");
                    eprintln!("{USAGE}");
                    return Ok(2);
                }
            };
            index += 2;
            continue;
        }
        positional.push(args[index].clone());
        index += 1;
    }
    let [raw_id] = positional.as_slice() else {
        eprintln!("{USAGE}");
        return Ok(2);
    };
    let id = match ProfileId::parse(raw_id.clone()) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("error: {error}");
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };
    let mut catalog = load_catalog()?;
    let Some(profile) = catalog.ssh.iter().find(|profile| profile.id == id) else {
        eprintln!("machine profile {id} was not found");
        return Ok(1);
    };
    let target = profile.target.clone();
    // Selection is a hint for future TUI opens; ignore storage errors.
    let _ = catalog.select_ssh(&id);
    let _ = catalog.store_selection();
    let remote = crate::remote::RemoteLaunch {
        target,
        keybindings,
        live_handoff: false,
    };
    crate::remote::run_remote(remote)?;
    Ok(0)
}

fn rename(args: &[String]) -> std::io::Result<i32> {
    let args = super::expand_equals_args(args, &["--label"]);
    let [raw_id, flag, label] = args.as_slice() else {
        eprintln!("usage: ao machine rename <profile-id> --label <label>");
        return Ok(2);
    };
    if flag != "--label" {
        eprintln!("usage: ao machine rename <profile-id> --label <label>");
        return Ok(2);
    }
    let id = match ProfileId::parse(raw_id.clone()) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    };
    let mut catalog = load_catalog()?;
    match catalog.rename_ssh(&id, label) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("machine profile {id} was not found");
            return Ok(1);
        }
        Err(error) => {
            eprintln!("error: {error}");
            return Ok(2);
        }
    }
    store_catalog(&catalog)?;
    println!("Renamed SSH machine {id}.");
    Ok(0)
}

fn remove(args: &[String]) -> std::io::Result<i32> {
    let Some(id) = one_profile_id(args, "usage: ao machine remove <profile-id>")? else {
        return Ok(2);
    };
    let mut catalog = load_catalog()?;
    let previous_selection = catalog.selected_profile.clone();
    if !catalog.remove_ssh(&id) {
        eprintln!("machine profile {id} was not found");
        return Ok(1);
    }
    store_catalog(&catalog)?;
    if catalog.selected_profile != previous_selection {
        catalog.store_selection().map_err(std::io::Error::other)?;
    }
    println!("Removed SSH machine {id}.");
    Ok(0)
}

fn set_enabled(args: &[String], enabled: bool) -> std::io::Result<i32> {
    let action = if enabled { "enable" } else { "disable" };
    let usage = format!("usage: ao machine {action} <profile-id>");
    let Some(id) = one_profile_id(args, &usage)? else {
        return Ok(2);
    };
    let mut catalog = load_catalog()?;
    let previous_selection = catalog.selected_profile.clone();
    if !catalog.set_enabled(&id, enabled) {
        eprintln!("machine profile {id} was not found");
        return Ok(1);
    }
    store_catalog(&catalog)?;
    if catalog.selected_profile != previous_selection {
        catalog.store_selection().map_err(std::io::Error::other)?;
    }
    println!(
        "{} SSH machine {id}.",
        if enabled { "Enabled" } else { "Disabled" }
    );
    Ok(0)
}

fn one_profile_id(args: &[String], usage: &str) -> std::io::Result<Option<ProfileId>> {
    let [raw] = args else {
        eprintln!("{usage}");
        return Ok(None);
    };
    match ProfileId::parse(raw.clone()) {
        Ok(id) => Ok(Some(id)),
        Err(error) => {
            eprintln!("error: {error}");
            Ok(None)
        }
    }
}

fn load_catalog() -> std::io::Result<EndpointCatalog> {
    EndpointCatalog::load().map_err(std::io::Error::other)
}

fn store_catalog(catalog: &EndpointCatalog) -> std::io::Result<()> {
    catalog.store_profiles().map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_id_parser_rejects_target_text() {
        assert!(one_profile_id(&["build.example".into()], "usage")
            .unwrap()
            .is_none());
    }

    #[test]
    fn list_rows_do_not_have_credential_fields() {
        let encoded = serde_json::to_string(&MachineListRow {
            id: "0123456789abcdef0123456789abcdef",
            label: "Build",
            target: "dev@build",
            session: "agents",
            enabled: true,
            selected: false,
        })
        .unwrap();
        assert!(!encoded.contains("password"));
        assert!(!encoded.contains("key"));
    }
}
