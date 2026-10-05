use std::path::PathBuf;
use anyhow::Result;
use crate::core::io::{ensure_dir, write_json_atomic};
use crate::core::types::OAuthTokenRecord;

pub struct OAuthVault {
    root_dir: PathBuf,
}

impl OAuthVault {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    fn tokens_dir(&self) -> PathBuf {
        self.root_dir.join(".allternit/vault/oauth/tokens")
    }

    pub fn store_token(&self, token: &OAuthTokenRecord) -> Result<()> {
        let dir = self.tokens_dir();
        ensure_dir(&dir)?;
        let path = dir.join(format!("{}.json", token.server_name));
        write_json_atomic(&path, token)?;
        Ok(())
    }

    pub fn get_token(&self, server_name: &str) -> Result<Option<OAuthTokenRecord>> {
        let path = self.tokens_dir().join(format!("{}.json", server_name));
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)?;
        let token: OAuthTokenRecord = serde_json::from_str(&content)?;
        Ok(Some(token))
    }

    pub fn list_servers(&self) -> Result<Vec<String>> {
        let dir = self.tokens_dir();
        if !dir.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") {
                out.push(name.replace(".json", ""));
            }
        }
        Ok(out)
    }
}
