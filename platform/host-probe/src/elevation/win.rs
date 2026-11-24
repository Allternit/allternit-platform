//! Windows transport between the probe and the SYSTEM helper task.
//!
//! A named pipe instead of a loopback port: no other local process can take
//! the channel first and read what the probe sends.
//!
//! - Server ([`serve_pipe`], the SYSTEM task): the first instance is created
//!   with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so if anything already holds the
//!   name the helper refuses to start instead of sharing it. The security
//!   descriptor ([`super::windows_pipe_sddl`]) makes SYSTEM the owner and
//!   lets in only SYSTEM and the installing user's SID, without the right to
//!   add pipe instances. Remote clients are rejected. One instance is always
//!   kept listening, so the name never lapses while the helper runs.
//! - Client ([`connect_helper_pipe`], the probe): before sending anything it
//!   checks that the pipe object is owned by SYSTEM (a non-admin process
//!   cannot create a SYSTEM-owned pipe) and that the server process
//!   (`GetNamedPipeServerProcessId`) runs the installed helper binary. It
//!   opens the pipe at identification level only, so the server cannot act
//!   as the user.

use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetLastError, LocalFree, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, IsWellKnownSid, TokenUser, WinLocalSystemSid, OWNER_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_GENERIC_READ,
    FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, GetNamedPipeServerProcessId,
    WaitNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::cmd::RunError;

const PIPE_BUFFER: u32 = 64 * 1024;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Memory the system allocated with LocalAlloc (SDDL, SID strings, security info).
struct LocalMem(*mut c_void);

impl Drop for LocalMem {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

fn raw(h: &OwnedHandle) -> HANDLE {
    h.as_raw_handle() as HANDLE
}

// ---------------------------------------------------------------------------
// The current user's SID (read by the unelevated runtime at setup)
// ---------------------------------------------------------------------------

pub(super) fn current_user_sid() -> Option<String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let token = OwnedHandle::from_raw_handle(token as _);
        let mut len = 0u32;
        GetTokenInformation(raw(&token), TokenUser, null_mut(), 0, &mut len);
        if len == 0 {
            return None;
        }
        // u64 storage keeps TOKEN_USER aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if GetTokenInformation(
            raw(&token),
            TokenUser,
            buf.as_mut_ptr().cast(),
            len,
            &mut len,
        ) == 0
        {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_string(user.User.Sid)
    }
}

unsafe fn sid_string(sid: PSID) -> Option<String> {
    let mut out: *mut u16 = null_mut();
    if ConvertSidToStringSidW(sid, &mut out) == 0 || out.is_null() {
        return None;
    }
    let _free = LocalMem(out.cast());
    let mut n = 0;
    while *out.add(n) != 0 {
        n += 1;
    }
    String::from_utf16(std::slice::from_raw_parts(out, n)).ok()
}

// ---------------------------------------------------------------------------
// Server: the SYSTEM task
// ---------------------------------------------------------------------------

fn create_instance(name: &[u16], sa: &SECURITY_ATTRIBUTES, first: bool) -> io::Result<OwnedHandle> {
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER,
            PIPE_BUFFER,
            0,
            sa,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(h as _) })
}

/// `allternit-host-helper serve --sid <SID>`: answer requests on the pipe forever.
pub(super) fn serve_pipe(sid: &str) -> io::Result<()> {
    let sddl = super::windows_pipe_sddl(sid)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid user SID"))?;
    let sddl_w = wide(&sddl);
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl_w.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let _sd = LocalMem(sd);
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let name = wide(super::WINDOWS_PIPE);
    let mut listening = create_instance(&name, &sa, true).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "{}: {e}. Another process may already hold this pipe name; the helper will not share it.",
                super::WINDOWS_PIPE
            ),
        )
    })?;
    loop {
        if unsafe { ConnectNamedPipe(raw(&listening), null_mut()) } == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_PIPE_CONNECTED {
                // The client left before the connect finished; reuse the instance.
                unsafe {
                    DisconnectNamedPipe(raw(&listening));
                }
                continue;
            }
        }
        // Make the next instance before handing this one off, so one is
        // always listening and nobody else can create the name.
        let next = loop {
            match create_instance(&name, &sa, false) {
                Ok(h) => break h,
                Err(e) => {
                    eprintln!("serve: next pipe instance: {e}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        };
        let conn = std::mem::replace(&mut listening, next);
        std::thread::spawn(move || serve_conn(conn));
    }
}

fn serve_conn(conn: OwnedHandle) {
    let file = File::from(conn);
    if let Ok(reader) = file.try_clone() {
        let _ = crate::helper::serve_one(
            &crate::actions::SystemExecutor,
            io::BufReader::new(reader),
            &file,
        );
    }
    let h = file.as_raw_handle() as HANDLE;
    unsafe {
        // Let the client read the answer before the instance goes away.
        FlushFileBuffers(h);
        DisconnectNamedPipe(h);
    }
}

// ---------------------------------------------------------------------------
// Client: the probe
// ---------------------------------------------------------------------------

/// Open the helper's pipe and check who serves it. Nothing is sent until the
/// check passes.
pub(super) fn connect_helper_pipe(wait: Duration) -> Result<File, RunError> {
    let name = wide(super::WINDOWS_PIPE);
    let deadline = Instant::now() + wait;
    let pipe = loop {
        // Read + write data only: no FILE_APPEND_DATA (= create pipe instance),
        // which the pipe's DACL does not grant the user.
        let h = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_GENERIC_READ | FILE_WRITE_DATA,
                0,
                null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                null_mut(),
            )
        };
        if h != INVALID_HANDLE_VALUE {
            break unsafe { OwnedHandle::from_raw_handle(h as _) };
        }
        match unsafe { GetLastError() } {
            ERROR_PIPE_BUSY => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(RunError::Timeout);
                }
                let ms = left.as_millis().clamp(1, u128::from(u32::MAX)) as u32;
                unsafe {
                    WaitNamedPipeW(name.as_ptr(), ms);
                }
            }
            ERROR_FILE_NOT_FOUND => return Err(RunError::NotFound),
            ERROR_ACCESS_DENIED => {
                return Err(RunError::Io(
                    "the admin helper's pipe does not let this account in (was it set up for another user?)"
                        .into(),
                ))
            }
            err => return Err(RunError::Io(io::Error::from_raw_os_error(err as i32).to_string())),
        }
    };
    verify_server(&pipe).map_err(|why| {
        RunError::Io(format!("refused to talk to {}: {why}", super::WINDOWS_PIPE))
    })?;
    Ok(File::from(pipe))
}

fn verify_server(pipe: &OwnedHandle) -> Result<(), String> {
    let h = raw(pipe);
    // 1. The pipe object must be owned by SYSTEM. Only SYSTEM / an admin
    //    can create a pipe with that owner, so a squatter cannot pass this.
    unsafe {
        let mut owner: PSID = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let rc = GetSecurityInfo(
            h,
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        );
        let _sd = LocalMem(sd);
        if rc != 0 {
            return Err(format!("cannot read the pipe's owner (error {rc})"));
        }
        if owner.is_null() || IsWellKnownSid(owner, WinLocalSystemSid) == 0 {
            let who = if owner.is_null() {
                None
            } else {
                sid_string(owner)
            };
            return Err(format!(
                "the pipe is owned by {}, not SYSTEM",
                who.unwrap_or_else(|| "an unknown account".into())
            ));
        }
    }
    // 2. The server process must be the installed helper binary.
    let mut pid = 0u32;
    if unsafe { GetNamedPipeServerProcessId(h, &mut pid) } == 0 || pid == 0 {
        return Err("cannot read the pipe server's process id".into());
    }
    let expected = super::windows_helper();
    match process_image(pid) {
        Ok(path) if same_path(&path, &expected) => Ok(()),
        Ok(path) => Err(format!(
            "the pipe server (pid {pid}) runs {path}, not {expected}"
        )),
        // A SYSTEM process may not let a standard user open it even for
        // limited queries. That denial itself means the server is not the
        // user's own process, and check 1 already proved it is SYSTEM-owned.
        Err(ERROR_ACCESS_DENIED) => Ok(()),
        Err(code) => Err(format!(
            "cannot read the pipe server's image (pid {pid}, error {code})"
        )),
    }
}

fn process_image(pid: u32) -> Result<String, u32> {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return Err(GetLastError());
        }
        let p = OwnedHandle::from_raw_handle(p as _);
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        if QueryFullProcessImageNameW(raw(&p), PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) == 0
        {
            return Err(GetLastError());
        }
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Windows paths compare without case; `\\?\` prefixes and `/` are ignored.
fn same_path(a: &str, b: &str) -> bool {
    fn norm(p: &str) -> String {
        p.trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_lowercase()
    }
    norm(a) == norm(b)
}
