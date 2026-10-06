//! `gizzi agents pack` / `install <path|github-url>`.
//!
//! A pack is a `.tar.gz` of the team folder (`team.yaml`, `CULTURE.md`, bot
//! persona / skill / `LEARNED.md` files under the team dir; never
//! `snapshots/`) with a `manifest.json` listing every file's sha256.
//!
//! Install verifies every hash, validates `team.yaml`, rejects path traversal,
//! links and device entries, and refuses to replace an existing team unless
//! forced. GitHub installs must be pinned to a 40-hex commit; a branch name is
//! refused. A raw GitHub folder without a manifest is hashed on the way in
//! and the hashes are recorded in `install.json`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::team::{parse_team, sha256_hex, team_dir, valid_slug, TEAMS_DIR, TEAM_FILE};

pub const MANIFEST_FILE: &str = "manifest.json";
pub const INSTALL_RECORD: &str = "install.json";
const MAX_FILES: usize = 2_000;
const MAX_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PackFile {
    pub path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackManifest {
    pub name: String,
    pub version: String,
    pub files: Vec<PackFile>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackResult {
    pub manifest: PackManifest,
    pub archive: PathBuf,
    pub archive_sha256: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub dry_run: bool,
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallReport {
    pub team: String,
    pub dest: PathBuf,
    pub source: String,
    pub manifest_present: bool,
    pub files: Vec<PackFile>,
    pub dry_run: bool,
    /// True when the files were written (false for a dry run).
    pub installed: bool,
    /// True when an existing team was replaced (`force`).
    pub replaced: bool,
}

/// A `https://github.com/<owner>/<repo>/tree/<sha>/<path>` source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubSource {
    pub owner: String,
    pub repo: String,
    pub commit: String,
    pub path: String,
}

impl GithubSource {
    pub fn tarball_url(&self) -> String {
        format!("https://codeload.github.com/{}/{}/tar.gz/{}", self.owner, self.repo, self.commit)
    }
}

/// A relative path made only of normal components, `/`-joined. `None` for
/// absolute paths, `..`, `.`, backslashes, NUL or empty.
pub fn safe_rel(path: &str) -> Option<String> {
    if path.is_empty() || path.contains('\\') || path.contains('\0') || path.starts_with('/') {
        return None;
    }
    let mut parts = vec![];
    for c in Path::new(path).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_str()?.to_string()),
            Component::CurDir if parts.is_empty() => {}
            _ => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn is_excluded(rel: &str) -> bool {
    rel == MANIFEST_FILE
        || rel == INSTALL_RECORD
        || rel == super::snapshot::SNAPSHOTS_DIR
        || rel.starts_with(&format!("{}/", super::snapshot::SNAPSHOTS_DIR))
        || rel.rsplit('/').next() == Some(".DS_Store")
}

/// Files of a folder, sorted by path. Links and special files are refused.
fn read_dir_files(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>, total: &mut u64) -> Result<()> {
        for ent in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let ent = ent?;
            let p = ent.path();
            let rel = p.strip_prefix(base)?.to_string_lossy().replace('\\', "/");
            if is_excluded(&rel) {
                continue;
            }
            let ft = ent.file_type()?;
            if ft.is_dir() {
                walk(base, &p, out, total)?;
            } else if ft.is_file() {
                let bytes = std::fs::read(&p)?;
                *total += bytes.len() as u64;
                if out.len() >= MAX_FILES || *total > MAX_BYTES {
                    bail!("team folder is too large (limit {MAX_FILES} files / {MAX_BYTES} bytes)");
                }
                let rel = safe_rel(&rel).ok_or_else(|| anyhow!("unsafe path {rel:?}"))?;
                out.insert(rel, bytes);
            } else {
                bail!("{} is a link or special file; packs hold regular files only", p.display());
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    let mut total = 0;
    walk(dir, dir, &mut out, &mut total)?;
    Ok(out)
}

fn manifest_for(name: &str, version: &str, files: &BTreeMap<String, Vec<u8>>) -> PackManifest {
    PackManifest {
        name: name.into(),
        version: version.into(),
        files: files.iter().map(|(p, b)| PackFile { path: p.clone(), sha256: sha256_hex(b) }).collect(),
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

/// Pack `.allternit/teams/<team>` into `out` (a `.tar.gz`). The team must
/// validate first. Archive entries are `<team>/manifest.json` and
/// `<team>/<path>`, sorted, with fixed mode and mtime.
pub fn pack(root: &Path, team: &str, out: &Path) -> Result<PackResult> {
    let loaded = super::team::load_team(root, team)?;
    let files = read_dir_files(&loaded.dir)?;
    let version = loaded.file.version.clone().unwrap_or_else(|| "0.0.0".into());
    let manifest = manifest_for(team, &version, &files);
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let f = std::fs::File::create(out).with_context(|| format!("create {}", out.display()))?;
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    let mut add = |path: String, bytes: &[u8]| -> Result<()> {
        let mut h = tar::Header::new_gnu();
        h.set_size(bytes.len() as u64);
        h.set_mode(0o644);
        h.set_mtime(0);
        h.set_entry_type(tar::EntryType::Regular);
        h.set_cksum();
        tar.append_data(&mut h, path, bytes)?;
        Ok(())
    };
    add(format!("{team}/{MANIFEST_FILE}"), format!("{}\n", serde_json::to_string_pretty(&manifest)?).as_bytes())?;
    for (p, b) in &files {
        add(format!("{team}/{p}"), b)?;
    }
    tar.into_inner()?.finish()?;
    let archive_sha256 = sha256_hex(&std::fs::read(out)?);
    Ok(PackResult { manifest, archive: out.to_path_buf(), archive_sha256 })
}

/// Read a `.tar.gz`, keeping regular files whose raw path `select` maps to a
/// relative path. Any unsafe path, or a link/device among kept entries, fails.
fn read_tar_gz(bytes: &[u8], select: impl Fn(&str) -> Option<String>) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut out = BTreeMap::new();
    let mut total = 0u64;
    for ent in ar.entries().context("read archive")? {
        let mut ent = ent.context("read archive entry")?;
        let raw = ent.path()?.to_string_lossy().to_string();
        let ty = ent.header().entry_type();
        if matches!(ty, tar::EntryType::XGlobalHeader | tar::EntryType::XHeader) {
            continue;
        }
        let trimmed = raw.trim_end_matches('/');
        if trimmed.is_empty() {
            continue;
        }
        let safe = safe_rel(trimmed).ok_or_else(|| anyhow!("archive entry {raw:?} escapes the pack (absolute or '..' path)"))?;
        let Some(rel) = select(&safe) else { continue };
        match ty {
            tar::EntryType::Directory => continue,
            tar::EntryType::Regular | tar::EntryType::Continuous => {}
            other => bail!("archive entry {raw:?} is {other:?}; packs hold regular files only"),
        }
        let rel = safe_rel(&rel).ok_or_else(|| anyhow!("archive entry {raw:?} maps to an unsafe path"))?;
        total += ent.header().size()?;
        if out.len() >= MAX_FILES || total > MAX_BYTES {
            bail!("archive is too large (limit {MAX_FILES} files / {MAX_BYTES} bytes)");
        }
        let mut buf = vec![];
        ent.read_to_end(&mut buf)?;
        if out.insert(rel.clone(), buf).is_some() {
            bail!("archive lists {rel:?} twice");
        }
    }
    Ok(out)
}

/// When every path sits under one top folder, strip it and return its name.
fn strip_common_top(files: BTreeMap<String, Vec<u8>>) -> (Option<String>, BTreeMap<String, Vec<u8>>) {
    let top = files.keys().next().and_then(|k| k.split_once('/')).map(|(t, _)| t.to_string());
    match top {
        Some(t) if files.keys().all(|k| k.starts_with(&format!("{t}/"))) => {
            let stripped = files.into_iter().map(|(k, v)| (k[t.len() + 1..].to_string(), v)).collect();
            (Some(t), stripped)
        }
        _ => (None, files),
    }
}

/// Install a pack (`.tar.gz`) or a team folder into `<root>/.allternit/teams/<name>`.
pub fn install_from_path(root: &Path, src: &Path, opts: InstallOptions) -> Result<InstallReport> {
    let (fallback_name, files) = if src.is_dir() {
        let name = src.file_name().map(|n| n.to_string_lossy().to_string());
        let mut files = read_dir_files(src)?;
        if let Ok(m) = std::fs::read(src.join(MANIFEST_FILE)) {
            files.insert(MANIFEST_FILE.into(), m);
        }
        (name, files)
    } else {
        let bytes = std::fs::read(src).with_context(|| format!("read {}", src.display()))?;
        let (top, files) = strip_common_top(read_tar_gz(&bytes, |p| Some(p.to_string()))?);
        (top, files)
    };
    install_files(root, files, fallback_name, &src.display().to_string(), opts)
}

/// Verify, validate and (unless a dry run) write a team's files.
fn install_files(
    root: &Path,
    mut files: BTreeMap<String, Vec<u8>>,
    fallback_name: Option<String>,
    source: &str,
    opts: InstallOptions,
) -> Result<InstallReport> {
    let manifest: Option<PackManifest> = match files.remove(MANIFEST_FILE) {
        Some(b) => Some(serde_json::from_slice(&b).context("manifest.json does not parse")?),
        None => None,
    };
    files.retain(|k, _| !is_excluded(k));
    let mut listed = vec![];
    if let Some(m) = &manifest {
        let mut seen = std::collections::BTreeSet::new();
        for f in &m.files {
            let p = safe_rel(&f.path).ok_or_else(|| anyhow!("manifest path {:?} is unsafe", f.path))?;
            if !seen.insert(p.clone()) {
                bail!("manifest lists {p:?} twice");
            }
            let b = files.get(&p).ok_or_else(|| anyhow!("manifest lists {p:?} but the pack has no such file"))?;
            let got = sha256_hex(b);
            if !got.eq_ignore_ascii_case(&f.sha256) {
                bail!("sha256 mismatch for {p:?}: manifest {} vs file {got}; the pack was altered", f.sha256);
            }
            listed.push(PackFile { path: p, sha256: got });
        }
        if let Some(extra) = files.keys().find(|k| !seen.contains(*k)) {
            bail!("pack holds {extra:?}, which the manifest does not list");
        }
    } else {
        listed = files.iter().map(|(p, b)| PackFile { path: p.clone(), sha256: sha256_hex(b) }).collect();
    }
    listed.sort();

    let yaml = files.get(TEAM_FILE).ok_or_else(|| anyhow!("no {TEAM_FILE} at the top of the pack"))?;
    let yaml = std::str::from_utf8(yaml).context("team.yaml is not UTF-8")?;
    let file_name = super::team::parse_team_file(yaml)?.name;
    let name = manifest
        .as_ref()
        .map(|m| m.name.clone())
        .or(file_name)
        .or(fallback_name)
        .ok_or_else(|| anyhow!("cannot tell the team name: no manifest, no `name` in team.yaml"))?;
    if !valid_slug(&name) {
        bail!("invalid team name {name:?}");
    }
    parse_team(&name, yaml)?;

    let dest = team_dir(root, &name);
    let exists = dest.exists();
    if exists && !opts.force {
        bail!("team {name:?} already exists at {}; pass force to replace it", dest.display());
    }
    let mut report = InstallReport {
        team: name.clone(),
        dest: dest.clone(),
        source: source.to_string(),
        manifest_present: manifest.is_some(),
        files: listed,
        dry_run: opts.dry_run,
        installed: false,
        replaced: false,
    };
    if opts.dry_run {
        return Ok(report);
    }

    let teams = root.join(TEAMS_DIR);
    std::fs::create_dir_all(&teams)?;
    let staging = tempfile::Builder::new().prefix(&format!(".install-{name}-")).tempdir_in(&teams)?;
    for (rel, bytes) in &files {
        let p = staging.path().join(rel);
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(&p, bytes)?;
    }
    let record = serde_json::json!({
        "source": source,
        "manifestPresent": report.manifest_present,
        "version": manifest.as_ref().map(|m| m.version.clone()),
        "files": report.files,
        "installedAt": chrono::Utc::now().to_rfc3339(),
    });
    std::fs::write(staging.path().join(INSTALL_RECORD), format!("{}\n", serde_json::to_string_pretty(&record)?))?;
    let staged = staging.keep();
    if exists {
        // Keep the old team's snapshots: they describe runs that happened.
        let old_snaps = dest.join(super::snapshot::SNAPSHOTS_DIR);
        if old_snaps.is_dir() {
            std::fs::rename(&old_snaps, staged.join(super::snapshot::SNAPSHOTS_DIR))?;
        }
        std::fs::remove_dir_all(&dest).with_context(|| format!("remove {}", dest.display()))?;
        report.replaced = true;
    }
    std::fs::rename(&staged, &dest).with_context(|| format!("move into {}", dest.display()))?;
    report.installed = true;
    Ok(report)
}

/// Parse `https://github.com/<owner>/<repo>/tree/<40-hex sha>/<path>`.
pub fn parse_github_source(url: &str) -> Result<GithubSource> {
    let rest = url
        .strip_prefix("https://github.com/")
        .ok_or_else(|| anyhow!("only https://github.com/<owner>/<repo>/tree/<commit>/<path> URLs are accepted"))?;
    if rest.contains('?') || rest.contains('#') {
        bail!("the URL must not carry a query or fragment");
    }
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    if parts.len() < 5 || parts[2] != "tree" {
        bail!("expected https://github.com/<owner>/<repo>/tree/<commit>/<path> (a folder pinned to a commit)");
    }
    let ok_name = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.') && s != "." && s != "..";
    if !ok_name(parts[0]) || !ok_name(parts[1]) {
        bail!("invalid owner or repo in {url:?}");
    }
    let commit = parts[3];
    if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{commit:?} is not a commit: installs must be pinned to a full 40-character commit sha (a branch or tag can change under you)");
    }
    let path = parts[4..].join("/");
    let path = safe_rel(&path).ok_or_else(|| anyhow!("unsafe folder path {path:?}"))?;
    Ok(GithubSource {
        owner: parts[0].into(),
        repo: parts[1].into(),
        commit: commit.to_ascii_lowercase(),
        path,
    })
}

/// Download the pinned commit's tarball with `curl -fsSL`, take only the
/// folder, and install it through the same checks as [`install_from_path`].
pub fn install_from_github(root: &Path, url: &str, opts: InstallOptions) -> Result<InstallReport> {
    let src = parse_github_source(url)?;
    let tmp = tempfile::NamedTempFile::new()?;
    let status = std::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--max-time", "300", "-o"])
        .arg(tmp.path())
        .arg(src.tarball_url())
        .status()
        .context("run curl")?;
    if !status.success() {
        bail!("download of {} failed ({status})", src.tarball_url());
    }
    let bytes = std::fs::read(tmp.path())?;
    install_from_github_tarball(root, &src, &bytes, url, opts)
}

/// The extraction half of [`install_from_github`] (testable without network).
pub fn install_from_github_tarball(root: &Path, src: &GithubSource, bytes: &[u8], source: &str, opts: InstallOptions) -> Result<InstallReport> {
    let prefix = format!("{}/", src.path);
    let files = read_tar_gz(bytes, |p| {
        // codeload wraps everything in `<repo>-<sha>/`.
        let (_, inner) = p.split_once('/')?;
        inner.strip_prefix(&prefix).map(str::to_string)
    })?;
    if files.is_empty() {
        bail!("{} has no files at {}", src.tarball_url(), src.path);
    }
    let fallback = src.path.rsplit('/').next().map(str::to_string);
    install_files(root, files, fallback, source, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::team::{tests::GOOD, CULTURE_FILE};

    fn make_team(root: &Path) {
        let d = team_dir(root, "product-build");
        std::fs::create_dir_all(d.join("bots/builder")).unwrap();
        std::fs::create_dir_all(d.join("snapshots")).unwrap();
        std::fs::write(d.join(TEAM_FILE), GOOD).unwrap();
        std::fs::write(d.join(CULTURE_FILE), "# Culture\n").unwrap();
        std::fs::write(d.join("bots/builder/LEARNED.md"), "- fact\n").unwrap();
        std::fs::write(d.join("snapshots/x.json"), "{}").unwrap();
    }

    fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(vec![], flate2::Compression::default()));
        for (p, b) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(b.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            // Write the raw name so tests can build hostile paths.
            let name = &mut h.as_old_mut().name;
            name[..p.len()].copy_from_slice(p.as_bytes());
            h.set_cksum();
            tar.append(&h, *b).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn pack_install_roundtrip() {
        let a = tempfile::tempdir().unwrap();
        make_team(a.path());
        let out = a.path().join("out/pb.tar.gz");
        let r = pack(a.path(), "product-build", &out).unwrap();
        let paths: Vec<_> = r.manifest.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["CULTURE.md", "bots/builder/LEARNED.md", "team.yaml"]);
        assert_eq!(r.archive_sha256, sha256_hex(&std::fs::read(&out).unwrap()));

        let b = tempfile::tempdir().unwrap();
        let dry = install_from_path(b.path(), &out, InstallOptions { dry_run: true, force: false }).unwrap();
        assert!(!dry.installed && dry.manifest_present);
        assert!(!team_dir(b.path(), "product-build").exists());
        let rep = install_from_path(b.path(), &out, InstallOptions::default()).unwrap();
        assert!(rep.installed);
        let t = crate::agents::team::load_team(b.path(), "product-build").unwrap();
        assert_eq!(t.raw, GOOD);
        assert!(team_dir(b.path(), "product-build").join(INSTALL_RECORD).is_file());
        // Existing team: refused without force, replaced with it (snapshots kept).
        assert!(install_from_path(b.path(), &out, InstallOptions::default()).unwrap_err().to_string().contains("already exists"));
        std::fs::create_dir_all(team_dir(b.path(), "product-build").join("snapshots")).unwrap();
        std::fs::write(team_dir(b.path(), "product-build").join("snapshots/s.json"), "{}").unwrap();
        let rep = install_from_path(b.path(), &out, InstallOptions { dry_run: false, force: true }).unwrap();
        assert!(rep.replaced);
        assert!(team_dir(b.path(), "product-build").join("snapshots/s.json").is_file());
        // A folder installs too (no manifest → hashes computed).
        let c = tempfile::tempdir().unwrap();
        let rep = install_from_path(c.path(), &team_dir(a.path(), "product-build"), InstallOptions::default()).unwrap();
        assert!(!rep.manifest_present);
        assert_eq!(rep.files.len(), 3);
    }

    #[test]
    fn tampered_and_hostile_packs_rejected() {
        let a = tempfile::tempdir().unwrap();
        make_team(a.path());
        let out = a.path().join("pb.tar.gz");
        let r = pack(a.path(), "product-build", &out).unwrap();
        let manifest = serde_json::to_vec(&r.manifest).unwrap();
        let tampered = tar_gz(&[
            ("product-build/manifest.json", &manifest),
            ("product-build/team.yaml", GOOD.as_bytes()),
            ("product-build/CULTURE.md", b"# Culture (edited)\n"),
            ("product-build/bots/builder/LEARNED.md", b"- fact\n"),
        ]);
        let p = a.path().join("t.tar.gz");
        std::fs::write(&p, &tampered).unwrap();
        let b = tempfile::tempdir().unwrap();
        let e = install_from_path(b.path(), &p, InstallOptions::default()).unwrap_err().to_string();
        assert!(e.contains("sha256 mismatch"), "{e}");

        let extra = tar_gz(&[
            ("product-build/manifest.json", &manifest),
            ("product-build/team.yaml", GOOD.as_bytes()),
            ("product-build/CULTURE.md", b"# Culture\n"),
            ("product-build/bots/builder/LEARNED.md", b"- fact\n"),
            ("product-build/evil.sh", b"rm -rf /\n"),
        ]);
        std::fs::write(&p, &extra).unwrap();
        assert!(install_from_path(b.path(), &p, InstallOptions::default()).unwrap_err().to_string().contains("does not list"));

        let trav = tar_gz(&[("product-build/team.yaml", GOOD.as_bytes()), ("product-build/../../etc/x", b"x")]);
        std::fs::write(&p, &trav).unwrap();
        assert!(install_from_path(b.path(), &p, InstallOptions::default()).unwrap_err().to_string().contains("escapes"));

        let bad_yaml = tar_gz(&[("t/team.yaml", b"bots:\n  - { bot: a, role: r, binding: cloud }\n")]);
        std::fs::write(&p, &bad_yaml).unwrap();
        assert!(install_from_path(b.path(), &p, InstallOptions::default()).unwrap_err().to_string().contains("bots[0].binding"));
        assert!(!b.path().join(TEAMS_DIR).join("product-build").exists());
    }

    #[test]
    fn github_urls_must_pin_a_commit() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let s = parse_github_source(&format!("https://github.com/Gizziio/teams/tree/{sha}/teams/product-build/")).unwrap();
        assert_eq!((s.owner.as_str(), s.repo.as_str(), s.commit.as_str(), s.path.as_str()), ("Gizziio", "teams", sha, "teams/product-build"));
        assert_eq!(s.tarball_url(), format!("https://codeload.github.com/Gizziio/teams/tar.gz/{sha}"));
        let e = parse_github_source("https://github.com/Gizziio/teams/tree/main/teams/pb").unwrap_err().to_string();
        assert!(e.contains("40-character commit"), "{e}");
        for bad in [
            "http://github.com/o/r/tree/0123456789abcdef0123456789abcdef01234567/p",
            "https://gitlab.com/o/r/tree/0123456789abcdef0123456789abcdef01234567/p",
            "https://github.com/o/r/blob/0123456789abcdef0123456789abcdef01234567/p",
            "https://github.com/o/r/tree/0123456789abcdef0123456789abcdef01234567",
            "https://github.com/o/r/tree/0123456789abcdef0123456789abcdef01234567/../x",
            "https://github.com/o/r/tree/0123456789abcdef0123456789abcdef01234567/p?x=1",
        ] {
            assert!(parse_github_source(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn github_tarball_subfolder_install() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        let src = parse_github_source(&format!("https://github.com/o/r/tree/{sha}/teams/product-build")).unwrap();
        let tgz = tar_gz(&[
            ("r-0123456/README.md", b"hi"),
            ("r-0123456/teams/product-build/team.yaml", GOOD.as_bytes()),
            ("r-0123456/teams/product-build/CULTURE.md", b"# C\n"),
            ("r-0123456/teams/other/team.yaml", b"junk"),
        ]);
        let root = tempfile::tempdir().unwrap();
        let rep = install_from_github_tarball(root.path(), &src, &tgz, "gh", InstallOptions::default()).unwrap();
        assert_eq!(rep.team, "product-build");
        assert!(!rep.manifest_present);
        assert_eq!(rep.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), vec!["CULTURE.md", "team.yaml"]);
    }
}
