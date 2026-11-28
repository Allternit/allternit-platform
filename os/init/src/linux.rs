use allternit_init::config::{self, Action, Config, Exit, Service};
use allternit_init::verity::{self, Verity};
use libc::{c_int, c_void};
use std::ffi::CString;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CONFIG_PATH: &str = "/etc/allternit/init.toml";
const AUTH_SOCK: &str = "/run/allternit/authority.sock";
const READY_LINE: &str = "ALLTERNIT-OS READY kernel-interface=1";

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: c_int) {
    STOP.store(true, Ordering::SeqCst);
}

fn log(msg: &str) {
    // Stdout is /dev/console once console_setup ran.
    let _ = writeln!(std::io::stdout(), "init: {msg}");
    let _ = std::io::stdout().flush();
}

fn cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new("").unwrap())
}

fn mount(src: &str, target: &str, fstype: &str, flags: u64, data: &str) -> bool {
    let _ = std::fs::create_dir_all(target);
    let (s, t, f, d) = (cstr(src), cstr(target), cstr(fstype), cstr(data));
    let rc = unsafe {
        libc::mount(s.as_ptr(), t.as_ptr(), f.as_ptr(), flags as libc::c_ulong, d.as_ptr() as *const c_void)
    };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EBUSY)
}

fn mount_basics() {
    let nsd = libc::MS_NOSUID | libc::MS_NODEV;
    for (src, dst, fs, fl, data) in [
        ("proc", "/proc", "proc", nsd | libc::MS_NOEXEC, ""),
        ("sysfs", "/sys", "sysfs", nsd | libc::MS_NOEXEC, ""),
        ("devtmpfs", "/dev", "devtmpfs", libc::MS_NOSUID, "mode=0755"),
        ("tmpfs", "/run", "tmpfs", nsd, "mode=0755,size=64m"),
    ] {
        if !mount(src, dst, fs, fl as u64, data) {
            eprintln!("init: mount {dst} failed: {}", std::io::Error::last_os_error());
        }
    }
}

fn console_setup() {
    unsafe {
        libc::setsid();
        let fd = libc::open(cstr("/dev/console").as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        if fd >= 0 {
            libc::ioctl(fd, libc::TIOCSCTTY as _, 0);
            for t in 0..3 {
                libc::dup2(fd, t);
            }
            if fd > 2 {
                libc::close(fd);
            }
        }
        // Serial line ttyS0: 115200 8N1, best effort.
        let s = libc::open(cstr("/dev/ttyS0").as_ptr(), libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK);
        if s >= 0 {
            let mut tio: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(s, &mut tio) == 0 {
                libc::cfsetispeed(&mut tio, libc::B115200);
                libc::cfsetospeed(&mut tio, libc::B115200);
                tio.c_cflag |= libc::CS8 | libc::CREAD | libc::CLOCAL;
                tio.c_cflag &= !(libc::PARENB | libc::CSTOPB);
                libc::tcsetattr(s, libc::TCSANOW, &tio);
            }
            libc::close(s);
        }
    }
}

fn set_hostname(name: &str) {
    let rc = unsafe { libc::sethostname(name.as_ptr() as *const libc::c_char, name.len() as _) };
    if rc != 0 {
        log(&format!("sethostname failed: {}", std::io::Error::last_os_error()));
    }
}

fn mount_state(cfg: &Config) -> bool {
    let Some(dev) = &cfg.state_device else { return false };
    if !std::path::Path::new(dev).exists() {
        log(&format!("no state partition at {dev}, running without state"));
        return false;
    }
    if mount(dev, &cfg.state_mount, &cfg.state_fstype, (libc::MS_NOSUID | libc::MS_NODEV) as u64, "") {
        log(&format!("state mounted at {}", cfg.state_mount));
        true
    } else {
        log(&format!("state mount failed: {}", std::io::Error::last_os_error()));
        false
    }
}

/// Bring `lo` up in the root netns so 127.0.0.1 works (the model listens there until the
/// ADR-0006 private-netns + socket proxy lands).
fn loopback_up() {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
        if fd < 0 {
            return;
        }
        let mut ifr: libc::ifreq = std::mem::zeroed();
        for (i, b) in b"lo\0".iter().enumerate() {
            ifr.ifr_name[i] = *b as libc::c_char;
        }
        ifr.ifr_ifru.ifru_flags = (libc::IFF_UP | libc::IFF_RUNNING) as libc::c_short;
        if libc::ioctl(fd, libc::SIOCSIFFLAGS as _, &ifr) != 0 {
            log(&format!("lo up failed: {}", std::io::Error::last_os_error()));
        }
        libc::close(fd);
    }
}

fn prepare_run_dirs(cfg: &Config) {
    // Socket directories: owned by the service that binds in them.
    let _ = std::fs::create_dir_all("/run/allternit/model");
    for (path, name) in [("/run/allternit", "authority"), ("/run/allternit/model", "model")] {
        if let Some(s) = cfg.services.iter().find(|s| s.name == name) {
            unsafe { libc::chown(cstr(path).as_ptr(), s.uid, s.gid) };
        }
    }
    // Shell home on the state partition, if present.
    if let Some(s) = cfg.services.iter().find(|s| s.name == "shell") {
        if let Some((_, home)) = s.env.iter().find(|(k, _)| k == "HOME") {
            if std::fs::create_dir_all(home).is_ok() {
                unsafe { libc::chown(cstr(home).as_ptr(), s.uid, s.gid) };
            }
        }
    }
}

fn spawn(svc: &Service) -> Result<i32, String> {
    let mut cmd = Command::new(&svc.path);
    cmd.args(&svc.args).env_clear();
    cmd.env("PATH", "/usr/bin:/bin");
    for (k, v) in &svc.env {
        cmd.env(k, v);
    }
    let (uid, gid, netns) = (svc.uid, svc.gid, svc.netns);
    unsafe {
        cmd.pre_exec(move || {
            if netns && libc::unshare(libc::CLONE_NEWNET) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::setgroups(0, std::ptr::null()) != 0
                || libc::setgid(gid) != 0
                || libc::setuid(uid) != 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    // Child is reaped by the waitpid(-1) loop, not by std.
    cmd.spawn().map(|c| c.id() as i32).map_err(|e| e.to_string())
}

fn wait_for_socket(path: &str, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if UnixStream::connect(path).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn ask(class: &str) -> Option<String> {
    let mut s = UnixStream::connect(AUTH_SOCK).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    writeln!(s, r#"{{"op":"check","action":"init-self-check","class":"{class}"}}"#).ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    Some(line)
}

/// One allow plus one deny must come back from the authority.
fn kernel_check() -> bool {
    let allow = ask("read").map_or(false, |r| r.contains("\"decision\":\"allow\""));
    let deny = ask("money").map_or(false, |r| r.contains("\"decision\":\"deny\""));
    allow && deny
}

struct Running {
    idx: usize,
    pid: i32,
    started: Instant,
}

fn reap() -> Vec<(i32, Exit)> {
    let mut out = Vec::new();
    loop {
        let mut st: c_int = 0;
        let pid = unsafe { libc::waitpid(-1, &mut st, libc::WNOHANG) };
        if pid <= 0 {
            return out;
        }
        let exit = if libc::WIFEXITED(st) {
            Exit::Code(libc::WEXITSTATUS(st))
        } else {
            Exit::Signal(libc::WTERMSIG(st))
        };
        out.push((pid, exit));
    }
}

fn shutdown(running: &[Running], state_mounted: bool, cfg: &Config) -> ! {
    log("shutting down");
    for r in running {
        unsafe { libc::kill(r.pid, libc::SIGTERM) };
    }
    let end = Instant::now() + Duration::from_secs(5);
    let mut left: Vec<i32> = running.iter().map(|r| r.pid).collect();
    while !left.is_empty() && Instant::now() < end {
        for (pid, _) in reap() {
            left.retain(|p| *p != pid);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    for pid in &left {
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
    reap();
    unsafe { libc::sync() };
    if state_mounted {
        unsafe { libc::umount2(cstr(&cfg.state_mount).as_ptr(), 0) };
    }
    unsafe {
        libc::umount2(cstr("/run").as_ptr(), 0);
        libc::sync();
        libc::reboot(libc::RB_POWER_OFF);
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

// --- system slot: dm-verity + switch root (plan step 30a) ------------------------------

const SYSROOT: &str = "/sysroot";

/// Power off now. Used when no slot may boot.
fn halt() -> ! {
    let _ = std::io::stdout().flush();
    unsafe {
        libc::sync();
        libc::reboot(libc::RB_POWER_OFF);
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

/// The slot failed verification: say so on the console and do not boot it.
fn slot_fail(slot: &str, reason: &str) -> ! {
    println!("verity: FAIL slot={slot} ({reason})");
    // TODO(30b): try the other slot (health-confirmed slot switch) instead of powering off.
    log(&format!("slot {slot} is not booted; no fallback slot yet (step 30b), powering off"));
    halt()
}

fn wait_for_path(path: &str, secs: u64) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        if std::path::Path::new(path).exists() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn mknod_dev(path: &str, kind: libc::mode_t, major: u32, minor: u32) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let rc = unsafe { libc::mknod(cstr(path).as_ptr(), kind | 0o600, libc::makedev(major, minor)) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("mknod {path}: {}", std::io::Error::last_os_error()))
    }
}

/// /dev/mapper/control from devtmpfs, or our own node for misc 10:236 if it is missing.
fn dm_control() -> Result<std::fs::File, String> {
    let open = |p: &str| std::fs::OpenOptions::new().read(true).write(true).open(p);
    if let Ok(f) = open("/dev/mapper/control") {
        return Ok(f);
    }
    // Not under /run: it is mounted nodev.
    let _ = std::fs::create_dir_all("/dev/mapper");
    mknod_dev("/dev/mapper/control", libc::S_IFCHR, 10, 236)?;
    open("/dev/mapper/control").map_err(|e| format!("device-mapper control: {e} (CONFIG_BLK_DEV_DM?)"))
}

fn dm_call(ctl: &std::fs::File, nr: u32, buf: &mut [u8]) -> Result<(), String> {
    let rc = unsafe { libc::ioctl(ctl.as_raw_fd(), verity::dm_ioctl_request(nr) as _, buf.as_mut_ptr()) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("dm ioctl {nr}: {}", std::io::Error::last_os_error()))
    }
}

/// Create a read-only dm-verity device over the slot and read all of it once, so a bad
/// block anywhere fails now (dm-verity checks blocks as they are read). Returns its node.
fn open_verity(v: &Verity) -> Result<String, String> {
    let dev = v.slot.device();
    if !wait_for_path(dev, 10) {
        return Err(format!("no partition at {dev}"));
    }
    let name = v.slot.dm_name();
    let ctl = dm_control()?;
    dm_call(&ctl, verity::DM_DEV_CREATE, &mut verity::dm_buffer(name, 0, None)?)?;
    let (sectors, params) = v.table();
    let loaded = verity::dm_buffer(name, verity::DM_READONLY_FLAG, Some((sectors, "verity", &params)))
        .and_then(|mut b| dm_call(&ctl, verity::DM_TABLE_LOAD, &mut b));
    if let Err(e) = loaded {
        if let Ok(mut b) = verity::dm_buffer(name, 0, None) {
            let _ = dm_call(&ctl, verity::DM_DEV_REMOVE, &mut b);
        }
        return Err(format!("table load: {e}"));
    }
    // DEV_SUSPEND without the suspend flag = resume, which makes the loaded table live.
    let mut resume = verity::dm_buffer(name, 0, None)?;
    dm_call(&ctl, verity::DM_DEV_SUSPEND, &mut resume)?;
    let (major, minor) = verity::dm_dev(&resume);
    let node = format!("/dev/mapper/{name}");
    mknod_dev(&node, libc::S_IFBLK, major, minor)?;

    let mut f = std::fs::File::open(&node).map_err(|e| format!("open {node}: {e}"))?;
    let mut buf = vec![0u8; 1 << 20];
    let mut total: u64 = 0;
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => total += n as u64,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("read failed at byte {total}: {e}")),
        }
    }
    if total != v.data_bytes() {
        return Err(format!("read {total} bytes, want {}", v.data_bytes()));
    }
    Ok(node)
}

fn sys_mount(src: &str, target: &str, fstype: &str, flags: libc::c_ulong, data: &str) -> Result<(), String> {
    let (s, t, f, d) = (cstr(src), cstr(target), cstr(fstype), cstr(data));
    let fp = if fstype.is_empty() { std::ptr::null() } else { f.as_ptr() };
    let rc = unsafe { libc::mount(s.as_ptr(), t.as_ptr(), fp, flags, d.as_ptr() as *const c_void) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("mount {src} on {target}: {}", std::io::Error::last_os_error()))
    }
}

/// Mount the verified slot read-only and make it `/` (busybox switch_root order). The
/// initramfs stays in memory; services and init.toml now come from the slot.
fn switch_root(node: &str) -> Result<(), String> {
    let _ = std::fs::create_dir_all(SYSROOT);
    sys_mount(node, SYSROOT, "ext4", libc::MS_RDONLY | libc::MS_NODEV, "")?;
    for d in ["/proc", "/sys", "/dev", "/run"] {
        sys_mount(d, &format!("{SYSROOT}{d}"), "", libc::MS_MOVE, "")?;
    }
    let cd = |p: &str| {
        if unsafe { libc::chdir(cstr(p).as_ptr()) } == 0 {
            Ok(())
        } else {
            Err(format!("chdir {p}: {}", std::io::Error::last_os_error()))
        }
    };
    cd(SYSROOT)?;
    sys_mount(".", "/", "", libc::MS_MOVE, "")?;
    if unsafe { libc::chroot(cstr(".").as_ptr()) } != 0 {
        return Err(format!("chroot: {}", std::io::Error::last_os_error()));
    }
    cd("/")?;
    // The root is read-only now; /tmp needs its own tmpfs.
    let nsd = libc::MS_NOSUID | libc::MS_NODEV;
    if let Err(e) = sys_mount("tmpfs", "/tmp", "tmpfs", nsd, "mode=1777,size=64m") {
        log(&format!("{e}"));
    }
    Ok(())
}

/// Open the slot the kernel command line names, verify it, and switch root into it.
/// No slot on the command line (direct boot without a disk): stay on the initramfs.
fn boot_slot() {
    let cmdline = std::fs::read_to_string("/proc/cmdline").unwrap_or_default();
    match verity::from_cmdline(&cmdline) {
        None => log("no allternit.slot on the kernel command line: running from the initramfs"),
        Some(Err(e)) => slot_fail(&e.slot, &e.reason),
        Some(Ok(v)) => {
            let slot = v.slot.name();
            let node = open_verity(&v).unwrap_or_else(|e| slot_fail(slot, &e));
            println!("verity: ok slot={slot}");
            let _ = std::io::stdout().flush();
            match switch_root(&node) {
                Ok(()) => log(&format!("root is slot {slot} ({node}, read-only)")),
                Err(e) => {
                    log(&format!("slot {slot}: switch root failed ({e}), powering off"));
                    halt()
                }
            }
        }
    }
}

pub fn run() {
    mount_basics();
    console_setup();
    boot_slot();
    let cfg = match std::fs::read_to_string(CONFIG_PATH) {
        Ok(t) => match config::parse(&t) {
            Ok(c) => c,
            Err(e) => {
                log(&format!("{CONFIG_PATH} invalid ({e}), using built-in table"));
                config::default_config()
            }
        },
        Err(_) => config::default_config(),
    };
    set_hostname(&cfg.hostname);
    let state_mounted = mount_state(&cfg);
    prepare_run_dirs(&cfg);
    loopback_up();

    unsafe {
        libc::signal(libc::SIGTERM, on_signal as usize);
        libc::signal(libc::SIGINT, on_signal as usize);
    }

    let order = config::order_services(&cfg.services).expect("validated at parse");
    let mut running: Vec<Running> = Vec::new();
    for &i in &order {
        let svc = &cfg.services[i];
        match spawn(svc) {
            Ok(pid) => {
                log(&format!("started {} (pid {pid}, uid {})", svc.name, svc.uid));
                running.push(Running { idx: i, pid, started: Instant::now() });
            }
            Err(e) => log(&format!("failed to start {}: {e}", svc.name)),
        }
        if svc.name == "authority" && !wait_for_socket(AUTH_SOCK, 5) {
            log("authority socket did not appear");
        }
    }

    if kernel_check() {
        log("kernel-check ok");
        println!("kernel-check ok");
        println!("{READY_LINE}");
        let _ = std::io::stdout().flush();
    } else {
        log("kernel-check FAILED, not printing READY");
    }

    let mut respawn_at: Vec<(usize, Instant)> = Vec::new();
    loop {
        if STOP.load(Ordering::SeqCst) {
            shutdown(&running, state_mounted, &cfg);
        }
        for (pid, exit) in reap() {
            let Some(pos) = running.iter().position(|r| r.pid == pid) else { continue };
            let r = running.remove(pos);
            let svc = &cfg.services[r.idx];
            log(&format!("{} exited ({exit:?})", svc.name));
            match config::on_exit(svc, exit) {
                Action::PowerOff => {
                    shutdown(&running, state_mounted, &cfg);
                }
                Action::Restart => {
                    // Back off one second after a quick death to avoid a crash loop.
                    let delay = if r.started.elapsed() < Duration::from_secs(5) { 1 } else { 0 };
                    respawn_at.push((r.idx, Instant::now() + Duration::from_secs(delay)));
                }
                Action::Leave => {}
            }
        }
        let now = Instant::now();
        let due: Vec<usize> = respawn_at.iter().filter(|(_, t)| *t <= now).map(|(i, _)| *i).collect();
        respawn_at.retain(|(_, t)| *t > now);
        for i in due {
            let svc = &cfg.services[i];
            match spawn(svc) {
                Ok(pid) => {
                    log(&format!("restarted {} (pid {pid})", svc.name));
                    running.push(Running { idx: i, pid, started: Instant::now() });
                }
                Err(e) => {
                    log(&format!("restart of {} failed: {e}", svc.name));
                    respawn_at.push((i, Instant::now() + Duration::from_secs(5)));
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
