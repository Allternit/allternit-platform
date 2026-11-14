//! Live check against the real catalog (needs network):
//! `cargo run -p allternit-model-source --example live_check [org/repo] [path]`
//! With no args: the smallest model in `/api/catalog`, then its smallest file.

use allternit_model_source::{Client, FetchOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::new()?;
    let mut args = std::env::args().skip(1);
    let repo = match args.next() {
        Some(r) => r,
        None => {
            let mut models: Vec<_> = client
                .catalog()
                .await?
                .into_iter()
                .filter(|m| m.kind.as_deref() == Some("model") && m.total_size.is_some())
                .collect();
            println!("catalog: {} models at {}", models.len(), client.base_url());
            models.sort_by_key(|m| m.total_size);
            models.first().ok_or("empty catalog")?.id.clone()
        }
    };
    let res = client.resolve(&repo, None).await?;
    println!(
        "resolve {repo}: basis={:?} commit={} fallback={:?}",
        res.basis, res.revision, res.fallback_reason
    );
    for s in &res.sources {
        println!("  source {:?} {}", s.kind, s.base_url);
    }
    let path = match args.next() {
        Some(p) => p,
        None => res
            .files
            .iter()
            .filter(|(_, f)| f.sha256.is_some())
            .min_by_key(|(_, f)| f.size)
            .map(|(p, _)| p.clone())
            .ok_or("no hashed files")?,
    };
    let dest = std::env::temp_dir()
        .join("allternit-model-source-live")
        .join(&path);
    let out = client
        .fetch_verified(&res, &path, &dest, &FetchOptions::default())
        .await?;
    println!("fetched {path}: {} bytes sha256={}", out.size, out.sha256);
    println!(
        "verification: {}",
        serde_json::to_string(&out.verification)?
    );
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("allternit-model-source-live"));
    Ok(())
}
