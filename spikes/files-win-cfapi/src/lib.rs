//! Spike: a Rust helper using the Windows Cloud Files API (cfapi) as a sync root.
//! Raw `windows` crate, no wrapper, so each cfapi call in the test log is the real one.
#![cfg(windows)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::core::{HRESULT, PCWSTR, HSTRING};
use windows::Win32::Foundation::NTSTATUS;
use windows::Win32::Storage::CloudFilters::*;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO};

/// Fake server: relative path (using `\`) -> file bytes. Directories are implied.
pub static SERVER: OnceLock<HashMap<String, Vec<u8>>> = OnceLock::new();

pub fn fake_tree() -> HashMap<String, Vec<u8>> {
    let mut m = HashMap::new();
    m.insert("hello.txt".into(), b"hello from the allternit fake server\n".to_vec());
    // > 4096 and not 4096-aligned, exercises the EOF-aligned transfer rule.
    m.insert("docs\\big.bin".into(), (0..10_000u32).map(|i| (i % 251) as u8).collect());
    m.insert("docs\\nested\\note.md".into(), b"# nested\n".to_vec());
    m
}

fn log(s: &str) { println!("[cfapi-spike] {s}"); }

fn basic(dir: bool) -> FILE_BASIC_INFO {
    let t = 133_500_000_000_000_000i64;
    FILE_BASIC_INFO {
        CreationTime: t, LastAccessTime: t, LastWriteTime: t, ChangeTime: t,
        FileAttributes: if dir { FILE_ATTRIBUTE_DIRECTORY.0 } else { FILE_ATTRIBUTE_NORMAL.0 },
    }
}

pub fn register(root: &Path) -> windows::core::Result<()> {
    std::fs::create_dir_all(root).unwrap();
    let name = HSTRING::from("AllternitFilesSpike");
    let version = HSTRING::from("0.0.1");
    let identity = b"allternit-spike-root";
    let reg = CF_SYNC_REGISTRATION {
        StructSize: std::mem::size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: PCWSTR(name.as_ptr()),
        ProviderVersion: PCWSTR(version.as_ptr()),
        SyncRootIdentity: identity.as_ptr() as *const c_void,
        SyncRootIdentityLength: identity.len() as u32,
        FileIdentity: std::ptr::null(),
        FileIdentityLength: 0,
        ProviderId: windows::core::GUID::from_u128(0xa11e_0001_0000_4000_8000_0000_0000_0001),
    };
    let pol = CF_SYNC_POLICIES {
        StructSize: std::mem::size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY { Primary: CF_HYDRATION_POLICY_PRIMARY(2), Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE },
        Population: CF_POPULATION_POLICY { Primary: CF_POPULATION_POLICY_PRIMARY(2), Modifier: CF_POPULATION_POLICY_MODIFIER_NONE },
        InSync: CF_INSYNC_POLICY_NONE,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    unsafe { CfRegisterSyncRoot(&HSTRING::from(root.as_os_str()), &reg, &pol, CF_REGISTER_FLAG_NONE) }
}

/// Creates one directory level of placeholders under `base` (relative names), returns processed count.
fn create_level(base: &Path, entries: &[(String, bool, i64, Vec<u8>)]) -> windows::core::Result<u32> {
    let names: Vec<HSTRING> = entries.iter().map(|e| HSTRING::from(e.0.as_str())).collect();
    let mut infos: Vec<CF_PLACEHOLDER_CREATE_INFO> = entries.iter().enumerate().map(|(i, e)| {
        let mut info: CF_PLACEHOLDER_CREATE_INFO = unsafe { std::mem::zeroed() };
        info.RelativeFileName = PCWSTR(names[i].as_ptr());
        info.FsMetadata = CF_FS_METADATA { BasicInfo: basic(e.1), FileSize: e.2 };
        info.FileIdentity = e.3.as_ptr() as *const c_void;
        info.FileIdentityLength = e.3.len() as u32;
        info.Flags = if e.1 { CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC | CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION }
                     else { CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC };
        info
    }).collect();
    let mut done = 0u32;
    unsafe { CfCreatePlaceholders(&HSTRING::from(base.as_os_str()), &mut infos, CF_CREATE_FLAG_NONE, Some(&mut done))?; }
    for (i, inf) in infos.iter().enumerate() {
        if inf.Result.is_err() { log(&format!("placeholder {} result {:?}", entries[i].0, inf.Result)); }
    }
    Ok(done)
}

/// Creates placeholders for the whole fake tree. Returns total created.
pub fn create_tree(root: &Path) -> windows::core::Result<u32> {
    let srv = SERVER.get().unwrap();
    let mut total = 0;
    // top-level: hello.txt + docs/
    let size = |k: &str| srv[k].len() as i64;
    total += create_level(root, &[
        ("hello.txt".into(), false, size("hello.txt"), b"hello.txt".to_vec()),
        ("docs".into(), true, 0, b"docs".to_vec()),
    ])?;
    total += create_level(&root.join("docs"), &[
        ("big.bin".into(), false, size("docs\\big.bin"), b"docs\\big.bin".to_vec()),
        ("nested".into(), true, 0, b"docs\\nested".to_vec()),
    ])?;
    total += create_level(&root.join("docs").join("nested"), &[
        ("note.md".into(), false, size("docs\\nested\\note.md"), b"docs\\nested\\note.md".to_vec()),
    ])?;
    Ok(total)
}

unsafe extern "system" fn on_fetch_data(info: *const CF_CALLBACK_INFO, params: *const CF_CALLBACK_PARAMETERS) {
    let info = &*info;
    let id = std::slice::from_raw_parts(info.FileIdentity as *const u8, info.FileIdentityLength as usize);
    let key = String::from_utf8_lossy(id).to_string();
    let fd = (*params).Anonymous.FetchData;
    log(&format!("FETCH_DATA callback for {key} required_offset={} required_length={}",
        fd.RequiredFileOffset, fd.RequiredLength));
    let data = match SERVER.get().and_then(|s| s.get(&key)) { Some(d) => d, None => return };
    // Transfer the whole file in one go (offset 0 .. EOF is always a legal, aligned-at-EOF range).
    let mut op: CF_OPERATION_PARAMETERS = std::mem::zeroed();
    op.ParamSize = std::mem::size_of::<CF_OPERATION_PARAMETERS>() as u32;
    op.Anonymous.TransferData.Flags = CF_OPERATION_TRANSFER_DATA_FLAG_NONE;
    op.Anonymous.TransferData.CompletionStatus = NTSTATUS(0);
    op.Anonymous.TransferData.Buffer = data.as_ptr() as *const c_void;
    op.Anonymous.TransferData.Offset = 0;
    op.Anonymous.TransferData.Length = data.len() as i64;
    let oi = CF_OPERATION_INFO {
        StructSize: std::mem::size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_DATA,
        ConnectionKey: info.ConnectionKey,
        TransferKey: info.TransferKey,
        CorrelationVector: std::ptr::null(),
        SyncStatus: std::ptr::null(),
        RequestKey: info.RequestKey,
    };
    match CfExecute(&oi, &mut op) {
        Ok(()) => log(&format!("CfExecute TRANSFER_DATA ok for {key} ({} bytes)", data.len())),
        Err(e) => log(&format!("CfExecute TRANSFER_DATA FAILED for {key}: {e}")),
    }
}

pub fn connect(root: &Path) -> windows::core::Result<CF_CONNECTION_KEY> {
    // Table must outlive the connection: leak it (spike).
    let table: &'static [CF_CALLBACK_REGISTRATION] = Box::leak(Box::new([
        CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_FETCH_DATA, Callback: Some(on_fetch_data) },
        CF_CALLBACK_REGISTRATION { Type: CF_CALLBACK_TYPE_NONE, Callback: None },
    ]));
    unsafe { CfConnectSyncRoot(&HSTRING::from(root.as_os_str()), table.as_ptr(), None, CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO | CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH) }
}

pub fn disconnect_and_unregister(key: CF_CONNECTION_KEY, root: &Path) -> windows::core::Result<()> {
    unsafe {
        CfDisconnectSyncRoot(key)?;
        CfUnregisterSyncRoot(&HSTRING::from(root.as_os_str()))
    }
}

pub fn temp_root() -> PathBuf {
    std::env::temp_dir().join(format!("allternit-cfapi-spike-{}", std::process::id()))
}

#[allow(dead_code)]
fn _unused(_: HRESULT) {}
