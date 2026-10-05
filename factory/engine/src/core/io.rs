use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn ensure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

pub fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path, fallback: T) -> io::Result<T> {
    if !path.exists() {
        return Ok(fallback);
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(fallback);
    }
    let value = serde_json::from_str(&raw).unwrap_or(fallback);
    Ok(value)
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let value = serde_json::from_str(&raw)?;
    Ok(Some(value))
}

pub fn write_json_atomic(path: &Path, data: &impl serde::Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let tmp_path = PathBuf::from(format!("{}.tmp", path.display()));
    let json = serde_json::to_string_pretty(data).unwrap_or_else(|_| "{}".to_string());
    fs::write(&tmp_path, json)?;
    fs::rename(&tmp_path, path)?;
    Ok(())
}

pub fn append_json_line(path: &Path, record: &impl serde::Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let json = serde_json::to_string(record).unwrap_or_else(|_| "{}".to_string());
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(json.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn list_jsonl_sorted(dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().map(|ext| ext == "jsonl").unwrap_or(false))
        .collect();
    entries.sort();
    Ok(entries)
}

pub fn read_json_lines<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<T>(line) {
            out.push(value);
        }
    }
    Ok(out)
}
