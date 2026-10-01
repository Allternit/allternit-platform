//! Substrate router: dispatch desktop operations between Incus (Linux/Windows)
//! and Tart (macOS) behind a single `ExecutionDriver` handle.
//!
//! The router is the heterogeneous glue described in
//! `allternit-cloud-computer-architecture.md`: one control-plane API, two
//! substrates. Routing decisions come from the `ALLTERNIT_DESKTOP_OS` env var
//! injected at provisioning time, or from the `provider` stored in an existing
//! execution handle.

use crate::driver::IncusDriver;
use crate::tart::TartDriver;
use allternit_driver_interface::{
    Artifact, CommandSpec, DesktopEndpoint, DriverCapabilities, DriverError, DriverHealth,
    DriverType, EnvSpecType, ExecResult, ExecutionDriver, ExecutionHandle, IsolationLevel,
    LogEntry, Receipt, ResourceConsumption, ResourceSpec, SpawnSpec,
};
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{info, warn};

const OS_ENV_KEY: &str = "ALLTERNIT_DESKTOP_OS";
const PROVIDER_ENV_KEY: &str = "ALLTERNIT_DESKTOP_PROVIDER";
const PROVIDER_INCUS: &str = "incus";
const PROVIDER_TART: &str = "tart";

/// Substrate-aware router implementing the unified execution driver trait.
#[derive(Clone, Debug)]
pub struct SubstrateRouter {
    incus: Option<Arc<IncusDriver>>,
    tart: Option<Arc<TartDriver>>,
}

impl SubstrateRouter {
    pub fn new(incus: Option<Arc<IncusDriver>>, tart: Option<Arc<TartDriver>>) -> Self {
        Self { incus, tart }
    }

    pub fn incus(&self) -> Option<&Arc<IncusDriver>> {
        self.incus.as_ref()
    }

    pub fn tart(&self) -> Option<&Arc<TartDriver>> {
        self.tart.as_ref()
    }

    /// True if at least one substrate is configured.
    pub fn has_any_driver(&self) -> bool {
        self.incus.is_some() || self.tart.is_some()
    }

    fn choose_spawn_driver(
        &self,
        spec: &SpawnSpec,
    ) -> Result<Arc<dyn ExecutionDriver>, DriverError> {
        let os = spec.env.env_vars.get(OS_ENV_KEY).map(|s| s.as_str());
        let preferred = spec
            .env
            .env_vars
            .get(PROVIDER_ENV_KEY)
            .map(|s| s.as_str());

        if let Some(provider) = preferred {
            return self.choose_provider(provider, os);
        }

        match os {
            Some("macos") => self
                .tart
                .clone()
                .map(|d| d as Arc<dyn ExecutionDriver>)
                .ok_or_else(|| DriverError::NotSupported {
                    feature: "macos Tart substrate".to_string(),
                }),
            Some("windows") | Some("linux") => self
                .incus
                .clone()
                .map(|d| d as Arc<dyn ExecutionDriver>)
                .ok_or_else(|| DriverError::NotSupported {
                    feature: "Incus substrate".to_string(),
                }),
            _ => {
                // No explicit OS: prefer Incus for generic Linux/Windows images,
                // fall back to Tart if Incus is unavailable.
                if let Some(d) = self.incus.clone() {
                    Ok(d)
                } else if let Some(d) = self.tart.clone() {
                    Ok(d)
                } else {
                    Err(DriverError::NotSupported {
                        feature: "any desktop substrate".to_string(),
                    })
                }
            }
        }
    }

    fn provider_supports_os(provider: &str, os: Option<&str>) -> bool {
        match (provider, os) {
            (PROVIDER_TART, Some("macos")) | (PROVIDER_TART, Some("linux")) => true,
            (PROVIDER_INCUS, Some("linux")) | (PROVIDER_INCUS, Some("windows")) => true,
            _ => false,
        }
    }

    fn choose_provider(
        &self,
        provider: &str,
        os: Option<&str>,
    ) -> Result<Arc<dyn ExecutionDriver>, DriverError> {
        if !Self::provider_supports_os(provider, os) {
            return Err(DriverError::NotSupported {
                feature: format!("provider '{}' does not support os {:?}", provider, os),
            });
        }
        match provider {
            PROVIDER_INCUS => self.incus.clone().map(|d| d as Arc<dyn ExecutionDriver>).ok_or_else(
                || DriverError::NotSupported {
                    feature: "Incus substrate".to_string(),
                }),
            PROVIDER_TART => self
                .tart
                .clone()
                .map(|d| d as Arc<dyn ExecutionDriver>)
                .ok_or_else(|| DriverError::NotSupported {
                    feature: "Tart substrate".to_string(),
                }),
            _ => Err(DriverError::NotSupported {
                feature: format!("unknown provider '{}'", provider),
            }),
        }
    }

    fn choose_handle_driver(
        &self,
        handle: &ExecutionHandle,
    ) -> Result<Arc<dyn ExecutionDriver>, DriverError> {
        match handle.driver_info.get("provider").map(|s| s.as_str()) {
            Some(PROVIDER_INCUS) => self
                .incus
                .clone()
                .map(|d| d as Arc<dyn ExecutionDriver>)
                .ok_or_else(|| DriverError::NotSupported {
                    feature: "incus substrate no longer configured".to_string(),
                }),
            Some(PROVIDER_TART) => self
                .tart
                .clone()
                .map(|d| d as Arc<dyn ExecutionDriver>)
                .ok_or_else(|| DriverError::NotSupported {
                    feature: "tart substrate no longer configured".to_string(),
                }),
            _ => {
                // Handles created before the provider tag existed: infer from OS env var.
                self.choose_spawn_driver(&SpawnSpec {
                    env: handle.env_spec.clone(),
                    tenant: handle.tenant.clone(),
                    policy: allternit_driver_interface::PolicySpec::default_permissive(),
                    resources: ResourceSpec::default(),
                    project: None,
                    workspace: None,
                    run_id: Some(handle.id),
                    envelope: None,
                    prewarm_pool: None,
                })
            }
        }
    }
}

#[async_trait]
impl ExecutionDriver for SubstrateRouter {
    fn capabilities(&self) -> DriverCapabilities {
        // Aggregate the two substrates under a single "router" capability.
        // Max resources are the larger of the two; supported env specs are the union.
        let mut max_resources = ResourceSpec::default();
        let mut supported_env_specs = vec![EnvSpecType::Oci];
        let mut has_incus = false;
        let mut _has_tart = false;

        if let Some(d) = &self.incus {
            let c = d.capabilities();
            max_resources.cpu_millis = max_resources.cpu_millis.max(c.max_resources.cpu_millis);
            max_resources.memory_mib = max_resources.memory_mib.max(c.max_resources.memory_mib);
            supported_env_specs.extend(c.supported_env_specs);
            has_incus = true;
        }
        if let Some(d) = &self.tart {
            let c = d.capabilities();
            max_resources.cpu_millis = max_resources.cpu_millis.max(c.max_resources.cpu_millis);
            max_resources.memory_mib = max_resources.memory_mib.max(c.max_resources.memory_mib);
            supported_env_specs.extend(c.supported_env_specs);
            _has_tart = true;
        }

        let mut seen = Vec::new();
        let supported_env_specs: Vec<_> = supported_env_specs
            .into_iter()
            .filter(|t| {
                if seen.contains(t) {
                    false
                } else {
                    seen.push(*t);
                    true
                }
            })
            .collect();

        DriverCapabilities {
            resize: false,
            clone: false,
            driver_type: DriverType::MicroVM,
            isolation: IsolationLevel::Maximum,
            max_resources,
            supported_env_specs,
            features: allternit_driver_interface::DriverFeatures {
                snapshot: has_incus,
                live_restore: false,
                gpu: false,
                prewarm: false,
            },
        }
    }

    async fn spawn(&self, spec: SpawnSpec) -> Result<ExecutionHandle, DriverError> {
        let driver = self.choose_spawn_driver(&spec)?;
        info!(os = %spec.env.env_vars.get(OS_ENV_KEY).cloned().unwrap_or_default(), "Routing desktop spawn to substrate");
        driver.spawn(spec).await
    }

    async fn pause_vm(&self, handle: &ExecutionHandle) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.pause_vm(handle).await
    }

    async fn resume_vm(&self, handle: &ExecutionHandle) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.resume_vm(handle).await
    }

    async fn destroy(&self, handle: &ExecutionHandle) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.destroy(handle).await
    }

    async fn exec(
        &self,
        handle: &ExecutionHandle,
        cmd: CommandSpec,
    ) -> Result<ExecResult, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.exec(handle, cmd).await
    }

    async fn stream_logs(&self, handle: &ExecutionHandle) -> Result<Vec<LogEntry>, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.stream_logs(handle).await
    }

    async fn get_artifacts(&self, handle: &ExecutionHandle) -> Result<Vec<Artifact>, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.get_artifacts(handle).await
    }

    async fn get_consumption(
        &self,
        handle: &ExecutionHandle,
    ) -> Result<ResourceConsumption, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.get_consumption(handle).await
    }

    async fn get_receipt(&self, handle: &ExecutionHandle) -> Result<Option<Receipt>, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.get_receipt(handle).await
    }

    async fn health_check(&self) -> Result<DriverHealth, DriverError> {
        let mut healthy = true;
        let mut message_parts = Vec::new();
        let mut active = 0u32;
        let mut available_capacity = ResourceSpec::default();
        let mut capabilities: Vec<String> = Vec::new();

        if let Some(d) = &self.incus {
            match d.health_check().await {
                Ok(h) => {
                    healthy = healthy && h.healthy;
                    active += h.active_executions;
                    available_capacity.cpu_millis = available_capacity.cpu_millis.max(h.available_capacity.cpu_millis);
                    available_capacity.memory_mib = available_capacity.memory_mib.max(h.available_capacity.memory_mib);
                    capabilities.extend(h.capabilities);
                    if !h.healthy {
                        message_parts.push(format!("incus unhealthy: {:?}", h.message));
                    }
                }
                Err(e) => {
                    healthy = false;
                    message_parts.push(format!("incus health error: {}", e));
                }
            }
        }
        if let Some(d) = &self.tart {
            match d.health_check().await {
                Ok(h) => {
                    healthy = healthy && h.healthy;
                    active += h.active_executions;
                    available_capacity.cpu_millis = available_capacity.cpu_millis.max(h.available_capacity.cpu_millis);
                    available_capacity.memory_mib = available_capacity.memory_mib.max(h.available_capacity.memory_mib);
                    capabilities.extend(h.capabilities);
                    if !h.healthy {
                        message_parts.push(format!("tart unhealthy: {:?}", h.message));
                    }
                }
                Err(e) => {
                    healthy = false;
                    message_parts.push(format!("tart health error: {}", e));
                }
            }
        }

        if self.incus.is_none() && self.tart.is_none() {
            healthy = false;
            message_parts.push("no substrate configured".to_string());
        }

        // Deduplicate while preserving order.
        let mut seen = std::collections::HashSet::new();
        capabilities.retain(|c| seen.insert(c.clone()));

        Ok(DriverHealth {
            healthy,
            message: if message_parts.is_empty() {
                None
            } else {
                Some(message_parts.join("; "))
            },
            active_executions: active,
            available_capacity,
            capabilities,
        })
    }

    async fn substrate_capacities(
        &self,
    ) -> Result<Vec<(String, DriverHealth, DriverCapabilities)>, DriverError> {
        let mut out = Vec::new();
        if let Some(d) = &self.incus {
            match d.health_check().await {
                Ok(health) => {
                    let caps = d.capabilities();
                    out.push(("incus".to_string(), health, caps));
                }
                Err(e) => {
                    warn!(error = %e, "Incus substrate capacity check failed");
                }
            }
        }
        if let Some(d) = &self.tart {
            match d.health_check().await {
                Ok(health) => {
                    let caps = d.capabilities();
                    out.push(("tart".to_string(), health, caps));
                }
                Err(e) => {
                    warn!(error = %e, "Tart substrate capacity check failed");
                }
            }
        }
        Ok(out)
    }

    async fn get_desktop_endpoint(
        &self,
        handle: &ExecutionHandle,
    ) -> Result<Option<DesktopEndpoint>, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.get_desktop_endpoint(handle).await
    }

    async fn guest_service_url(
        &self,
        handle: &ExecutionHandle,
        guest_port: u16,
    ) -> Result<String, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.guest_service_url(handle, guest_port).await
    }

    async fn get_desktop_endpoint_by_native_id(
        &self,
        native_id: &str,
    ) -> Result<Option<DesktopEndpoint>, DriverError> {
        // Try each configured driver; the one that owns the native sandbox will
        // recognize it. This lets API status checks work without knowing the
        // substrate ahead of time.
        if let Some(d) = &self.tart {
            if let Ok(Some(ep)) = d.get_desktop_endpoint_by_native_id(native_id).await {
                return Ok(Some(ep));
            }
        }
        if let Some(d) = &self.incus {
            match d.get_desktop_endpoint_by_native_id(native_id).await {
                Ok(Some(ep)) => return Ok(Some(ep)),
                // No Incus host has the instance (and Tart didn't either).
                Err(e @ DriverError::NotFound { .. }) => return Err(e),
                _ => {}
            }
        }
        Ok(None)
    }

    fn supports_desktop(&self) -> bool {
        self.incus.as_ref().map(|d| d.supports_desktop()).unwrap_or(false)
            || self.tart.as_ref().map(|d| d.supports_desktop()).unwrap_or(false)
    }

    async fn register_native_sandbox(
        &self,
        native_id: &str,
        agent_id: &str,
        host: &str,
    ) -> Result<(), DriverError> {
        // Register with every configured driver; the one that owns the native_id will keep it.
        if let Some(d) = &self.incus {
            let _ = d.register_native_sandbox(native_id, agent_id, host).await;
        }
        if let Some(d) = &self.tart {
            let _ = d.register_native_sandbox(native_id, agent_id, host).await;
        }
        Ok(())
    }

    async fn pull_file(
        &self,
        handle: &ExecutionHandle,
        path: &str,
    ) -> Result<Vec<u8>, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.pull_file(handle, path).await
    }

    async fn push_file(
        &self,
        handle: &ExecutionHandle,
        path: &str,
        content: Vec<u8>,
    ) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.push_file(handle, path, content).await
    }

    async fn resize_vm(
        &self,
        handle: &ExecutionHandle,
        resources: &ResourceSpec,
    ) -> Result<(), DriverError> {
        self.choose_handle_driver(handle)?
            .resize_vm(handle, resources)
            .await
    }

    async fn clone_vm(
        &self,
        handle: &ExecutionHandle,
        new_native_id: &str,
    ) -> Result<ExecutionHandle, DriverError> {
        self.choose_handle_driver(handle)?
            .clone_vm(handle, new_native_id)
            .await
    }

    async fn clone_from_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
        new_native_id: &str,
        resources: Option<&ResourceSpec>,
        env: &std::collections::HashMap<String, String>,
    ) -> Result<ExecutionHandle, DriverError> {
        self.choose_handle_driver(handle)?
            .clone_from_snapshot(handle, snapshot_id, new_native_id, resources, env)
            .await
    }

    async fn create_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
        stateful: bool,
    ) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.create_snapshot(handle, snapshot_id, stateful).await
    }

    async fn restore_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
    ) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.restore_snapshot(handle, snapshot_id).await
    }

    async fn delete_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
    ) -> Result<(), DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.delete_snapshot(handle, snapshot_id).await
    }

    async fn list_snapshots(
        &self,
        handle: &ExecutionHandle,
    ) -> Result<Vec<allternit_driver_interface::SnapshotInfo>, DriverError> {
        let driver = self.choose_handle_driver(handle)?;
        driver.list_snapshots(handle).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use allternit_driver_interface::{EnvironmentSpec, PolicySpec, ResourceSpec, TenantId};

    fn dummy_spec(os: &str) -> SpawnSpec {
        let mut env = EnvironmentSpec::default();
        env.env_vars.insert(OS_ENV_KEY.to_string(), os.to_string());
        SpawnSpec {
            tenant: TenantId::new("test").unwrap(),
            env,
            policy: PolicySpec::default_permissive(),
            resources: ResourceSpec::default(),
            project: None,
            workspace: None,
            run_id: None,
            envelope: None,
            prewarm_pool: None,
        }
    }

    #[test]
    fn router_without_drivers_errors() {
        let router = SubstrateRouter::new(None, None);
        assert!(router.choose_spawn_driver(&dummy_spec("macos")).is_err());
        assert!(router.choose_spawn_driver(&dummy_spec("linux")).is_err());
        assert!(!router.supports_desktop());
    }

    #[test]
    fn router_chooses_tart_for_macos() {
        // We cannot build a real driver in a unit test, but we can verify the
        // error message identifies the expected substrate.
        let router = SubstrateRouter::new(None, None);
        let err = router.choose_spawn_driver(&dummy_spec("macos")).unwrap_err();
        match err {
            DriverError::NotSupported { feature } => assert!(feature.contains("Tart")),
            _ => panic!("expected NotSupported for Tart"),
        }
    }

    #[test]
    fn router_chooses_incus_for_linux_and_windows() {
        let router = SubstrateRouter::new(None, None);
        for os in ["linux", "windows"] {
            let err = router.choose_spawn_driver(&dummy_spec(os)).unwrap_err();
            match err {
                DriverError::NotSupported { feature } => assert!(feature.contains("Incus")),
                _ => panic!("expected NotSupported for Incus"),
            }
        }
    }

    fn dummy_spec_with_provider(os: &str, provider: &str) -> SpawnSpec {
        let mut env = EnvironmentSpec::default();
        env.env_vars.insert(OS_ENV_KEY.to_string(), os.to_string());
        env.env_vars.insert(PROVIDER_ENV_KEY.to_string(), provider.to_string());
        SpawnSpec {
            tenant: TenantId::new("test").unwrap(),
            env,
            policy: PolicySpec::default_permissive(),
            resources: ResourceSpec::default(),
            project: None,
            workspace: None,
            run_id: None,
            envelope: None,
            prewarm_pool: None,
        }
    }

    #[test]
    fn router_provider_override_can_send_linux_to_tart() {
        let router = SubstrateRouter::new(None, None);
        let err = router
            .choose_spawn_driver(&dummy_spec_with_provider("linux", "tart"))
            .unwrap_err();
        match err {
            DriverError::NotSupported { feature } => assert!(feature.contains("Tart")),
            _ => panic!("expected NotSupported for Tart"),
        }
    }

    #[test]
    fn router_provider_override_can_send_macos_to_incus_is_invalid() {
        let router = SubstrateRouter::new(None, None);
        let err = router
            .choose_spawn_driver(&dummy_spec_with_provider("macos", "incus"))
            .unwrap_err();
        match err {
            DriverError::NotSupported { feature } => {
                assert!(
                    feature.to_lowercase().contains("incus") && feature.to_lowercase().contains("macos"),
                    "unexpected feature: {}",
                    feature
                );
            }
            _ => panic!("expected NotSupported for invalid provider/os pair"),
        }
    }

    #[tokio::test]
    async fn router_forwards_guest_service_url_to_handle_driver() {
        use crate::substrate::{HttpClient, IncusSubstrate, SubstrateError};
        use allternit_driver_interface::ExecutionId;
        use serde_json::{json, Value};
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        struct ScriptedClient {
            responses: Mutex<std::collections::VecDeque<(u16, Value)>>,
        }
        #[async_trait]
        impl HttpClient for ScriptedClient {
            async fn request(
                &self,
                _method: reqwest::Method,
                _path: &str,
                _body: Option<Value>,
            ) -> Result<(u16, Value), SubstrateError> {
                Ok(self
                    .responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected HTTP request"))
            }
            async fn request_bytes(
                &self,
                _: reqwest::Method,
                _: &str,
                _: Option<Value>,
            ) -> Result<(u16, Vec<u8>), SubstrateError> {
                panic!("unexpected bytes request")
            }
            async fn request_bytes_with_body(
                &self,
                _: reqwest::Method,
                _: &str,
                _: Vec<u8>,
            ) -> Result<(u16, Vec<u8>), SubstrateError> {
                panic!("unexpected bytes request")
            }
        }

        // Existing proxy device for guest port 6010: the Incus driver answers
        // with a single GET and no allocation PATCH.
        let substrate = IncusSubstrate::with_client(Box::new(ScriptedClient {
            responses: Mutex::new(
                vec![(
                    200,
                    json!({"metadata":{"devices":{
                        "svc6010": {"type":"proxy","listen":"tcp:0.0.0.0:36010","connect":"tcp:127.0.0.1:6010"}
                    }}}),
                )]
                .into(),
            ),
        }));
        let driver = IncusDriver::new(Arc::new(substrate), "127.0.0.1");
        // Single-host pools are keyed by the synthetic name "legacy"; the URL
        // must use the driver-level fallback host, not that name.
        assert_eq!(driver.pool().hosts()[0].vnc_host, "legacy");
        let router = SubstrateRouter::new(Some(Arc::new(driver)), None);
        let handle = ExecutionHandle {
            id: ExecutionId::new(),
            tenant: TenantId::new("user-test").unwrap(),
            driver_info: HashMap::from([
                ("native_id".to_string(), "source".to_string()),
                ("provider".to_string(), PROVIDER_INCUS.to_string()),
            ]),
            env_spec: Default::default(),
        };

        // Before the forwarding fix this hit the trait default and 501'd with
        // NotSupported{"guest service url"}; it must reach the Incus driver.
        // The URL must be the reachable fallback host ("127.0.0.1"), never
        // the synthetic pool name "legacy".
        let url = router
            .guest_service_url(&handle, 6010)
            .await
            .expect("router forwards guest_service_url to the substrate driver");
        assert_eq!(url, "http://127.0.0.1:36010");
    }

    #[tokio::test]
    async fn router_guest_service_url_without_driver_errors_from_routing() {
        use allternit_driver_interface::ExecutionId;
        use std::collections::HashMap;

        let router = SubstrateRouter::new(None, None);
        let mut env = EnvironmentSpec::default();
        env.env_vars
            .insert(OS_ENV_KEY.to_string(), "linux".to_string());
        let handle = ExecutionHandle {
            id: ExecutionId::new(),
            tenant: TenantId::new("test").unwrap(),
            driver_info: HashMap::new(),
            env_spec: env,
        };
        let err = router.guest_service_url(&handle, 6010).await.unwrap_err();
        match err {
            DriverError::NotSupported { feature } => assert!(
                !feature.contains("guest service url"),
                "must surface the routing error, not the trait default: {}",
                feature
            ),
            _ => panic!("expected NotSupported"),
        }
    }
}
