//! Every route the selfhost router mounts must be in `selfhost-v1.yaml`,
//! with the same method. This is what keeps the server binary from growing
//! routes outside the contract (and from mounting any hosted-API route).

use std::collections::BTreeSet;

use allternit_api::selfhost::route_table;

fn spec_operations() -> BTreeSet<(String, String)> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/openapi/selfhost-v1.yaml");
    let text = std::fs::read_to_string(path).expect("read selfhost-v1.yaml");
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).expect("parse selfhost-v1.yaml");
    let paths = doc["paths"].as_mapping().expect("paths");
    let mut out = BTreeSet::new();
    for (p, item) in paths {
        let p = p.as_str().unwrap().to_string();
        for (m, _) in item.as_mapping().unwrap() {
            let m = m.as_str().unwrap();
            if ["get", "post", "put", "patch", "delete", "head", "options"].contains(&m) {
                out.insert((m.to_ascii_uppercase(), p.clone()));
            }
        }
    }
    out
}

/// axum `/v1/agents/:id` → OpenAPI `/v1/agents/{id}`.
fn to_openapi(path: &str) -> String {
    path.split('/')
        .map(|seg| match seg.strip_prefix(':') {
            Some(name) => format!("{{{name}}}"),
            None => seg.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[test]
fn every_mounted_route_is_in_the_contract() {
    let spec = spec_operations();
    let routes = route_table();
    assert!(!routes.is_empty());
    let mut missing = Vec::new();
    for (method, path) in &routes {
        let key = (method.to_string(), to_openapi(path));
        if !spec.contains(&key) {
            missing.push(format!("{} {}", key.0, key.1));
        }
    }
    assert!(missing.is_empty(), "routes not in selfhost-v1.yaml: {missing:?}");
}

#[test]
fn no_route_is_mounted_twice() {
    let routes = route_table();
    let unique: BTreeSet<_> = routes.iter().collect();
    assert_eq!(unique.len(), routes.len());
}

#[test]
fn every_contract_route_is_mounted() {
    // Client, worker (work and tool call) and internal routes alike.
    let mounted: BTreeSet<(String, String)> = route_table()
        .iter()
        .map(|(m, p)| (m.to_string(), to_openapi(p)))
        .collect();
    for op in spec_operations() {
        assert!(mounted.contains(&op), "{} {} is in the contract but not mounted", op.0, op.1);
    }
}
