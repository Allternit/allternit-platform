//! `allternit-brain-runner`: the first process (`lxc.init.cmd`) in a person's
//! runner box. See `allternit_api::brain_runner` for what it does and the
//! environment it reads.
//!
//! As PID 1 it forks once: the parent only reaps orphans and forwards
//! SIGTERM/SIGINT/SIGHUP, and the child runs the runner. That keeps
//! grandchildren a vendor CLI leaves behind from piling up as zombies without
//! stealing exit statuses from the runner's own children.

use std::process::ExitCode;

use allternit_api::brain_runner::{self, Config, RUNNER_VERSION};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
mod init {
    use std::sync::atomic::{AtomicI32, Ordering};

    static CHILD: AtomicI32 = AtomicI32::new(0);

    extern "C" fn forward(signal: libc::c_int) {
        let child = CHILD.load(Ordering::SeqCst);
        if child > 0 {
            // SAFETY: kill is async-signal-safe.
            unsafe {
                libc::kill(child, signal);
            }
        }
    }

    /// Returns in the runner process. The reaper parent never returns.
    pub fn split_if_pid1() {
        let forced = std::env::var("ALLTERNIT_RUNNER_INIT").is_ok_and(|v| v == "1");
        if std::process::id() != 1 && !forced {
            return;
        }
        // SAFETY: called before any thread exists (main, before the runtime).
        let child = unsafe { libc::fork() };
        if child <= 0 {
            // 0: the runner. <0: fork failed; run without a reaper.
            return;
        }
        CHILD.store(child, Ordering::SeqCst);
        // SAFETY: installing a handler that only calls kill.
        unsafe {
            for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
                libc::signal(signal, forward as extern "C" fn(libc::c_int) as libc::sighandler_t);
            }
        }
        loop {
            let mut status: libc::c_int = 0;
            // SAFETY: plain waitpid on any child.
            let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
            if pid == child {
                let code = if libc::WIFEXITED(status) {
                    libc::WEXITSTATUS(status)
                } else {
                    128 + libc::WTERMSIG(status)
                };
                std::process::exit(code);
            }
            if pid < 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                std::process::exit(1);
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-V" | "--version" | "version") => {
            println!("allternit-brain-runner {RUNNER_VERSION}");
            return ExitCode::SUCCESS;
        }
        Some("-h" | "--help" | "help") => {
            println!("allternit-brain-runner {RUNNER_VERSION}\n\nRuns in a person's runner box; configured by ALLTERNIT_RUNNER_* environment variables (see the brain_runner module docs).");
            return ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("allternit-brain-runner: unknown argument {other:?}");
            return ExitCode::from(2);
        }
        None => {}
    }

    #[cfg(unix)]
    init::split_if_pid1();

    let cfg = match Config::from_env() {
        Ok(cfg) => cfg,
        Err(error) => {
            eprintln!("allternit-brain-runner: {error}");
            return ExitCode::from(2);
        }
    };
    // The vendor CLIs never see the runner token. Still single-threaded here.
    std::env::remove_var("ALLTERNIT_RUNNER_TOKEN");

    tracing_subscriber::fmt().with_writer(std::io::stderr).with_target(false).init();

    let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("allternit-brain-runner: could not start: {error}");
            return ExitCode::from(1);
        }
    };
    runtime.block_on(async {
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                if let Ok(mut term) = signal(SignalKind::terminate()) {
                    tokio::select! {
                        _ = term.recv() => {}
                        _ = tokio::signal::ctrl_c() => {}
                    }
                } else {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
        });
        brain_runner::run(cfg, shutdown).await;
    });
    ExitCode::SUCCESS
}
