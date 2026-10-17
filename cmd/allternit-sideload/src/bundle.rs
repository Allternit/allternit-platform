//! The .ipa on disk: unpack, give every bundle its new identity, repack.
//!
//! The XCUITest runner is an app with a nested test bundle,
//! `PlugIns/<Name>.xctest`. isideload 0.4.6 treats every PlugIns/* bundle as an
//! app extension: it requires the plug-in's id to extend the app's id
//! (`<app>.<suffix>`) and registers an App ID + profile for it. The .xctest
//! instead carries the RUNNER's own bundle id (package-phone-link.sh aligns it)
//! and must be signed with the runner's identity and team, with no App ID of
//! its own — exactly what Scripts/sign-ipa.sh does with codesign. If it gets a
//! different identity the runner installs and then crashes at launch.
//!
//! So this module rewrites the ids itself (the .xctest takes the app's new id)
//! and reports only real extensions (.appex) as needing App IDs; signer.rs then
//! drives isideload's lower-level APIs instead of `Sideloader::sign_app`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use plist::Value;

use crate::bundle_ids::rebase_bundle_id;
use crate::errors::{ErrorCode, Failure};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedApp {
    /// <work>/Payload/<Name>.app
    pub app_dir: PathBuf,
    /// The app's bundle id after the rewrite.
    pub bundle_id: String,
    /// CFBundleName (falls back to the .app's file stem).
    pub name: String,
    /// .appex ids that need their own App ID + profile.
    pub extension_ids: Vec<String>,
    /// .xctest bundles that now share `bundle_id` (no App ID of their own).
    #[cfg_attr(not(test), allow(dead_code))]
    pub xctests: Vec<PathBuf>,
}

fn io_fail(what: &str, e: impl std::fmt::Display) -> Failure {
    Failure::new(ErrorCode::SigningFailed, format!("{what}: {e}"))
}

/// Unpack `ipa` into `work` (emptied first) and return Payload/<Name>.app.
pub fn extract_ipa(ipa: &Path, work: &Path) -> Result<PathBuf, Failure> {
    if work.exists() {
        fs::remove_dir_all(work).map_err(|e| io_fail("Couldn't clear the work folder", e))?;
    }
    fs::create_dir_all(work).map_err(|e| io_fail("Couldn't create the work folder", e))?;
    let file = fs::File::open(ipa).map_err(|e| io_fail(&format!("Couldn't open {}", ipa.display()), e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| io_fail(&format!("{} is not an .ipa", ipa.display()), e))?;
    // zip's extract() refuses entries that escape `work` (enclosed_name) and
    // restores unix permissions, which keeps the executables executable.
    archive.extract(work).map_err(|e| io_fail("Couldn't unpack the .ipa", e))?;
    find_app(&work.join("Payload"))
}

pub fn find_app(payload: &Path) -> Result<PathBuf, Failure> {
    let mut apps: Vec<PathBuf> = fs::read_dir(payload)
        .map_err(|e| io_fail("No Payload folder in the .ipa", e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.extension().is_some_and(|x| x == "app"))
        .collect();
    match apps.len() {
        1 => Ok(apps.remove(0)),
        0 => Err(Failure::new(ErrorCode::SigningFailed, "No Payload/*.app in the .ipa.")),
        _ => Err(Failure::new(ErrorCode::SigningFailed, "More than one app in the .ipa.")),
    }
}

fn read_info(bundle: &Path) -> Result<plist::Dictionary, Failure> {
    let path = bundle.join("Info.plist");
    match Value::from_file(&path) {
        Ok(Value::Dictionary(d)) => Ok(d),
        Ok(_) => Err(Failure::new(ErrorCode::SigningFailed, format!("{} is not a dictionary", path.display()))),
        Err(e) => Err(io_fail(&format!("Couldn't read {}", path.display()), e)),
    }
}

fn write_info(bundle: &Path, info: plist::Dictionary) -> Result<(), Failure> {
    let path = bundle.join("Info.plist");
    plist::to_file_binary(&path, &Value::Dictionary(info)).map_err(|e| io_fail(&format!("Couldn't write {}", path.display()), e))
}

fn bundle_id_of(info: &plist::Dictionary, bundle: &Path) -> Result<String, Failure> {
    info.get("CFBundleIdentifier")
        .and_then(Value::as_string)
        .map(str::to_string)
        .ok_or_else(|| Failure::new(ErrorCode::SigningFailed, format!("{} has no CFBundleIdentifier", bundle.display())))
}

fn set_bundle_id(info: &mut plist::Dictionary, id: &str) {
    info.insert("CFBundleIdentifier".into(), Value::String(id.to_string()));
}

fn plugin_dirs(app: &Path) -> Result<Vec<PathBuf>, Failure> {
    let plugins = app.join("PlugIns");
    if !plugins.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&plugins)
        .map_err(|e| io_fail("Couldn't read PlugIns", e))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir() && p.join("Info.plist").is_file())
        .collect();
    dirs.sort();
    Ok(dirs)
}

pub fn is_xctest(path: &Path) -> bool {
    path.extension().is_some_and(|x| x == "xctest")
}

/// Rewrite the app's id from `source_prefix` to `target_prefix`; every
/// PlugIns/*.xctest takes the app's new id; .appex ids follow the app's.
pub fn rewrite_identities(app: &Path, source_prefix: &str, target_prefix: &str) -> Result<PreparedApp, Failure> {
    let mut info = read_info(app)?;
    let old_id = bundle_id_of(&info, app)?;
    let new_id = rebase_bundle_id(&old_id, source_prefix, target_prefix).ok_or_else(|| {
        Failure::new(ErrorCode::SigningFailed, format!("The helper's bundle id {old_id} isn't under {source_prefix}."))
    })?;
    let name = info
        .get("CFBundleName")
        .and_then(Value::as_string)
        .map(str::to_string)
        .unwrap_or_else(|| app.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "App".into()));
    set_bundle_id(&mut info, &new_id);
    write_info(app, info)?;

    let mut extension_ids = Vec::new();
    let mut xctests = Vec::new();
    for plugin in plugin_dirs(app)? {
        let mut pinfo = read_info(&plugin)?;
        if is_xctest(&plugin) {
            // The test bundle runs inside the runner: same id, same team, same
            // profile; it never gets an App ID of its own.
            set_bundle_id(&mut pinfo, &new_id);
            write_info(&plugin, pinfo)?;
            // A profile left over from the build would name another team.
            let stale = plugin.join("embedded.mobileprovision");
            if stale.exists() {
                fs::remove_file(&stale).map_err(|e| io_fail("Couldn't remove a stale profile", e))?;
            }
            xctests.push(plugin);
        } else {
            let pid = bundle_id_of(&pinfo, &plugin)?;
            let new_pid = match pid.strip_prefix(old_id.as_str()).filter(|rest| rest.starts_with('.')) {
                Some(rest) => format!("{new_id}{rest}"),
                None => rebase_bundle_id(&pid, source_prefix, target_prefix).ok_or_else(|| {
                    Failure::new(ErrorCode::SigningFailed, format!("Plug-in {pid} isn't part of {old_id}."))
                })?,
            };
            set_bundle_id(&mut pinfo, &new_pid);
            write_info(&plugin, pinfo)?;
            extension_ids.push(new_pid);
        }
    }
    Ok(PreparedApp { app_dir: app.to_path_buf(), bundle_id: new_id, name, extension_ids, xctests })
}

/// Which of `needed` must be registered, given the team's existing App IDs and
/// Apple's reported allowance. Reuses existing ones (no allowance spent).
pub fn plan_app_ids(needed: &[String], existing: &[String], available: Option<i64>) -> Result<Vec<String>, Failure> {
    let mut missing: Vec<String> = Vec::new();
    for id in needed {
        if !existing.contains(id) && !missing.contains(id) {
            missing.push(id.clone());
        }
    }
    if let Some(available) = available {
        // Apple has reported negative numbers; treat that as "unknown" and try.
        if available >= 0 && missing.len() as i64 > available {
            return Err(Failure::new(
                ErrorCode::AppIdLimit,
                format!(
                    "{} (needs {} new App ID{}, {} left this week)",
                    ErrorCode::AppIdLimit.default_message(),
                    missing.len(),
                    if missing.len() == 1 { "" } else { "s" },
                    available
                ),
            ));
        }
    }
    Ok(missing)
}

/// Zip `<work>/Payload` into `dest` (replaced), keeping permissions and symlinks.
pub fn zip_payload(work: &Path, dest: &Path) -> Result<(), Failure> {
    let tmp = dest.with_extension("ipa.partial");
    let result = (|| -> io::Result<()> {
        let file = fs::File::create(&tmp)?;
        let mut zip = zip::ZipWriter::new(file);
        add_tree(&mut zip, work, &work.join("Payload"))?;
        zip.finish().map_err(io::Error::other)?;
        fs::rename(&tmp, dest)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(io_fail("Couldn't package the signed helper", e));
    }
    Ok(())
}

fn rel_name(root: &Path, path: &Path) -> io::Result<String> {
    let rel = path.strip_prefix(root).map_err(io::Error::other)?;
    Ok(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"))
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn mode_of(meta: &fs::Metadata) -> u32 {
    if meta.is_dir() { 0o755 } else { 0o644 }
}

fn add_tree(zip: &mut zip::ZipWriter<fs::File>, root: &Path, dir: &Path) -> io::Result<()> {
    use zip::write::SimpleFileOptions;
    let meta = fs::symlink_metadata(dir)?;
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    zip.add_directory(format!("{}/", rel_name(root, dir)?), opts.unix_permissions(mode_of(&meta)))
        .map_err(io::Error::other)?;
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let meta = fs::symlink_metadata(&path)?;
        let name = rel_name(root, &path)?;
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            zip.add_symlink(name, target.to_string_lossy(), opts).map_err(io::Error::other)?;
        } else if meta.is_dir() {
            add_tree(zip, root, &path)?;
        } else {
            zip.start_file(name, opts.unix_permissions(mode_of(&meta))).map_err(io::Error::other)?;
            let mut f = fs::File::open(&path)?;
            io::copy(&mut f, zip)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle_ids::{team_prefix, DEFAULT_SOURCE_PREFIX};

    fn write_plist(dir: &Path, id: &str, name: Option<&str>) {
        fs::create_dir_all(dir).unwrap();
        let mut d = plist::Dictionary::new();
        d.insert("CFBundleIdentifier".into(), Value::String(id.into()));
        d.insert("CFBundleExecutable".into(), Value::String("bin".into()));
        if let Some(n) = name {
            d.insert("CFBundleName".into(), Value::String(n.into()));
        }
        plist::to_file_xml(dir.join("Info.plist"), &Value::Dictionary(d)).unwrap();
        fs::write(dir.join("bin"), b"\xcf\xfa\xed\xfe fake mach-o").unwrap();
    }

    fn id_of(dir: &Path) -> String {
        bundle_id_of(&read_info(dir).unwrap(), dir).unwrap()
    }

    /// The runner .ipa layout package-phone-link.sh publishes.
    fn runner_fixture(root: &Path) -> PathBuf {
        let app = root.join("Payload/iCTRLUITests-Runner.app");
        let runner_id = "com.allternit.phonelink.ictrl.uitests.xctrunner";
        write_plist(&app, runner_id, Some("iCTRLUITests-Runner"));
        let xctest = app.join("PlugIns/iCTRLUITests.xctest");
        write_plist(&xctest, runner_id, Some("iCTRLUITests"));
        fs::write(xctest.join("embedded.mobileprovision"), b"stale").unwrap();
        write_plist(&app.join("Frameworks/XCTest.framework"), "com.apple.dt.XCTest", None);
        app
    }

    #[test]
    fn xctest_takes_the_runners_new_identity() {
        let dir = tempfile::tempdir().unwrap();
        let app = runner_fixture(dir.path());
        let target = team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE12345").unwrap();
        let prepared = rewrite_identities(&app, DEFAULT_SOURCE_PREFIX, &target).unwrap();

        let expected = "com.allternit.phonelink.abcde12345.ictrl.uitests.xctrunner";
        assert_eq!(prepared.bundle_id, expected);
        assert_eq!(id_of(&app), expected);
        let xctest = app.join("PlugIns/iCTRLUITests.xctest");
        assert_eq!(id_of(&xctest), expected, "the .xctest must carry the runner's id");
        assert!(prepared.extension_ids.is_empty(), "the .xctest must not get its own App ID");
        assert_eq!(prepared.xctests, vec![xctest.clone()]);
        assert!(!xctest.join("embedded.mobileprovision").exists());
        // Apple's frameworks keep their ids.
        assert_eq!(id_of(&app.join("Frameworks/XCTest.framework")), "com.apple.dt.XCTest");
        assert_eq!(prepared.name, "iCTRLUITests-Runner");
    }

    #[test]
    fn xctest_rewrite_works_even_if_the_build_left_its_own_id() {
        // An unaligned build (xctest = "<runner>.uitests") still ends up on the runner's id.
        let dir = tempfile::tempdir().unwrap();
        let app = runner_fixture(dir.path());
        let xctest = app.join("PlugIns/iCTRLUITests.xctest");
        write_plist(&xctest, "com.allternit.phonelink.ictrl.uitests", None);
        let target = team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE12345").unwrap();
        let prepared = rewrite_identities(&app, DEFAULT_SOURCE_PREFIX, &target).unwrap();
        assert_eq!(id_of(&xctest), prepared.bundle_id);
    }

    #[test]
    fn appex_follows_the_app_and_needs_an_app_id() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Payload/iCTRL.app");
        write_plist(&app, "com.allternit.phonelink.ictrl", Some("iCTRL"));
        write_plist(&app.join("PlugIns/Widget.appex"), "com.allternit.phonelink.ictrl.widget", None);
        let target = team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE12345").unwrap();
        let prepared = rewrite_identities(&app, DEFAULT_SOURCE_PREFIX, &target).unwrap();
        assert_eq!(prepared.bundle_id, "com.allternit.phonelink.abcde12345.ictrl");
        assert_eq!(prepared.extension_ids, vec!["com.allternit.phonelink.abcde12345.ictrl.widget".to_string()]);
    }

    #[test]
    fn rewrite_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let app = runner_fixture(dir.path());
        let target = team_prefix(DEFAULT_SOURCE_PREFIX, "ABCDE12345").unwrap();
        let a = rewrite_identities(&app, DEFAULT_SOURCE_PREFIX, &target).unwrap();
        let b = rewrite_identities(&app, DEFAULT_SOURCE_PREFIX, &target).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn refuses_foreign_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Payload/Other.app");
        write_plist(&app, "com.example.other", None);
        let err = rewrite_identities(&app, DEFAULT_SOURCE_PREFIX, "com.allternit.phonelink.abcde12345").unwrap_err();
        assert_eq!(err.code, ErrorCode::SigningFailed);
    }

    #[test]
    fn plans_app_ids_reusing_existing_ones() {
        let host = "com.allternit.phonelink.abcde12345.ictrl".to_string();
        let runner = "com.allternit.phonelink.abcde12345.ictrl.uitests.xctrunner".to_string();
        let needed = vec![host.clone(), runner.clone()];
        // First run: both new.
        assert_eq!(plan_app_ids(&needed, &[], Some(10)).unwrap(), needed);
        // Re-sign: nothing new, even with zero allowance left.
        assert!(plan_app_ids(&needed, &needed, Some(0)).unwrap().is_empty());
        // One left, one needed.
        assert_eq!(plan_app_ids(&needed, std::slice::from_ref(&host), Some(1)).unwrap(), vec![runner.clone()]);
        // Not enough allowance: typed error before Apple is asked.
        assert_eq!(plan_app_ids(&needed, &[], Some(1)).unwrap_err().code, ErrorCode::AppIdLimit);
        // Unknown or negative allowance: try anyway.
        assert_eq!(plan_app_ids(&needed, &[], None).unwrap().len(), 2);
        assert_eq!(plan_app_ids(&needed, &[], Some(-1)).unwrap().len(), 2);
    }

    #[test]
    fn ipa_round_trip_keeps_layout_and_modes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        let app = runner_fixture(&src);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(app.join("bin"), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let ipa = dir.path().join("runner.ipa");
        zip_payload(&src, &ipa).unwrap();
        assert!(!ipa.with_extension("ipa.partial").exists());

        let work = dir.path().join("work");
        let extracted = extract_ipa(&ipa, &work).unwrap();
        assert_eq!(extracted, work.join("Payload/iCTRLUITests-Runner.app"));
        assert!(extracted.join("PlugIns/iCTRLUITests.xctest/Info.plist").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(extracted.join("bin")).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "executable bit lost: {mode:o}");
        }
    }

    #[test]
    fn extract_rejects_non_ipa() {
        let dir = tempfile::tempdir().unwrap();
        let bogus = dir.path().join("x.ipa");
        fs::write(&bogus, b"not a zip").unwrap();
        assert!(extract_ipa(&bogus, &dir.path().join("w")).is_err());
    }
}
