//! PrimitiveRegistry: the 191 canonical primitive ids of Kernel ABI 1.0.0.
//!
//! Embedded at compile time from `spec/Contracts/kernel/v1/registry/primitives.json`
//! (single source of truth; no copy to drift). Lookup by dotted id
//! (`obs.list_files`) or UPPER_SNAKE alias (`LIST_FILES`, plus extra aliases).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

const REGISTRY_JSON: &str =
    include_str!("../../../spec/Contracts/kernel/v1/registry/primitives.json");

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Primitive {
    pub id: String,
    pub alias: String,
    #[serde(default)]
    pub extra_aliases: Vec<String>,
    pub family: String,
    #[serde(default)]
    pub class: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RegistryFile {
    registry_version: String,
    count: usize,
    primitives: Vec<Primitive>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("unknown primitive '{0}'")]
    Unknown(String),
    #[error("registry load failed: {0}")]
    Load(String),
}

#[derive(Debug)]
pub struct PrimitiveRegistry {
    pub version: String,
    primitives: Vec<Primitive>,
    by_id: HashMap<String, usize>,
    by_alias: HashMap<String, usize>,
}

impl PrimitiveRegistry {
    pub fn from_json(json: &str) -> Result<Self, RegistryError> {
        let f: RegistryFile =
            serde_json::from_str(json).map_err(|e| RegistryError::Load(e.to_string()))?;
        if f.count != f.primitives.len() {
            return Err(RegistryError::Load(format!(
                "count {} != {} primitives",
                f.count,
                f.primitives.len()
            )));
        }
        let mut by_id = HashMap::new();
        let mut by_alias = HashMap::new();
        for (i, p) in f.primitives.iter().enumerate() {
            if by_id.insert(p.id.clone(), i).is_some() {
                return Err(RegistryError::Load(format!("duplicate id {}", p.id)));
            }
            for a in std::iter::once(&p.alias).chain(p.extra_aliases.iter()) {
                if by_alias.insert(a.clone(), i).is_some() {
                    return Err(RegistryError::Load(format!("duplicate alias {a}")));
                }
            }
        }
        Ok(Self { version: f.registry_version, primitives: f.primitives, by_id, by_alias })
    }

    /// The embedded ABI 1.0.0 registry (parsed once).
    pub fn global() -> &'static PrimitiveRegistry {
        static REG: OnceLock<PrimitiveRegistry> = OnceLock::new();
        REG.get_or_init(|| {
            PrimitiveRegistry::from_json(REGISTRY_JSON)
                .expect("embedded primitives.json must be valid")
        })
    }

    pub fn len(&self) -> usize {
        self.primitives.len()
    }

    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Primitive> {
        self.primitives.iter()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    /// Lookup by dotted id.
    pub fn get(&self, id: &str) -> Option<&Primitive> {
        self.by_id.get(id).map(|&i| &self.primitives[i])
    }

    /// Lookup by UPPER_SNAKE alias (canonical or extra).
    pub fn get_by_alias(&self, alias: &str) -> Option<&Primitive> {
        self.by_alias.get(alias).map(|&i| &self.primitives[i])
    }

    /// Resolve either spelling to the canonical dotted id; unknown is rejected.
    pub fn resolve(&self, id_or_alias: &str) -> Result<&Primitive, RegistryError> {
        self.get(id_or_alias)
            .or_else(|| self.get_by_alias(id_or_alias))
            .ok_or_else(|| RegistryError::Unknown(id_or_alias.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_191_ids() {
        let r = PrimitiveRegistry::global();
        assert_eq!(r.len(), 191);
        assert_eq!(r.version, "1.0.0");
    }

    #[test]
    fn alias_round_trip() {
        let r = PrimitiveRegistry::global();
        for p in r.iter() {
            assert_eq!(r.get_by_alias(&p.alias).unwrap().id, p.id);
            assert_eq!(r.resolve(&p.alias).unwrap().id, p.id);
            assert_eq!(r.resolve(&p.id).unwrap().alias, p.alias);
            for a in &p.extra_aliases {
                assert_eq!(r.get_by_alias(a).unwrap().id, p.id);
            }
        }
        assert_eq!(r.resolve("LIST_FILES").unwrap().id, "obs.list_files");
        assert_eq!(r.resolve("RECOVER_TOOL_FAILURE").unwrap().id, "tool.recover");
    }

    #[test]
    fn unknown_rejected() {
        let r = PrimitiveRegistry::global();
        assert_eq!(r.resolve("obs.nope"), Err(RegistryError::Unknown("obs.nope".into())));
        assert!(!r.contains("NOPE"));
        assert!(r.get_by_alias("obs.list_files").is_none());
    }

    #[test]
    fn rejects_count_mismatch() {
        let bad = r#"{"registry_version":"1","count":2,"primitives":[]}"#;
        assert!(PrimitiveRegistry::from_json(bad).is_err());
    }
}
