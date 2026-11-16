//! Registering one sync root per computer.
//!
//! First choice: `StorageProviderSyncRootManager.Register`. It gives the Explorer
//! navigation-pane entry, the location icon and custom state columns (the colour
//! badges). It is documented for packaged apps; unpackaged Win32 apps can call it
//! on current Windows 10/11 builds, but if it refuses (no package identity, older
//! build, policy) the helper falls back to the path the spike proved
//! (`CfRegisterSyncRoot`, unpackaged) plus a per-user navigation-pane entry in the
//! registry (the documented shell-folder method). In the fallback Explorer still
//! shows the cloud status column (cloud / green tick / syncing); only the custom
//! badges need the first path. MSIX packaging would make the first path certain;
//! that needs the code-signing certificate (gate G-W).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use windows::core::{Result, GUID, HSTRING, PCWSTR, PWSTR};
use windows::Foundation::Collections::IVector;
use windows::Storage::Provider::*;
use windows::Storage::StorageFolder;
use windows::Win32::Foundation::{HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
use windows::Win32::Storage::CloudFilters::*;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::conflict::Badge;
use crate::identity::{Identity, RootIdentity};
use crate::paths;

/// cfapi provider ID for Allternit Files.
pub const PROVIDER_GUID: GUID = GUID::from_u128(0xa11e_f11e_0001_4a11_8000_a11e_f11e_0001);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Registration {
    /// StorageProviderSyncRootManager: sidebar, icon, badges.
    StorageProvider { id: String },
    /// CfRegisterSyncRoot + registry navigation-pane entry.
    Cfapi { clsid: String },
}

pub struct Spec<'a> {
    pub path: &'a Path,
    pub computer_id: &'a str,
    pub display_name: &'a str,
    /// Location icon (`allternit.ico`), design pick 5A (the app tile).
    pub icon: Option<&'a Path>,
}

/// Saved in the state dir so uninstall finds every root, whichever path was used.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recorded {
    pub path: PathBuf,
    pub computer_id: String,
    pub registration: Registration,
}

fn records_file(state_dir: &Path) -> PathBuf {
    state_dir.join("roots.json")
}

pub fn recorded(state_dir: &Path) -> Vec<Recorded> {
    std::fs::read(records_file(state_dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_records(state_dir: &Path, list: &[Recorded]) {
    let _ = std::fs::create_dir_all(state_dir);
    let _ = std::fs::write(records_file(state_dir), serde_json::to_vec_pretty(list).unwrap_or_default());
}

/// The current user's SID string (`S-1-5-21-...`), for the sync-root ID.
pub fn user_sid() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u64; (len as usize) / 8 + 1];
        GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr() as *mut c_void), (buf.len() * 8) as u32, &mut len)?;
        let _ = windows::Win32::Foundation::CloseHandle(token);
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s)?;
        let out = s.to_string().unwrap_or_default();
        let _ = LocalFree(HLOCAL(s.0 as *mut c_void));
        Ok(out)
    }
}

fn register_storage_provider(spec: &Spec, id: &str) -> Result<()> {
    let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(spec.path.as_os_str()))?.get()?;
    let info = StorageProviderSyncRootInfo::new()?;
    info.SetId(&HSTRING::from(id))?;
    info.SetPath(&folder)?;
    info.SetDisplayNameResource(&HSTRING::from(format!("Allternit · {}", spec.display_name)))?;
    if let Some(icon) = spec.icon {
        info.SetIconResource(&HSTRING::from(icon.as_os_str()))?;
    }
    info.SetHydrationPolicy(StorageProviderHydrationPolicy::Full)?;
    info.SetHydrationPolicyModifier(StorageProviderHydrationPolicyModifier::None)?;
    info.SetPopulationPolicy(StorageProviderPopulationPolicy::Full)?;
    info.SetInSyncPolicy(StorageProviderInSyncPolicy::Default)?;
    info.SetHardlinkPolicy(StorageProviderHardlinkPolicy::None)?;
    info.SetVersion(&HSTRING::from(crate::HELPER_VERSION))?;
    info.SetShowSiblingsAsGroup(false)?;
    info.SetProviderId(PROVIDER_GUID)?;
    // The colour badges (design pick 6A) are custom state columns.
    let defs: IVector<StorageProviderItemPropertyDefinition> = info.StorageProviderItemPropertyDefinitions()?;
    for b in Badge::ALL {
        let d = StorageProviderItemPropertyDefinition::new()?;
        d.SetId(b.id())?;
        d.SetDisplayNameResource(&HSTRING::from(b.label()))?;
        defs.Append(&d)?;
    }
    StorageProviderSyncRootManager::Register(&info)
}

fn register_cfapi(spec: &Spec) -> Result<()> {
    let name = HSTRING::from(format!("Allternit · {}", spec.display_name));
    let version = HSTRING::from(crate::HELPER_VERSION);
    let root_identity = RootIdentity { v: 1, computer_id: spec.computer_id.to_string() }.encode();
    let top = Identity::top().encode();
    let reg = CF_SYNC_REGISTRATION {
        StructSize: std::mem::size_of::<CF_SYNC_REGISTRATION>() as u32,
        ProviderName: PCWSTR(name.as_ptr()),
        ProviderVersion: PCWSTR(version.as_ptr()),
        SyncRootIdentity: root_identity.as_ptr() as *const c_void,
        SyncRootIdentityLength: root_identity.len() as u32,
        FileIdentity: top.as_ptr() as *const c_void,
        FileIdentityLength: top.len() as u32,
        ProviderId: PROVIDER_GUID,
    };
    let pol = CF_SYNC_POLICIES {
        StructSize: std::mem::size_of::<CF_SYNC_POLICIES>() as u32,
        Hydration: CF_HYDRATION_POLICY { Primary: CF_HYDRATION_POLICY_FULL, Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE },
        Population: CF_POPULATION_POLICY { Primary: CF_POPULATION_POLICY_FULL, Modifier: CF_POPULATION_POLICY_MODIFIER_NONE },
        InSync: CF_INSYNC_POLICY_NONE,
        HardLink: CF_HARDLINK_POLICY_NONE,
        PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
    };
    unsafe { CfRegisterSyncRoot(&HSTRING::from(spec.path.as_os_str()), &reg, &pol, CF_REGISTER_FLAG_NONE) }
}

fn reg(args: &[&str]) -> bool {
    Command::new("reg.exe").args(args).output().map(|o| o.status.success()).unwrap_or(false)
}

/// Per-user shell folder that pins the sync root in Explorer's navigation pane.
fn add_nav_pane(clsid: &str, display: &str, path: &Path, icon: Option<&Path>) {
    let k = format!(r"HKCU\Software\Classes\CLSID\{clsid}");
    let p = path.to_string_lossy();
    reg(&["add", &k, "/ve", "/t", "REG_SZ", "/d", display, "/f"]);
    reg(&["add", &k, "/v", "System.IsPinnedToNameSpaceTree", "/t", "REG_DWORD", "/d", "1", "/f"]);
    reg(&["add", &k, "/v", "SortOrderIndex", "/t", "REG_DWORD", "/d", "66", "/f"]);
    if let Some(i) = icon {
        reg(&["add", &format!(r"{k}\DefaultIcon"), "/ve", "/t", "REG_EXPAND_SZ", "/d", &i.to_string_lossy(), "/f"]);
    }
    reg(&["add", &format!(r"{k}\InProcServer32"), "/ve", "/t", "REG_EXPAND_SZ", "/d", r"%SystemRoot%\system32\shell32.dll", "/f"]);
    reg(&["add", &format!(r"{k}\Instance"), "/v", "CLSID", "/t", "REG_SZ", "/d", "{0E5AAE11-A475-4c5b-AB00-C66DE400274E}", "/f"]);
    reg(&["add", &format!(r"{k}\Instance\InitPropertyBag"), "/v", "Attributes", "/t", "REG_DWORD", "/d", "17", "/f"]);
    reg(&["add", &format!(r"{k}\Instance\InitPropertyBag"), "/v", "TargetFolderPath", "/t", "REG_EXPAND_SZ", "/d", &p, "/f"]);
    reg(&["add", &format!(r"{k}\ShellFolder"), "/v", "FolderValueFlags", "/t", "REG_DWORD", "/d", "40", "/f"]);
    reg(&["add", &format!(r"{k}\ShellFolder"), "/v", "Attributes", "/t", "REG_DWORD", "/d", "4034920525", "/f"]);
    let ns = format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Desktop\NameSpace\{clsid}");
    reg(&["add", &ns, "/ve", "/t", "REG_SZ", "/d", display, "/f"]);
    let hide = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel";
    reg(&["add", hide, "/v", clsid, "/t", "REG_DWORD", "/d", "1", "/f"]);
}

fn remove_nav_pane(clsid: &str) {
    reg(&["delete", &format!(r"HKCU\Software\Classes\CLSID\{clsid}"), "/f"]);
    reg(&["delete", &format!(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Desktop\NameSpace\{clsid}"), "/f"]);
    reg(&["delete", r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel", "/v", clsid, "/f"]);
}

/// Registers the sync root for one computer (creating its folder) and records it.
pub fn register(spec: &Spec, state_dir: &Path) -> Result<Registration> {
    std::fs::create_dir_all(spec.path).map_err(|e| windows::core::Error::new(windows::core::HRESULT(0x80004005u32 as i32), e.to_string()))?;
    let registration = match user_sid().and_then(|sid| {
        let id = paths::sync_root_id(&sid, spec.computer_id);
        register_storage_provider(spec, &id).map(|_| id)
    }) {
        Ok(id) => Registration::StorageProvider { id },
        Err(e) => {
            crate::emit(&serde_json::json!({"type": "log", "level": "warn",
                "message": format!("StorageProviderSyncRootManager refused ({e}); using CfRegisterSyncRoot")}));
            register_cfapi(spec)?;
            let clsid = paths::nav_pane_clsid(spec.computer_id);
            add_nav_pane(&clsid, &format!("Allternit · {}", spec.display_name), spec.path, spec.icon);
            Registration::Cfapi { clsid }
        }
    };
    let mut list = recorded(state_dir);
    list.retain(|r| r.path != spec.path);
    list.push(Recorded { path: spec.path.to_path_buf(), computer_id: spec.computer_id.to_string(), registration: registration.clone() });
    save_records(state_dir, &list);
    Ok(registration)
}

/// True when cfapi knows `path` as a sync root.
pub fn is_registered(path: &Path) -> bool {
    let mut buf = vec![0u64; 512];
    unsafe {
        CfGetSyncRootInfoByPath(
            &HSTRING::from(path.as_os_str()),
            CF_SYNC_ROOT_INFO_BASIC,
            buf.as_mut_ptr() as *mut c_void,
            (buf.len() * 8) as u32,
            None,
        )
        .is_ok()
    }
}

fn unregister_one(r: &Recorded) -> Result<()> {
    match &r.registration {
        Registration::StorageProvider { id } => StorageProviderSyncRootManager::Unregister(&HSTRING::from(id.as_str()))?,
        Registration::Cfapi { clsid } => {
            remove_nav_pane(clsid);
            unsafe { CfUnregisterSyncRoot(&HSTRING::from(r.path.as_os_str()))? };
        }
    }
    Ok(())
}

/// Unregisters one computer's root. Placeholders that were never opened are
/// dropped by Windows; files that were opened stay on disk as normal files.
pub fn unregister(path: &Path, state_dir: &Path) -> Result<()> {
    let mut list = recorded(state_dir);
    let mut result = Ok(());
    if let Some(r) = list.iter().find(|r| r.path == path) {
        result = unregister_one(r);
    } else if is_registered(path) {
        result = unsafe { CfUnregisterSyncRoot(&HSTRING::from(path.as_os_str())) };
    }
    list.retain(|r| r.path != path);
    save_records(state_dir, &list);
    result
}

/// Uninstall: every Allternit sync root of this user, recorded or not.
pub fn unregister_all(state_dir: &Path) -> Vec<String> {
    let mut done = Vec::new();
    for r in recorded(state_dir) {
        match unregister_one(&r) {
            Ok(()) => done.push(r.path.display().to_string()),
            Err(e) => done.push(format!("{} (failed: {e})", r.path.display())),
        }
    }
    save_records(state_dir, &[]);
    // Roots registered by an earlier install whose record is gone.
    if let Ok(roots) = StorageProviderSyncRootManager::GetCurrentSyncRoots() {
        let n = roots.Size().unwrap_or(0);
        for i in 0..n {
            let Ok(info) = roots.GetAt(i) else { continue };
            let Ok(id) = info.Id() else { continue };
            let id = id.to_string();
            if id.starts_with(&format!("{}!", paths::PROVIDER_ID_PREFIX)) && StorageProviderSyncRootManager::Unregister(&HSTRING::from(id.as_str())).is_ok() {
                done.push(id);
            }
        }
    }
    done
}
