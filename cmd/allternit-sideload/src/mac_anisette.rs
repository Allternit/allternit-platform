//! Anisette headers from this Mac's own AuthKit, the way Xcode and AltServer
//! sign in: `AOSUtilities retrieveOTPHeadersForDSID:` gives the one-time
//! password and machine id, `AKDevice currentDevice` the device id and client
//! description. No server and no stored machine state.
//!
//! Preferred over the remote anisette v3 servers on macOS: on 2026-10-18 Apple
//! answered -80045 ("This action cannot be completed at this time") to every
//! emulated-device server we tried (anisette.allternit.com, ani.sidestore.app)
//! and accepted headers from a real Mac. `MacAnisetteProvider::probe` returns
//! None when AuthKit gives nothing, and the signer falls back to the server.

use std::sync::Arc;

use isideload::anisette::{AnisetteClientInfo, AnisetteData, AnisetteDataResponse, AnisetteProvider};
use isideload::auth::grandslam::GrandSlam;
use rootcause::prelude::*;

/// What AltServer sends; AuthKit doesn't hand out a routing info here.
const ROUTING_INFO: &str = "17106176";

#[derive(Debug, Clone)]
struct MacHeaders {
    one_time_password: String,
    machine_id: String,
    device_id: String,
    local_user_id: String,
    /// e.g. `<MacBookPro18,3> <macOS;14.6.1;23G93> <com.apple.AuthKit/1 (com.apple.akd/1.0)>`
    client_info: String,
}

pub struct MacAnisetteProvider {
    client_info: AnisetteClientInfo,
}

impl MacAnisetteProvider {
    /// Some(provider) when this Mac's AuthKit hands out headers.
    pub fn probe() -> Option<Self> {
        let h = read_headers()?;
        Some(MacAnisetteProvider {
            client_info: AnisetteClientInfo { client_info: h.client_info, user_agent: user_agent() },
        })
    }
}

#[async_trait::async_trait]
impl AnisetteProvider for MacAnisetteProvider {
    async fn get_anisette_data(&self) -> Result<AnisetteDataResponse, Report> {
        let h = read_headers().ok_or_else(|| report!("This Mac's AuthKit returned no anisette headers"))?;
        Ok(AnisetteDataResponse::Data(AnisetteData::new(
            h.machine_id,
            h.one_time_password,
            ROUTING_INFO.to_string(),
            h.client_info,
            h.device_id,
            h.local_user_id,
        )))
    }

    async fn get_client_info(&self) -> Result<AnisetteClientInfo, Report> {
        Ok(self.client_info.clone())
    }

    /// AuthKit keeps its own provisioning; nothing to do.
    async fn provision(&mut self, _gs: &Arc<GrandSlam>, _force_recheck: bool) -> Result<(), Report> {
        Ok(())
    }
}

/// `akd/1.0 CFNetwork/<version> Darwin/<kernel>`, as the system's akd sends it.
fn user_agent() -> String {
    let cfnetwork = plist::Value::from_file("/System/Library/Frameworks/CFNetwork.framework/Resources/Info.plist")
        .ok()
        .and_then(|v| v.as_dictionary()?.get("CFBundleVersion")?.as_string().map(str::to_string))
        .unwrap_or_else(|| "1.0".to_string());
    format!("akd/1.0 CFNetwork/{cfnetwork} Darwin/{}", ffi::darwin_release().unwrap_or_default())
}

#[cfg(target_os = "macos")]
fn read_headers() -> Option<MacHeaders> {
    ffi::read_headers()
}

#[cfg(not(target_os = "macos"))]
fn read_headers() -> Option<MacHeaders> {
    None
}

#[cfg(target_os = "macos")]
mod ffi {
    //! Plain Objective-C runtime calls; the frameworks are private, so they're
    //! loaded with dlopen and every class/selector is looked up by name.
    use std::ffi::{c_char, c_int, c_void, CStr};

    use super::MacHeaders;

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc")]
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
    #[link(name = "Foundation", kind = "framework")]
    extern "C" {}
    extern "C" {
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        fn sysctlbyname(name: *const c_char, old: *mut c_void, oldlen: *mut usize, new: *mut c_void, newlen: usize) -> c_int;
    }
    const RTLD_NOW: c_int = 2;

    unsafe fn send0(obj: Id, sel: &CStr) -> Id {
        if obj.is_null() {
            return obj;
        }
        let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel_registerName(sel.as_ptr()))
    }

    unsafe fn send1(obj: Id, sel: &CStr, arg: Id) -> Id {
        if obj.is_null() {
            return obj;
        }
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(obj, sel_registerName(sel.as_ptr()), arg)
    }

    unsafe fn ns_string(s: &CStr) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        f(objc_getClass(c"NSString".as_ptr()), sel_registerName(c"stringWithUTF8String:".as_ptr()), s.as_ptr())
    }

    /// `[obj description]` as a Rust string; None for nil.
    unsafe fn describe(obj: Id) -> Option<String> {
        let s = send0(send0(obj, c"description"), c"UTF8String") as *const c_char;
        (!s.is_null()).then(|| CStr::from_ptr(s).to_string_lossy().into_owned()).filter(|s| !s.is_empty())
    }

    pub(super) fn read_headers() -> Option<MacHeaders> {
        unsafe {
            dlopen(c"/System/Library/PrivateFrameworks/AOSKit.framework/AOSKit".as_ptr(), RTLD_NOW);
            dlopen(c"/System/Library/PrivateFrameworks/AuthKit.framework/AuthKit".as_ptr(), RTLD_NOW);
            let pool = objc_autoreleasePoolPush();
            let out = (|| {
                let utils = objc_getClass(c"AOSUtilities".as_ptr());
                let device = send0(objc_getClass(c"AKDevice".as_ptr()), c"currentDevice");
                if utils.is_null() || device.is_null() {
                    return None;
                }
                // DSID -2 is the "no account" request Xcode makes before login.
                let otp = send1(utils, c"retrieveOTPHeadersForDSID:", ns_string(c"-2"));
                let header = |k: &CStr| describe(send1(otp, c"objectForKey:", ns_string(k)));
                Some(MacHeaders {
                    one_time_password: header(c"X-Apple-MD")?,
                    machine_id: header(c"X-Apple-MD-M")?,
                    device_id: describe(send0(device, c"uniqueDeviceIdentifier"))?,
                    local_user_id: describe(send0(device, c"localUserUUID")).unwrap_or_default(),
                    // A process without a bundle id shows up as "(null)"; say akd, like the system does.
                    client_info: describe(send0(device, c"serverFriendlyDescription"))?.replace("((null)/1.0)", "(com.apple.akd/1.0)"),
                })
            })();
            objc_autoreleasePoolPop(pool);
            out
        }
    }

    pub(super) fn darwin_release() -> Option<String> {
        let mut buf = [0u8; 64];
        let mut len = buf.len();
        let ok = unsafe { sysctlbyname(c"kern.osrelease".as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) } == 0;
        ok.then(|| CStr::from_bytes_until_nul(&buf).ok().map(|s| s.to_string_lossy().into_owned())).flatten()
    }
}

#[cfg(not(target_os = "macos"))]
mod ffi {
    pub(super) fn darwin_release() -> Option<String> {
        None
    }
}
