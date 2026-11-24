//! Listening sockets and the process that owns each one (`GET /v1/host/ports`).
//!
//! | OS | Source |
//! |---|---|
//! | Linux | `ss -ltnupH`; without `ss`, `/proc/net/{tcp,tcp6,udp,udp6}` plus the `/proc/<pid>/fd` socket inodes of every PID we may read |
//! | macOS | `lsof -nP -iTCP -sTCP:LISTEN` and `lsof -nP -iUDP` |
//! | Windows | `Get-NetTCPConnection -State Listen` and `Get-NetUDPEndpoint` joined with `Get-Process`; `netstat -ano` as fallback |
//!
//! Parsers are pure and run on every OS so the fixtures test everywhere. A
//! missing or slow tool is reported in [`PortsReport::sources`], not as an
//! error. Nothing here needs root; without it Linux and macOS leave `pid` and
//! `process` empty for sockets owned by other users.
//!
//! Exposure follows the contract (`loopback` | `lan` | `public`) and is based
//! on the bind address: loopback addresses are `loopback`, the wildcard
//! (`0.0.0.0`, `::`, `*`) and any non-private address are `public`, private,
//! link-local and unique-local addresses are `lan`. The CGNAT range
//! `100.64.0.0/10` and `fd7a:115c:a1e0::/48` (the mesh addresses) are `lan`
//! with the additive flag `mesh: true`.

use std::collections::{BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cmd::{first_line, run, SourceStatus};
use crate::services::{services, HostService};
use crate::{OsFamily, ProbeError, Result};

/// `lsof` and `powershell` can be slower than the 3 s default.
const SLOW_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

/// Who can reach the port, judged by the bind address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Exposure {
    Loopback,
    Lan,
    Public,
}

/// One listening socket. Contract fields: `port`, `protocol`, `address`,
/// `process`, `pid`, `exposure`. `mesh` is an additive extra.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPort {
    pub port: u16,
    pub protocol: Protocol,
    /// Bind address; the wildcard is written `0.0.0.0` or `::`.
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub exposure: Exposure,
    /// The bind address is a mesh (Tailscale-style) address.
    #[serde(default, skip_serializing_if = "is_false")]
    pub mesh: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl HostPort {
    fn new(protocol: Protocol, address: &str, port: u16, pid: Option<u32>, process: Option<String>) -> Self {
        let (exposure, mesh) = classify(address);
        HostPort {
            port,
            protocol,
            address: address.to_string(),
            process: process.filter(|p| !p.is_empty()),
            pid,
            exposure,
            mesh,
        }
    }
}

/// Listening sockets plus the status of each source that was tried.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortsReport {
    pub ports: Vec<HostPort>,
    pub sources: Vec<SourceStatus>,
}

/// Every listening port. Blocking; async callers use `spawn_blocking`.
pub fn ports() -> Result<Vec<HostPort>> {
    Ok(ports_report()?.ports)
}

/// Like [`ports`] but also says which sources could be read.
pub fn ports_report() -> Result<PortsReport> {
    let family = OsFamily::current()
        .ok_or_else(|| ProbeError::Unsupported(format!("ports on {}", std::env::consts::OS)))?;
    let (ports, sources) = match family {
        OsFamily::Linux => collect_linux(),
        OsFamily::Macos => collect_macos(),
        OsFamily::Windows => collect_windows(),
    };
    Ok(PortsReport {
        ports: finalize(ports),
        sources,
    })
}

/// Services with their `ports` filled in, matched on the main process PID.
pub fn services_with_ports() -> Result<Vec<HostService>> {
    let mut list = services()?;
    // A host whose ports cannot be read still has services to show.
    let found = ports().unwrap_or_default();
    attach_ports(&mut list, &found);
    Ok(list)
}

/// Fill `HostService::ports` from the ports whose owning PID matches.
pub fn attach_ports(services: &mut [HostService], ports: &[HostPort]) {
    let mut by_pid: HashMap<u32, BTreeSet<u16>> = HashMap::new();
    for p in ports {
        if let Some(pid) = p.pid {
            by_pid.entry(pid).or_default().insert(p.port);
        }
    }
    for s in services {
        if let Some(set) = s.pid.and_then(|pid| by_pid.get(&pid)) {
            s.ports = set.iter().copied().collect();
        }
    }
}

// ---------------------------------------------------------------------------
// Exposure
// ---------------------------------------------------------------------------

/// Exposure and mesh flag for a bind address.
pub fn classify(address: &str) -> (Exposure, bool) {
    let a = address.trim();
    if a.is_empty() || a == "*" {
        return (Exposure::Public, false);
    }
    match a.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => classify_v4(v4),
        Ok(IpAddr::V6(v6)) => classify_v6(v6),
        Err(_) if a.eq_ignore_ascii_case("localhost") => (Exposure::Loopback, false),
        Err(_) => (Exposure::Public, false),
    }
}

fn classify_v4(a: Ipv4Addr) -> (Exposure, bool) {
    let o = a.octets();
    if a.is_loopback() {
        (Exposure::Loopback, false)
    } else if a.is_unspecified() {
        (Exposure::Public, false)
    } else if o[0] == 100 && (o[1] & 0xc0) == 64 {
        (Exposure::Lan, true)
    } else if a.is_private() || a.is_link_local() {
        (Exposure::Lan, false)
    } else {
        (Exposure::Public, false)
    }
}

fn classify_v6(a: Ipv6Addr) -> (Exposure, bool) {
    if let Some(v4) = a.to_ipv4_mapped() {
        return classify_v4(v4);
    }
    let seg = a.segments();
    if a.is_loopback() {
        (Exposure::Loopback, false)
    } else if a.is_unspecified() {
        (Exposure::Public, false)
    } else if seg[0] == 0xfd7a && seg[1] == 0x115c && seg[2] == 0xa1e0 {
        (Exposure::Lan, true)
    } else if (seg[0] & 0xfe00) == 0xfc00 || (seg[0] & 0xffc0) == 0xfe80 {
        (Exposure::Lan, false)
    } else {
        (Exposure::Public, false)
    }
}

/// Sort, drop duplicates (one socket often shows once per file descriptor)
/// and prefer the row that knows its process.
fn finalize(mut ports: Vec<HostPort>) -> Vec<HostPort> {
    ports.sort_by(|a, b| {
        (a.port, a.protocol, &a.address, b.pid.is_some()).cmp(&(b.port, b.protocol, &b.address, a.pid.is_some()))
    });
    ports.dedup_by(|b, a| {
        a.port == b.port
            && a.protocol == b.protocol
            && a.address == b.address
            && (a.pid == b.pid || b.pid.is_none())
    });
    ports
}

/// Split `127.0.0.1:631`, `[::1]:631`, `*:80`, `127.0.0.53%lo:53` into
/// address and port. `*` becomes `0.0.0.0`; a zone (`%lo`) is dropped.
fn split_addr_port(s: &str) -> Option<(String, u16)> {
    let (addr, port) = s.trim().rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let addr = addr.trim_start_matches('[').trim_end_matches(']');
    let addr = addr.split('%').next().unwrap_or(addr);
    let addr = if addr == "*" || addr.is_empty() { "0.0.0.0" } else { addr };
    Some((addr.to_string(), port))
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

fn collect_linux() -> (Vec<HostPort>, Vec<SourceStatus>) {
    match run("ss", &["-ltnupH"], SLOW_TIMEOUT) {
        Ok(o) if o.code == Some(0) => (parse_ss(&o.stdout), vec![SourceStatus::ok("ss")]),
        Ok(o) => {
            let why = format!("ss: {}", first_line(&o.stderr));
            proc_fallback(why)
        }
        Err(e) => proc_fallback(e.reason("ss")),
    }
}

fn proc_fallback(ss_reason: String) -> (Vec<HostPort>, Vec<SourceStatus>) {
    let ports = read_proc_ports();
    let mut sources = vec![SourceStatus::unavailable("ss", ss_reason)];
    if ports.is_empty() && !std::path::Path::new("/proc/net/tcp").exists() {
        sources.push(SourceStatus::unavailable("/proc/net", "/proc/net/tcp not readable"));
    } else {
        sources.push(SourceStatus::ok("/proc/net"));
    }
    (ports, sources)
}

/// Parse `ss -ltnupH` output (no header).
///
/// ```text
/// tcp LISTEN 0 4096 127.0.0.53%lo:53 0.0.0.0:* users:(("systemd-resolve",pid=612,fd=14))
/// udp UNCONN 0 0    [::]:5353        [::]:*
/// ```
pub fn parse_ss(text: &str) -> Vec<HostPort> {
    let mut out = Vec::new();
    for line in text.lines() {
        let tok: Vec<&str> = line.split_whitespace().collect();
        if tok.len() < 5 {
            continue;
        }
        let protocol = match tok[0] {
            "tcp" | "tcp6" => Protocol::Tcp,
            "udp" | "udp6" => Protocol::Udp,
            _ => continue,
        };
        let Some((addr, port)) = split_addr_port(tok[4]) else { continue };
        let (pid, process) = ss_users(line);
        out.push(HostPort::new(protocol, &addr, port, pid, process));
    }
    out
}

/// First `("name",pid=N` of a `users:(...)` column.
fn ss_users(line: &str) -> (Option<u32>, Option<String>) {
    let Some(i) = line.find("users:((\"") else { return (None, None) };
    let rest = &line[i + "users:((\"".len()..];
    let Some(end) = rest.find('"') else { return (None, None) };
    let name = rest[..end].to_string();
    let pid = rest[end..].find("pid=").and_then(|j| {
        let digits: String = rest[end + j + 4..].chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    });
    (pid, Some(name))
}

/// One row of `/proc/net/{tcp,udp}[6]`, before the inode is mapped to a PID.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcSocket {
    pub protocol: Protocol,
    pub address: String,
    pub port: u16,
    pub inode: u64,
}

/// Parse `/proc/net/tcp`, `tcp6`, `udp` or `udp6`. TCP keeps state `0A`
/// (listen), UDP keeps state `07` (unconnected). Addresses are the kernel's
/// host-order hex words, read as little endian (every Linux target we ship).
pub fn parse_proc_net(text: &str, protocol: Protocol, v6: bool) -> Vec<ProcSocket> {
    let want_state = match protocol {
        Protocol::Tcp => "0A",
        Protocol::Udp => "07",
    };
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !f[3].eq_ignore_ascii_case(want_state) {
            continue;
        }
        let Some((ahex, phex)) = f[1].split_once(':') else { continue };
        let (Ok(port), Ok(inode)) = (u16::from_str_radix(phex, 16), f[9].parse::<u64>()) else { continue };
        let Some(address) = (if v6 { hex_v6(ahex) } else { hex_v4(ahex) }) else { continue };
        out.push(ProcSocket {
            protocol,
            address,
            port,
            inode,
        });
    }
    out
}

fn hex_v4(h: &str) -> Option<String> {
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Ipv4Addr::from(v.to_le_bytes()).to_string())
}

fn hex_v6(h: &str) -> Option<String> {
    if h.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for i in 0..4 {
        let w = u32::from_str_radix(&h[i * 8..i * 8 + 8], 16).ok()?;
        bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    Some(Ipv6Addr::from(bytes).to_string())
}

/// `socket:[12345]` link target to its inode.
fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok()
}

/// Map socket inode to `(pid, comm)` for every PID whose fd table we can read.
fn inode_owners() -> HashMap<u64, (u32, String)> {
    let mut map = HashMap::new();
    let Ok(dir) = std::fs::read_dir("/proc") else { return map };
    for entry in dir.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else { continue };
        let mut comm: Option<String> = None;
        for fd in fds.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else { continue };
            let Some(inode) = target.to_str().and_then(socket_inode) else { continue };
            let name = comm.get_or_insert_with(|| {
                std::fs::read_to_string(entry.path().join("comm"))
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            });
            map.entry(inode).or_insert((pid, name.clone()));
        }
    }
    map
}

fn read_proc_ports() -> Vec<HostPort> {
    let owners = inode_owners();
    let mut out = Vec::new();
    for (file, protocol, v6) in [
        ("/proc/net/tcp", Protocol::Tcp, false),
        ("/proc/net/tcp6", Protocol::Tcp, true),
        ("/proc/net/udp", Protocol::Udp, false),
        ("/proc/net/udp6", Protocol::Udp, true),
    ] {
        let Ok(text) = std::fs::read_to_string(file) else { continue };
        for s in parse_proc_net(&text, protocol, v6) {
            let owner = owners.get(&s.inode);
            out.push(HostPort::new(
                s.protocol,
                &s.address,
                s.port,
                owner.map(|o| o.0),
                owner.map(|o| o.1.clone()),
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

fn collect_macos() -> (Vec<HostPort>, Vec<SourceStatus>) {
    let mut ports = Vec::new();
    let mut sources = Vec::new();
    for (protocol, args, name) in [
        (Protocol::Tcp, &["-nP", "-iTCP", "-sTCP:LISTEN"][..], "lsof tcp"),
        (Protocol::Udp, &["-nP", "-iUDP"][..], "lsof udp"),
    ] {
        match run("lsof", args, SLOW_TIMEOUT) {
            // lsof exits 1 when it finds nothing; only a real failure carries stderr.
            Ok(o) if o.code == Some(0) || (o.code == Some(1) && o.stderr.trim().is_empty()) => {
                ports.extend(parse_lsof(&o.stdout, protocol));
                sources.push(SourceStatus::ok(name));
            }
            Ok(o) => sources.push(SourceStatus::unavailable(name, format!("lsof: {}", first_line(&o.stderr)))),
            Err(e) => sources.push(SourceStatus::unavailable(name, e.reason("lsof"))),
        }
    }
    (ports, sources)
}

/// Parse `lsof -nP -iTCP -sTCP:LISTEN` / `-iUDP` output.
///
/// ```text
/// COMMAND   PID USER   FD   TYPE DEVICE SIZE/OFF NODE NAME
/// ControlCe 612 joe    8u  IPv4 0x1234      0t0  TCP *:7000 (LISTEN)
/// mDNSRespo 301 root  22u  IPv6 0x5678      0t0  UDP *:5353
/// ```
///
/// UDP rows with `->` are connected sockets, not listeners, and are skipped.
pub fn parse_lsof(text: &str, protocol: Protocol) -> Vec<HostPort> {
    let want = match protocol {
        Protocol::Tcp => "TCP",
        Protocol::Udp => "UDP",
    };
    let mut out = Vec::new();
    for line in text.lines() {
        let tok: Vec<&str> = line.split_whitespace().collect();
        if tok.len() < 9 || tok[7] != want || tok[8].contains("->") {
            continue;
        }
        let Ok(pid) = tok[1].parse::<u32>() else { continue };
        if protocol == Protocol::Tcp && !line.contains("(LISTEN)") {
            continue;
        }
        let Some((addr, port)) = split_addr_port(tok[8]) else { continue };
        // lsof escapes spaces in command names as \x20.
        let name = tok[0].replace("\\x20", " ");
        out.push(HostPort::new(protocol, &addr, port, Some(pid), Some(name)));
    }
    out
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

const WINDOWS_SCRIPT: &str = r#"$ErrorActionPreference='Stop'
$names=@{}
Get-Process | ForEach-Object { $names[[int]$_.Id]=$_.ProcessName }
$rows=@()
$rows += Get-NetTCPConnection -State Listen | ForEach-Object { [pscustomobject]@{Protocol='tcp';Address=$_.LocalAddress;Port=[int]$_.LocalPort;Pid=[int]$_.OwningProcess;Process=$names[[int]$_.OwningProcess]} }
$rows += Get-NetUDPEndpoint | ForEach-Object { [pscustomobject]@{Protocol='udp';Address=$_.LocalAddress;Port=[int]$_.LocalPort;Pid=[int]$_.OwningProcess;Process=$names[[int]$_.OwningProcess]} }
ConvertTo-Json -Compress -InputObject @($rows)"#;

fn collect_windows() -> (Vec<HostPort>, Vec<SourceStatus>) {
    let ps = run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT],
        SLOW_TIMEOUT,
    );
    let ps_reason = match ps {
        Ok(o) if o.code == Some(0) => match parse_windows_json(&o.stdout) {
            Ok(list) => return (list, vec![SourceStatus::ok("Get-NetTCPConnection")]),
            Err(e) => e,
        },
        Ok(o) => format!("powershell: {}", first_line(&o.stderr)),
        Err(e) => e.reason("powershell"),
    };
    let mut sources = vec![SourceStatus::unavailable("Get-NetTCPConnection", ps_reason)];
    match run("netstat", &["-ano"], SLOW_TIMEOUT) {
        Ok(o) if o.code == Some(0) => {
            let mut list = parse_netstat(&o.stdout);
            fill_process_names(&mut list);
            sources.push(SourceStatus::ok("netstat"));
            (list, sources)
        }
        Ok(o) => {
            sources.push(SourceStatus::unavailable("netstat", format!("netstat: {}", first_line(&o.stderr))));
            (Vec::new(), sources)
        }
        Err(e) => {
            sources.push(SourceStatus::unavailable("netstat", e.reason("netstat")));
            (Vec::new(), sources)
        }
    }
}

/// Parse the JSON the PowerShell script prints: an array of
/// `{Protocol, Address, Port, Pid, Process}`, or one bare object, or nothing.
pub fn parse_windows_json(text: &str) -> std::result::Result<Vec<HostPort>, String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let rows = match v {
        serde_json::Value::Array(a) => a,
        serde_json::Value::Null => Vec::new(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for r in &rows {
        let protocol = match r.get("Protocol").and_then(|p| p.as_str()) {
            Some("tcp") => Protocol::Tcp,
            Some("udp") => Protocol::Udp,
            _ => continue,
        };
        let Some(port) = r.get("Port").and_then(|p| p.as_u64()).and_then(|p| u16::try_from(p).ok()) else {
            continue;
        };
        let addr = r.get("Address").and_then(|a| a.as_str()).unwrap_or("0.0.0.0");
        let addr = addr.split('%').next().unwrap_or(addr);
        let addr = if addr.is_empty() { "0.0.0.0" } else { addr };
        // Pid 0 is the idle process, which owns nothing real.
        let pid = r.get("Pid").and_then(|p| p.as_u64()).filter(|p| *p > 0).map(|p| p as u32);
        let name = r.get("Process").and_then(|p| p.as_str()).map(str::to_string);
        out.push(HostPort::new(protocol, addr, port, pid, name));
    }
    Ok(out)
}

/// Parse `netstat -ano`: listening TCP rows and UDP rows.
///
/// ```text
///   TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1004
///   UDP    0.0.0.0:123            *:*                                    1176
/// ```
pub fn parse_netstat(text: &str) -> Vec<HostPort> {
    let mut out = Vec::new();
    for line in text.lines() {
        let tok: Vec<&str> = line.split_whitespace().collect();
        let (protocol, pid_tok) = match (tok.first().copied(), tok.len()) {
            (Some("TCP"), 5) if tok[3] == "LISTENING" => (Protocol::Tcp, tok[4]),
            (Some("UDP"), 4) => (Protocol::Udp, tok[3]),
            _ => continue,
        };
        let Some((addr, port)) = split_addr_port(tok[1]) else { continue };
        let pid = pid_tok.parse::<u32>().ok().filter(|p| *p > 0);
        out.push(HostPort::new(protocol, &addr, port, pid, None));
    }
    out
}

/// Name the PIDs of rows that have none, using sysinfo.
fn fill_process_names(list: &mut [HostPort]) {
    let mut sys = sysinfo::System::new();
    sys.refresh_processes();
    for p in list.iter_mut().filter(|p| p.process.is_none()) {
        if let Some(proc_) = p.pid.and_then(|pid| sys.process(sysinfo::Pid::from_u32(pid))) {
            p.process = Some(proc_.name().to_string());
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ServiceProvider;
    use crate::ServiceState;

    const SS: &str = include_str!("../fixtures/ports/ss-ltnupH.txt");
    const PROC_TCP: &str = include_str!("../fixtures/ports/proc-net-tcp");
    const PROC_TCP6: &str = include_str!("../fixtures/ports/proc-net-tcp6");
    const PROC_UDP: &str = include_str!("../fixtures/ports/proc-net-udp");
    const LSOF_TCP: &str = include_str!("../fixtures/ports/lsof-tcp.txt");
    const LSOF_UDP: &str = include_str!("../fixtures/ports/lsof-udp.txt");
    const WIN_JSON: &str = include_str!("../fixtures/ports/windows-ports.json");
    const NETSTAT: &str = include_str!("../fixtures/ports/netstat-ano.txt");

    fn find(v: &[HostPort], proto: Protocol, port: u16, addr: &str) -> HostPort {
        v.iter()
            .find(|p| p.protocol == proto && p.port == port && p.address == addr)
            .unwrap_or_else(|| panic!("no {proto:?} {addr}:{port} in {v:#?}"))
            .clone()
    }

    #[test]
    fn exposure_classes() {
        assert_eq!(classify("127.0.0.1"), (Exposure::Loopback, false));
        assert_eq!(classify("127.0.0.53"), (Exposure::Loopback, false));
        assert_eq!(classify("::1"), (Exposure::Loopback, false));
        assert_eq!(classify("0.0.0.0"), (Exposure::Public, false));
        assert_eq!(classify("::"), (Exposure::Public, false));
        assert_eq!(classify("*"), (Exposure::Public, false));
        assert_eq!(classify("203.0.113.9"), (Exposure::Public, false));
        assert_eq!(classify("192.168.1.20"), (Exposure::Lan, false));
        assert_eq!(classify("10.0.0.4"), (Exposure::Lan, false));
        assert_eq!(classify("172.16.5.1"), (Exposure::Lan, false));
        assert_eq!(classify("169.254.1.1"), (Exposure::Lan, false));
        assert_eq!(classify("fe80::1"), (Exposure::Lan, false));
        assert_eq!(classify("fd00::1"), (Exposure::Lan, false));
        assert_eq!(classify("2001:db8::1"), (Exposure::Public, false));
        assert_eq!(classify("::ffff:127.0.0.1"), (Exposure::Loopback, false));
    }

    #[test]
    fn mesh_range_is_lan_with_flag() {
        assert_eq!(classify("100.64.0.1"), (Exposure::Lan, true));
        assert_eq!(classify("100.101.102.103"), (Exposure::Lan, true));
        assert_eq!(classify("100.127.255.254"), (Exposure::Lan, true));
        // Just outside 100.64.0.0/10.
        assert_eq!(classify("100.63.255.255"), (Exposure::Public, false));
        assert_eq!(classify("100.128.0.1"), (Exposure::Public, false));
        assert_eq!(classify("fd7a:115c:a1e0::1"), (Exposure::Lan, true));
    }

    #[test]
    fn address_splitting() {
        assert_eq!(split_addr_port("127.0.0.53%lo:53"), Some(("127.0.0.53".into(), 53)));
        assert_eq!(split_addr_port("[::1]:631"), Some(("::1".into(), 631)));
        assert_eq!(split_addr_port("*:80"), Some(("0.0.0.0".into(), 80)));
        assert_eq!(split_addr_port("[fe80::1%eth0]:546"), Some(("fe80::1".into(), 546)));
        assert_eq!(split_addr_port("*:*"), None);
    }

    #[test]
    fn ss_rows_with_and_without_process() {
        let v = parse_ss(SS);
        assert_eq!(v.len(), 8);
        let ssh = find(&v, Protocol::Tcp, 22, "0.0.0.0");
        assert_eq!(ssh.pid, Some(871));
        assert_eq!(ssh.process.as_deref(), Some("sshd"));
        assert_eq!(ssh.exposure, Exposure::Public);
        let dns = find(&v, Protocol::Tcp, 53, "127.0.0.53");
        assert_eq!(dns.process.as_deref(), Some("systemd-resolve"));
        assert_eq!(dns.exposure, Exposure::Loopback);
        let v6 = find(&v, Protocol::Tcp, 22, "::");
        assert_eq!(v6.exposure, Exposure::Public);
        let ts = find(&v, Protocol::Tcp, 41641, "100.101.102.103");
        assert!(ts.mesh);
        assert_eq!(ts.exposure, Exposure::Lan);
        let udp = find(&v, Protocol::Udp, 68, "192.168.1.20");
        assert_eq!(udp.protocol, Protocol::Udp);
        assert_eq!(udp.exposure, Exposure::Lan);
        // Owned by another user, no `users:` column without root.
        let other = find(&v, Protocol::Tcp, 5432, "127.0.0.1");
        assert_eq!((other.pid, other.process.clone()), (None, None));
        // First of several owners wins.
        let nginx = find(&v, Protocol::Tcp, 80, "0.0.0.0");
        assert_eq!(nginx.process.as_deref(), Some("nginx"));
        assert_eq!(nginx.pid, Some(1204));
    }

    #[test]
    fn proc_net_address_decoding() {
        assert_eq!(hex_v4("0100007F").as_deref(), Some("127.0.0.1"));
        assert_eq!(hex_v4("00000000").as_deref(), Some("0.0.0.0"));
        assert_eq!(hex_v4("0100A8C0").as_deref(), Some("192.168.0.1"));
        assert_eq!(hex_v6("00000000000000000000000001000000").as_deref(), Some("::1"));
        assert_eq!(hex_v6("00000000000000000000000000000000").as_deref(), Some("::"));
        assert_eq!(hex_v6("0000000000000000FFFF00000100007F").as_deref(), Some("::ffff:127.0.0.1"));
        assert_eq!(hex_v6("short"), None);
    }

    #[test]
    fn proc_net_keeps_only_listeners() {
        let tcp = parse_proc_net(PROC_TCP, Protocol::Tcp, false);
        // The ESTABLISHED row (state 01) is dropped.
        assert_eq!(tcp.len(), 3);
        let ssh = tcp.iter().find(|s| s.port == 22).unwrap();
        assert_eq!((ssh.address.as_str(), ssh.inode), ("0.0.0.0", 18321));
        let lo = tcp.iter().find(|s| s.port == 631).unwrap();
        assert_eq!(lo.address, "127.0.0.1");
        let tcp6 = parse_proc_net(PROC_TCP6, Protocol::Tcp, true);
        assert_eq!(tcp6.len(), 2);
        assert_eq!(tcp6.iter().find(|s| s.port == 22).unwrap().address, "::");
        assert_eq!(tcp6.iter().find(|s| s.port == 631).unwrap().address, "::1");
        let udp = parse_proc_net(PROC_UDP, Protocol::Udp, false);
        // A connected UDP socket (state 01) is not a listener.
        assert_eq!(udp.len(), 2);
        assert!(udp.iter().any(|s| s.port == 68 && s.address == "192.168.1.20"));
    }

    #[test]
    fn socket_inode_links() {
        assert_eq!(socket_inode("socket:[18321]"), Some(18321));
        assert_eq!(socket_inode("pipe:[18321]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }

    #[test]
    fn lsof_tcp_and_udp() {
        let tcp = parse_lsof(LSOF_TCP, Protocol::Tcp);
        assert_eq!(tcp.len(), 5);
        let control = find(&tcp, Protocol::Tcp, 7000, "0.0.0.0");
        assert_eq!(control.pid, Some(612));
        assert_eq!(control.process.as_deref(), Some("ControlCe"));
        assert_eq!(control.exposure, Exposure::Public);
        let cups = find(&tcp, Protocol::Tcp, 631, "::1");
        assert_eq!(cups.exposure, Exposure::Loopback);
        let spaced = find(&tcp, Protocol::Tcp, 8080, "127.0.0.1");
        assert_eq!(spaced.process.as_deref(), Some("Google Chrome"));
        let udp = parse_lsof(LSOF_UDP, Protocol::Udp);
        // The `->` row is a connected socket.
        assert_eq!(udp.len(), 2);
        assert_eq!(find(&udp, Protocol::Udp, 5353, "0.0.0.0").process.as_deref(), Some("mDNSRespo"));
    }

    #[test]
    fn windows_json_array_and_single() {
        let v = parse_windows_json(WIN_JSON).unwrap();
        assert_eq!(v.len(), 5);
        let rpc = find(&v, Protocol::Tcp, 135, "0.0.0.0");
        assert_eq!(rpc.pid, Some(1004));
        assert_eq!(rpc.process.as_deref(), Some("svchost"));
        assert_eq!(find(&v, Protocol::Tcp, 445, "::").exposure, Exposure::Public);
        let lo = find(&v, Protocol::Tcp, 5040, "127.0.0.1");
        assert_eq!(lo.exposure, Exposure::Loopback);
        // Zone suffix dropped; pid 0 is no owner.
        let ll = find(&v, Protocol::Udp, 546, "fe80::1");
        assert_eq!((ll.pid, ll.exposure), (None, Exposure::Lan));
        let single = r#"{"Protocol":"tcp","Address":"0.0.0.0","Port":80,"Pid":4,"Process":"System"}"#;
        assert_eq!(parse_windows_json(single).unwrap().len(), 1);
        assert!(parse_windows_json("").unwrap().is_empty());
        assert!(parse_windows_json("null").unwrap().is_empty());
        assert!(parse_windows_json("Get-NetTCPConnection : denied").is_err());
    }

    #[test]
    fn netstat_listeners_only() {
        let v = parse_netstat(NETSTAT);
        // ESTABLISHED and TIME_WAIT rows are dropped.
        assert_eq!(v.len(), 5);
        let rpc = find(&v, Protocol::Tcp, 135, "0.0.0.0");
        assert_eq!(rpc.pid, Some(1004));
        assert_eq!(rpc.process, None, "names are filled from sysinfo afterwards");
        assert_eq!(find(&v, Protocol::Tcp, 445, "::").exposure, Exposure::Public);
        assert_eq!(find(&v, Protocol::Udp, 123, "0.0.0.0").pid, Some(1176));
        assert_eq!(find(&v, Protocol::Udp, 1900, "192.168.1.20").exposure, Exposure::Lan);
    }

    #[test]
    fn duplicates_collapse_and_keep_the_owner() {
        let owned = HostPort::new(Protocol::Tcp, "0.0.0.0", 80, Some(9), Some("nginx".into()));
        let anon = HostPort::new(Protocol::Tcp, "0.0.0.0", 80, None, None);
        let again = owned.clone();
        let out = finalize(vec![anon, owned.clone(), again]);
        assert_eq!(out, vec![owned]);
    }

    #[test]
    fn ports_attach_to_services_by_pid() {
        let mut svc = vec![
            service("nginx", Some(1204)),
            service("cron", Some(700)),
            service("docker", None),
        ];
        let ports = parse_ss(SS);
        attach_ports(&mut svc, &ports);
        assert_eq!(svc[0].ports, vec![80]);
        assert!(svc[1].ports.is_empty());
        assert!(svc[2].ports.is_empty());
        // sshd on v4 and v6 gives one entry per port.
        let mut sshd = vec![service("ssh", Some(871))];
        attach_ports(&mut sshd, &ports);
        assert_eq!(sshd[0].ports, vec![22]);
    }

    fn service(name: &str, pid: Option<u32>) -> HostService {
        let mut s = HostService::new(ServiceProvider::Systemd, name, name, ServiceState::Running);
        s.pid = pid;
        s
    }

    #[test]
    fn json_uses_contract_field_names() {
        let p = HostPort::new(Protocol::Tcp, "100.64.0.9", 8080, Some(7), Some("node".into()));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["port"], 8080);
        assert_eq!(v["protocol"], "tcp");
        assert_eq!(v["address"], "100.64.0.9");
        assert_eq!(v["process"], "node");
        assert_eq!(v["pid"], 7);
        assert_eq!(v["exposure"], "lan");
        assert_eq!(v["mesh"], true);
        let q = HostPort::new(Protocol::Udp, "0.0.0.0", 53, None, Some(String::new()));
        let v = serde_json::to_value(&q).unwrap();
        assert!(v.get("mesh").is_none() && v.get("pid").is_none() && v.get("process").is_none());
        assert_eq!(v["exposure"], "public");
        let back: HostPort = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn ports_report_never_errors_on_known_os() {
        // Real host: a tool may be missing, but a known OS never returns Err.
        let r = ports_report().unwrap();
        assert!(!r.sources.is_empty());
    }
}
