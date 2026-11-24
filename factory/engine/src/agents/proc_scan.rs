//! Read-only process facts for live sessions: the process table, a process's
//! exact argv, its working folder, the files it holds open, and which app it
//! runs under (Terminal, iTerm2, Ghostty, an Allternit pane, or other).
//!
//! Nothing here signals, writes to or types into another process. External
//! commands (`ps`, `lsof`, `sqlite3`) run with a timeout; the only process
//! this module ever stops is its own timed-out child.

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// One row of the process table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcRow {
    pub pid: u32,
    pub ppid: u32,
    /// `ttys003`; `None` when the process has no terminal (daemons).
    pub tty: Option<String>,
    /// Start time as `ps -o lstart` prints it, spaces collapsed (`Sat Nov 21 03:55:44 2026`).
    pub started: String,
    /// The executable (full path on macOS; may contain spaces).
    pub comm: String,
}

/// Run a command and return its stdout, or `None` if it fails to start or
/// outlives `timeout` (then the child is stopped and reaped).
pub fn run_with_timeout(prog: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(prog).args(args).env("LC_ALL", "C").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Drain stdout on its own thread so a full pipe never blocks the child.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
        }
    }
    reader.join().ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Parse `ps -A -o pid=,ppid=,tty=,lstart=,comm=` output. `lstart` is always five words.
pub fn parse_ps(text: &str) -> Vec<ProcRow> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pid = words.next()?.parse().ok()?;
            let ppid = words.next()?.parse().ok()?;
            let tty = words.next()?;
            let started: Vec<&str> = words.by_ref().take(5).collect();
            if started.len() != 5 {
                return None;
            }
            let comm = words.collect::<Vec<_>>().join(" ");
            if comm.is_empty() {
                return None;
            }
            let tty = (!matches!(tty, "??" | "?" | "-")).then(|| tty.to_string());
            Some(ProcRow { pid, ppid, tty, started: started.join(" "), comm })
        })
        .collect()
}

/// The process table, cached for 2 s (sessions are polled, and one listing reads it several times).
pub fn snapshot() -> Vec<ProcRow> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Vec<ProcRow>)>>> = OnceLock::new();
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, rows)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(2) {
            return rows.clone();
        }
    }
    let rows = read_table();
    *cache = Some((Instant::now(), rows.clone()));
    rows
}

#[cfg(unix)]
fn read_table() -> Vec<ProcRow> {
    run_with_timeout("ps", &["-A", "-o", "pid=,ppid=,tty=,lstart=,comm="], Duration::from_secs(3)).map(|t| parse_ps(&t)).unwrap_or_default()
}

#[cfg(not(unix))]
fn read_table() -> Vec<ProcRow> {
    Vec::new()
}

/// Rows by pid.
pub fn by_pid(rows: &[ProcRow]) -> HashMap<u32, &ProcRow> {
    rows.iter().map(|r| (r.pid, r)).collect()
}

fn base(comm: &str) -> &str {
    comm.rsplit('/').next().unwrap_or(comm)
}

/// The app a process runs under, from the nearest known ancestor:
/// `Allternit` (the pane engine `allternit-factory`, `allternit-node`, or Allternit Desktop),
/// `Terminal`, `iTerm2`, `Ghostty`, else `other`. `None` when the process is not in the table.
pub fn app_for(pid: u32, rows: &HashMap<u32, &ProcRow>) -> Option<&'static str> {
    let mut cur = rows.get(&pid)?.ppid;
    for _ in 0..64 {
        let Some(row) = rows.get(&cur) else { break };
        let name = base(&row.comm);
        let app = if name == "allternit-factory" || name == "allternit-node" || name.starts_with("Allternit") {
            Some("Allternit")
        } else if name == "Terminal" {
            Some("Terminal")
        } else if name == "iTerm2" || name.starts_with("iTermServer") {
            Some("iTerm2")
        } else if name.eq_ignore_ascii_case("ghostty") {
            Some("Ghostty")
        } else {
            None
        };
        if app.is_some() {
            return app;
        }
        if row.ppid <= 1 || row.ppid == cur {
            break;
        }
        cur = row.ppid;
    }
    Some("other")
}

/// A process's exact argv (`KERN_PROCARGS2` on macOS, `/proc/<pid>/cmdline` on Linux).
pub fn argv(pid: u32) -> Option<Vec<String>> {
    argv_os(pid)
}

#[cfg(target_os = "macos")]
fn argv_os(pid: u32) -> Option<Vec<String>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    // SAFETY: reads one c_int into `argmax`; `size` is its size.
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), mib.len() as libc::c_uint, (&mut argmax as *mut libc::c_int).cast(), &mut size, std::ptr::null_mut(), 0) };
    if r != 0 || argmax <= 0 {
        return None;
    }
    let mut buf = vec![0u8; argmax as usize];
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let mut size = buf.len();
    // SAFETY: the kernel writes at most `size` bytes into `buf` and updates `size`.
    let r = unsafe { libc::sysctl(mib.as_mut_ptr(), mib.len() as libc::c_uint, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) };
    if r != 0 {
        return None;
    }
    parse_procargs2(&buf[..size.min(buf.len())])
}

#[cfg(target_os = "linux")]
fn argv_os(pid: u32) -> Option<Vec<String>> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let v: Vec<String> = raw.split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    (!v.is_empty()).then_some(v)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn argv_os(_pid: u32) -> Option<Vec<String>> {
    None
}

/// `KERN_PROCARGS2`: `argc` (i32), the exec path, NUL padding, then `argc` NUL-terminated args (then env, ignored).
pub fn parse_procargs2(b: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(b.get(..4)?.try_into().ok()?);
    if argc <= 0 {
        return None;
    }
    let mut i = 4;
    while i < b.len() && b[i] != 0 {
        i += 1;
    }
    while i < b.len() && b[i] == 0 {
        i += 1;
    }
    let mut out = Vec::with_capacity(argc as usize);
    while out.len() < argc as usize && i < b.len() {
        let start = i;
        while i < b.len() && b[i] != 0 {
            i += 1;
        }
        out.push(String::from_utf8_lossy(&b[start..i]).into_owned());
        i += 1;
    }
    (!out.is_empty()).then_some(out)
}

/// A process's working folder (libproc on macOS, `/proc/<pid>/cwd` on Linux).
pub fn cwd(pid: u32) -> Option<String> {
    cwd_os(pid)
}

#[cfg(target_os = "macos")]
fn cwd_os(pid: u32) -> Option<String> {
    // SAFETY: proc_vnodepathinfo is plain C data; all-zero is a valid value.
    let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
    // SAFETY: the kernel writes at most `size` bytes into `info`.
    let r = unsafe { libc::proc_pidinfo(pid as libc::c_int, libc::PROC_PIDVNODEPATHINFO, 0, (&mut info as *mut libc::proc_vnodepathinfo).cast(), size) };
    if r < size {
        return None;
    }
    let raw = &info.pvi_cdir.vip_path;
    // SAFETY: `vip_path` is a contiguous [[c_char; 32]; 32] (MAXPATHLEN bytes).
    let bytes = unsafe { std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), std::mem::size_of_val(raw)) };
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    let s = String::from_utf8_lossy(&bytes[..end]).into_owned();
    (!s.is_empty()).then_some(s)
}

#[cfg(target_os = "linux")]
fn cwd_os(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok().map(|p| p.to_string_lossy().into_owned())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn cwd_os(_pid: u32) -> Option<String> {
    None
}

/// The files a process holds open (`lsof -p`, 2 s timeout), cached per pid for 30 s.
pub fn open_paths(pid: u32) -> Vec<String> {
    static CACHE: OnceLock<Mutex<HashMap<u32, (Instant, Vec<String>)>>> = OnceLock::new();
    {
        let cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, v)) = cache.get(&pid) {
            if at.elapsed() < Duration::from_secs(30) {
                return v.clone();
            }
        }
    }
    let pid_s = pid.to_string();
    let paths: Vec<String> = if cfg!(unix) {
        run_with_timeout("lsof", &["-n", "-P", "-w", "-p", &pid_s, "-Fn"], Duration::from_secs(2))
            .map(|t| t.lines().filter_map(|l| l.strip_prefix('n')).filter(|p| p.starts_with('/')).map(str::to_string).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(30));
    cache.insert(pid, (Instant::now(), paths.clone()));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sessions_holders_ps_rows_and_apps() {
        let text = "\
    1     0 ??       Sat Nov 21 03:00:00 2026     /sbin/launchd
25690     1 ??       Sat Nov 21 03:01:00 2026     /System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal
16269 25690 ttys000  Sat Nov 21 03:55:40 2026     login
16270 16269 ttys000  Sat Nov 21 03:55:41 2026     -zsh
16285 16270 ttys000  Sat Nov 21 03:55:44 2026     claude
36964     1 ??       Sat Nov  1 09:00:00 2026     /Applications/Allternit Desktop.app/Contents/Resources/bin/allternit-factory
62319 36964 ttys063  Sat Nov 21 04:00:00 2026     sh
62320 62319 ttys063  Sat Nov 21 04:00:01 2026     claude
 7001  7000 ttys009  Sat Nov 21 05:00:00 2026     iTermServer-3.5.4
 7002  7001 ttys009  Sat Nov 21 05:00:01 2026     codex
 8001     1 ttys010  Sat Nov 21 05:00:00 2026     kimi-code
garbage line
";
        let rows = parse_ps(text);
        assert_eq!(rows.len(), 11);
        let factory = rows.iter().find(|r| r.pid == 36964).unwrap();
        assert_eq!(factory.comm, "/Applications/Allternit Desktop.app/Contents/Resources/bin/allternit-factory");
        assert_eq!(factory.started, "Sat Nov 1 09:00:00 2026");
        assert_eq!(factory.tty, None);
        let map = by_pid(&rows);
        assert_eq!(app_for(16285, &map), Some("Terminal"));
        assert_eq!(app_for(62320, &map), Some("Allternit"));
        assert_eq!(app_for(7002, &map), Some("iTerm2"));
        assert_eq!(app_for(8001, &map), Some("other"));
        assert_eq!(app_for(4242, &map), None);
    }

    #[test]
    fn live_sessions_holders_procargs2_parse() {
        let mut b = 3i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/usr/local/bin/codex\0\0\0\0codex\0--yolo\0fix it\0HOME=/x\0");
        assert_eq!(parse_procargs2(&b), Some(vec!["codex".to_string(), "--yolo".into(), "fix it".into()]));
        assert_eq!(parse_procargs2(&[0, 0]), None);
        assert_eq!(parse_procargs2(&0i32.to_ne_bytes()), None);
    }
}
