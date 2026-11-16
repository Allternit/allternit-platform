//! `allternit-files-win`: the Windows Files drive helper.
//!
//! ```text
//! allternit-files-win serve --config <computer.json>   run one computer's sync root
//! allternit-files-win register --config <computer.json>
//! allternit-files-win unregister --config <computer.json>
//! allternit-files-win unregister-all                   uninstall (NSIS customUnInstall)
//! allternit-files-win free-up <path>                   "Free up space" for one file
//! allternit-files-win version
//! ```
//!
//! `serve --token-stdin` reads the bearer token from the first stdin line (Allternit
//! Desktop; without the flag, from `ALLTERNIT_FILES_TOKEN`), then JSON lines on
//! stdin: `{"type":"token","token":"..."}` (refresh) and `{"type":"stop"}`. It
//! writes JSON event lines on stdout.

#[cfg(not(windows))]
fn main() {
    eprintln!("allternit-files-win runs on Windows only");
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    std::process::exit(win::run(std::env::args().skip(1).collect()));
}

#[cfg(windows)]
mod win {
    use std::io::BufRead;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use allternit_files_win::client::{FilesClient, Transport};
    use allternit_files_win::config::HelperConfig;
    use allternit_files_win::provider::{Options, Provider};
    use allternit_files_win::sync_root::{self, Registration, Spec};
    use allternit_files_win::wake::{MessageBoxPrompt, WakeGate};
    use allternit_files_win::watcher::LocalWatcher;
    use allternit_files_win::{emit, paths, HELPER_VERSION};
    use serde_json::json;

    fn exe_dir() -> PathBuf {
        std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default()
    }

    fn default_state_dir() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        base.join("Allternit").join("files-win")
    }

    fn arg(args: &[String], name: &str) -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
    }

    fn load(args: &[String]) -> Result<HelperConfig, String> {
        let p = arg(args, "--config").ok_or("--config <file> is required")?;
        let b = std::fs::read(&p).map_err(|e| format!("{p}: {e}"))?;
        serde_json::from_slice(&b).map_err(|e| format!("{p}: {e}"))
    }

    struct Resolved {
        cfg: HelperConfig,
        root: PathBuf,
        state_dir: PathBuf,
        icons_dir: PathBuf,
    }

    fn resolve(cfg: HelperConfig) -> Resolved {
        let profile = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
        let root = cfg.sync_root.clone().unwrap_or_else(|| paths::default_sync_root(&profile, &cfg.computer_name));
        let state_dir = cfg.state_dir.clone().unwrap_or_else(default_state_dir);
        let icons_dir = cfg.icons_dir.clone().unwrap_or_else(|| exe_dir().join("icons"));
        Resolved { cfg, root, state_dir, icons_dir }
    }

    fn register(r: &Resolved) -> Result<Registration, String> {
        let icon = r.icons_dir.join("allternit.ico");
        let spec = Spec {
            path: &r.root,
            computer_id: &r.cfg.computer_id,
            display_name: &r.cfg.computer_name,
            icon: icon.exists().then_some(icon.as_path()),
        };
        sync_root::register(&spec, &r.state_dir).map_err(|e| e.to_string())
    }

    /// `--token-stdin`: Desktop sends `{"type":"token"}` as the first stdin line,
    /// so the token is never on a command line, in the environment or in a file.
    fn first_token_line() -> Option<String> {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).ok()?;
        let msg: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        (msg.get("type")?.as_str()? == "token").then(|| msg.get("token")?.as_str().map(str::to_string))?
    }

    fn serve(r: Resolved, token_stdin: bool) -> Result<(), String> {
        let registration = match sync_root::recorded(&r.state_dir).into_iter().find(|x| x.path == r.root) {
            Some(x) if sync_root::is_registered(&r.root) => x.registration,
            _ => register(&r)?,
        };
        emit(&json!({"type": "registered", "path": r.root.display().to_string(), "registration": registration}));
        let transport = Transport::from_config(&r.cfg.transport.kind, &r.cfg.transport.base_url).map_err(|e| e.to_string())?;
        let client = Arc::new(FilesClient::new(transport, &r.cfg.cloud_api_base, &r.cfg.computer_id, &r.cfg.device_id, &r.cfg.device_name));
        let token = if token_stdin { first_token_line() } else { std::env::var("ALLTERNIT_FILES_TOKEN").ok() };
        client.set_token(token);
        let wake = Arc::new(WakeGate::new(&r.cfg.computer_name, Box::new(MessageBoxPrompt)));
        let state_file = r.state_dir.join(format!("{}.index.json", paths::safe_component(&r.cfg.computer_id)));
        let provider = Provider::new(
            Options {
                root: r.root.clone(),
                state_file,
                icons_dir: r.icons_dir.clone(),
                badges: matches!(registration, Registration::StorageProvider { .. }),
            },
            Arc::clone(&client),
            wake,
        );
        let conn = provider.connect().map_err(|e| format!("CfConnectSyncRoot: {e}"))?;
        let feed = provider.run_feed();
        let watcher = LocalWatcher::start(Arc::clone(&provider)).map_err(|e| e.to_string())?;
        emit(&json!({"type": "ready", "version": HELPER_VERSION}));
        // stdin: token refreshes and stop. EOF (Desktop quit) also stops.
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            match msg.get("type").and_then(|t| t.as_str()) {
                Some("token") => client.set_token(msg.get("token").and_then(|t| t.as_str()).map(str::to_string)),
                Some("stop") => break,
                _ => {}
            }
        }
        provider.stop();
        watcher.join();
        let _ = feed.join();
        conn.disconnect().map_err(|e| e.to_string())?;
        emit(&json!({"type": "stopped"}));
        Ok(())
    }

    pub fn run(args: Vec<String>) -> i32 {
        let cmd = args.first().map(String::as_str).unwrap_or("");
        let result: Result<(), String> = match cmd {
            "serve" => {
                let token_stdin = args.iter().any(|a| a == "--token-stdin");
                load(&args).map(resolve).and_then(|r| serve(r, token_stdin))
            }
            "register" => load(&args).map(resolve).and_then(|r| {
                let reg = register(&r)?;
                emit(&json!({"type": "registered", "path": r.root.display().to_string(), "registration": reg}));
                Ok(())
            }),
            "unregister" => load(&args).map(resolve).and_then(|r| {
                sync_root::unregister(&r.root, &r.state_dir).map_err(|e| e.to_string())?;
                emit(&json!({"type": "unregistered", "path": r.root.display().to_string()}));
                Ok(())
            }),
            "unregister-all" => {
                let state_dir = arg(&args, "--state-dir").map(PathBuf::from).unwrap_or_else(default_state_dir);
                let done = sync_root::unregister_all(&state_dir);
                emit(&json!({"type": "unregistered_all", "roots": done}));
                Ok(())
            }
            "free-up" => match args.get(1) {
                Some(p) => allternit_files_win::cf::dehydrate(Path::new(p)).map_err(|e| e.to_string()),
                None => Err("free-up <path>".into()),
            },
            "version" => {
                println!("{HELPER_VERSION}");
                Ok(())
            }
            _ => Err("usage: allternit-files-win serve|register|unregister --config <file> | unregister-all | free-up <path> | version".into()),
        };
        match result {
            Ok(()) => 0,
            Err(e) => {
                emit(&json!({"type": "error", "message": e}));
                1
            }
        }
    }
}
