//! Incus-backed `ExecutionDriver` for the Allternit platform.
//!
//! Implements the platform driver-interface so that `cmd/allternit-api` can
//! provision bot desktops on an Incus host. The driver launches an Ubuntu
//! cloud-image container/VM,
//! starts the desktop services via cloud-init, and exposes x11vnc through an
//! Incus proxy device.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tracing::{info, warn};

use allternit_driver_interface::{
    CommandSpec, DesktopEndpoint, DesktopProtocol, DriverCapabilities, DriverError, DriverFeatures,
    DriverHealth, DriverType, EnvSpecType, ExecResult, ExecutionDriver, ExecutionHandle,
    ExecutionId, IsolationLevel, Receipt, ResourceConsumption, ResourceSpec, SnapshotInfo,
    SpawnSpec, TenantId,
};

use crate::incus_pool::{IncusHost, IncusHostPool};
use crate::mesh::MeshConfig;
use crate::substrate::{ComputerSpec, IncusSubstrate, Substrate, SubstrateError};

const VNC_DEVICE: &str = "vnc";
const VNC_CONTAINER_PORT: u16 = 5900;
const PROXY_PORT_BASE: u32 = 30_000;

/// Incus execution driver.
#[derive(Debug)]
pub struct IncusDriver {
    pool: Arc<IncusHostPool>,
    /// Default VNC host for single-host pools or legacy handles.
    vnc_host: String,
    /// VNC password shared with the guest image.
    vnc_password: String,
    /// Next candidate host port for the VNC proxy.
    next_port: AtomicU32,
    /// In-memory map from Incus instance name to allocated VNC host port.
    vnc_ports: std::sync::Mutex<HashMap<String, u16>>,
    /// Optional mesh VPN configuration injected into every spawned desktop.
    mesh: Option<MeshConfig>,
}

impl IncusDriver {
    /// Create a driver from an existing Incus substrate.
    pub fn new(substrate: Arc<IncusSubstrate>, vnc_host: impl Into<String>) -> Self {
        Self::from_pool(IncusHostPool::single("legacy", substrate), vnc_host)
    }

    /// Build a driver from an explicit multi-host pool.
    pub fn from_pool(pool: IncusHostPool, vnc_host: impl Into<String>) -> Self {
        Self {
            pool: Arc::new(pool),
            vnc_host: vnc_host.into(),
            vnc_password: std::env::var("BOT_DESKTOP_VNC_PASSWORD")
                .unwrap_or_else(|_| "allternit".to_string()),
            next_port: AtomicU32::new(PROXY_PORT_BASE),
            vnc_ports: std::sync::Mutex::new(HashMap::new()),
            mesh: None,
        }
    }

    /// Configure a mesh VPN provider for every desktop spawned by this driver.
    pub fn with_mesh(mut self, mesh: MeshConfig) -> Self {
        self.mesh = Some(mesh);
        self
    }

    /// Convenience constructor that builds the substrate from an Incus base URL.
    pub fn from_url(
        base_url: impl Into<String>,
        vnc_host: impl Into<String>,
    ) -> Result<Self, SubstrateError> {
        let substrate = Arc::new(IncusSubstrate::new(base_url)?);
        Ok(Self::new(substrate, vnc_host))
    }

    /// Build a driver from a comma-separated list of Incus URLs.
    pub fn from_urls(
        urls: &[String],
        fallback_vnc_host: impl Into<String>,
    ) -> Result<Self, SubstrateError> {
        let mut hosts = Vec::with_capacity(urls.len());
        for url in urls {
            let substrate = Arc::new(IncusSubstrate::new(url)?);
            hosts.push(Arc::new(IncusHost::new(url, substrate)));
        }
        Ok(Self::from_pool(
            IncusHostPool::new(hosts),
            fallback_vnc_host,
        ))
    }

    /// Access the underlying pool for dynamic host management.
    pub fn pool(&self) -> &Arc<IncusHostPool> {
        &self.pool
    }

    /// Add a new Incus host to the live pool.
    pub fn add_host(&self, url: impl Into<String>, substrate: Arc<IncusSubstrate>) {
        let url = url.into();
        info!(url = %url, "adding Incus host to driver pool");
        self.pool.add_host(Arc::new(IncusHost::new(url, substrate)));
    }

    /// Remove an Incus host from the live pool.
    pub fn remove_host(&self, url: &str) -> bool {
        self.pool.remove_host(url)
    }

    /// Update capacity metadata for a host.
    pub fn set_host_capacity(&self, url: &str, total_mb: u64, used_mb: u64) {
        self.pool.set_host_capacity(url, total_mb, used_mb);
    }

    /// Update the set of cached image aliases for a host.
    pub fn set_host_images(&self, url: &str, images: Vec<String>) {
        self.pool.set_host_images(url, images);
    }

    /// URLs of all configured hosts.
    pub fn host_urls(&self) -> Vec<String> {
        self.pool.hosts().iter().map(|h| h.url.clone()).collect()
    }

    /// Scan every configured Incus host for existing VNC proxy ports and seed
    /// the allocator so new spawns do not retry already-bound ports. This should
    /// be called once at service startup before the driver accepts provision
    /// requests.
    pub async fn recover_ports(&self) {
        let mut max_port: u16 = PROXY_PORT_BASE as u16;
        let mut recovered = 0usize;
        for host in self.pool.hosts() {
            let names = match host.substrate.list_instance_names().await {
                Ok(names) => names,
                Err(e) => {
                    warn!(host = %host.url, error = %e, "failed to list Incus instances for port recovery");
                    continue;
                }
            };
            for native_id in names {
                match host.substrate.get_config(&native_id).await {
                    Ok(config) => {
                        if let Some(port) = parse_vnc_port_from_config(&config) {
                            self.vnc_ports.lock().unwrap().insert(native_id, port);
                            if port > max_port {
                                max_port = port;
                            }
                            recovered += 1;
                        }
                    }
                    Err(e) => {
                        warn!(native_id, error = %e, "failed to read instance config during port recovery");
                    }
                }
            }
        }
        if recovered > 0 {
            // Start allocating just past the highest recovered port.
            let next = (max_port as u32).saturating_add(1).max(PROXY_PORT_BASE);
            self.next_port.store(next, Ordering::Relaxed);
            info!(recovered, next_port = next, "recovered existing VNC proxy ports");
        }
    }

    /// Access the underlying Incus substrate (used by examples/tests to pull files).
    pub fn substrate(&self) -> Arc<IncusSubstrate> {
        self.pool.hosts()[0].substrate.clone()
    }

    /// Allocate and configure a proxy device that forwards a host port to the
    /// guest's x11vnc port.
    async fn expose_vnc(
        &self,
        substrate: &Arc<IncusSubstrate>,
        native_id: &str,
    ) -> Result<u16, DriverError> {
        for _ in 0..128 {
            let port = self.next_port.fetch_add(1, Ordering::Relaxed) as u16;
            let listen = format!("tcp:0.0.0.0:{}", port);
            let connect = format!("tcp:127.0.0.1:{}", VNC_CONTAINER_PORT);

            match substrate
                .add_proxy_device(native_id, VNC_DEVICE, &listen, &connect)
                .await
            {
                Ok(()) => {
                    self.vnc_ports
                        .lock()
                        .unwrap()
                        .insert(native_id.to_string(), port);
                    return Ok(port);
                }
                Err(e) => {
                    warn!(native_id, port, error = %e, "failed to add VNC proxy device, trying next port");
                }
            }
        }
        Err(DriverError::InternalError {
            message: "could not allocate a VNC proxy port".to_string(),
        })
    }

    fn port_for(&self, native_id: &str) -> Option<u16> {
        self.vnc_ports.lock().unwrap().get(native_id).copied()
    }

    /// Allocate and configure a proxy device that forwards an arbitrary host
    /// port to a guest port. Returns the allocated host port.
    pub async fn expose_port(
        &self,
        native_id: &str,
        device_name: &str,
        guest_port: u16,
    ) -> Result<u16, DriverError> {
        // expose_port is called from outside the driver lifecycle where we do not
        // know the owning host; default to the first host. Multi-host callers
        // should route through the handle-aware APIs instead.
        let substrate = &self.pool.hosts()[0].substrate;
        self.expose_port_on(substrate, native_id, device_name, guest_port)
            .await
    }

    /// Allocate a proxy device on a specific host's substrate.
    async fn expose_port_on(
        &self,
        substrate: &Arc<IncusSubstrate>,
        native_id: &str,
        device_name: &str,
        guest_port: u16,
    ) -> Result<u16, DriverError> {
        for _ in 0..128 {
            let port = self.next_port.fetch_add(1, Ordering::Relaxed) as u16;
            let listen = format!("tcp:0.0.0.0:{}", port);
            let connect = format!("tcp:127.0.0.1:{}", guest_port);

            match substrate
                .add_proxy_device(native_id, device_name, &listen, &connect)
                .await
            {
                Ok(()) => return Ok(port),
                Err(e) => {
                    warn!(native_id, port, error = %e, "failed to add proxy device, trying next port");
                }
            }
        }
        Err(DriverError::InternalError {
            message: format!(
                "could not allocate a proxy port for {}:{}",
                native_id, guest_port
            ),
        })
    }

    /// Reuse an existing proxy device that forwards to `guest_port`, if one is
    /// already configured on the instance. Returns the host listen port.
    async fn existing_proxy_port(
        &self,
        substrate: &Arc<IncusSubstrate>,
        native_id: &str,
        guest_port: u16,
    ) -> Option<u16> {
        match substrate.get_config(native_id).await {
            Ok(config) => parse_proxy_port_for_guest(&config, guest_port),
            Err(e) => {
                warn!(native_id, error = %e, "failed to read instance config looking for proxy device");
                None
            }
        }
    }

    /// Poll the substrate until the instance reports `Running`.
    async fn wait_for_running_state(
        &self,
        substrate: &Arc<IncusSubstrate>,
        native_id: &str,
    ) -> Result<(), DriverError> {
        for attempt in 0..120 {
            match substrate.get(native_id).await {
                Ok(handle) if handle.state == crate::substrate::ComputerState::Running => {
                    info!(native_id, attempt, "instance reached Running");
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(native_id, attempt, error = %e, "waiting for instance state");
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Err(DriverError::InternalError {
            message: format!("instance {} did not reach Running in time", native_id),
        })
    }

    fn substrate_for(&self, handle: &ExecutionHandle) -> Arc<IncusSubstrate> {
        self.pool.host_for_handle(handle).substrate.clone()
    }

    /// Hostname clients should use to reach proxy/VNC ports for this host.
    /// Single-host pools built by `from_url` carry the synthetic pool name
    /// ("legacy") as the host url, and `IncusHost::new` derives vnc_host from
    /// that name — which clients cannot resolve. Fall back to the
    /// driver-level vnc host (e.g. INCUS_VNC_HOST) in that case.
    fn reachable_host(&self, host: &IncusHost) -> String {
        if host.url.starts_with("http://") || host.url.starts_with("https://") {
            host.vnc_host.clone()
        } else {
            self.vnc_host.clone()
        }
    }

    fn bot_id_from_tenant(tenant: &TenantId) -> String {
        tenant
            .0
            .strip_prefix("bot-")
            .or_else(|| tenant.0.strip_prefix("user-"))
            .unwrap_or(&tenant.0)
            .to_string()
    }

    fn map_spec(&self, spec: &SpawnSpec, native_id: &str) -> ComputerSpec {
        let mut env = spec.env.env_vars.clone();
        env.insert(
            "ALLTERNIT_BOT_ID".to_string(),
            Self::bot_id_from_tenant(&spec.tenant),
        );
        if let Some(mesh) = &self.mesh {
            for (k, v) in mesh.guest_env() {
                env.insert(k, v);
            }
        }

        let profiles: Vec<String> = std::env::var("INCUS_DESKTOP_PROFILES")
            .unwrap_or_else(|_| "default".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        ComputerSpec {
            name: native_id.to_string(),
            os: "linux".to_string(),
            image: normalize_image(&spec.env.image),
            cpu_cores: (spec.resources.cpu_millis / 1000).max(1),
            memory_mb: spec.resources.memory_mib.max(512),
            disk_mb: spec.resources.disk_mib.unwrap_or(20_480),
            env,
            profiles,
        }
    }
}

fn normalize_image(image: &str) -> String {
    match image {
        "" | "ubuntu-24.04-desktop" | "ubuntu-24.04" => "local:allternit-desktop".to_string(),
        "ubuntu/24.04/cloud" => "images:ubuntu/24.04/cloud".to_string(),
        other if other.contains(':') => other.to_string(),
        other => format!("local:{}", other),
    }
}

fn map_error(e: SubstrateError) -> DriverError {
    match e {
        SubstrateError::NotFound(id) => DriverError::NotFound { id },
        SubstrateError::Timeout => DriverError::ExecTimeout { timeout: 30 },
        other => DriverError::InternalError {
            message: other.to_string(),
        },
    }
}

#[async_trait]
impl ExecutionDriver for IncusDriver {
    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities {
            resize: true,
            clone: true,
            driver_type: DriverType::Container,
            isolation: IsolationLevel::Standard,
            max_resources: ResourceSpec {
                cpu_millis: 8000,
                memory_mib: 32_768,
                disk_mib: Some(200_000),
                network_egress_kib: None,
                gpu_count: None,
            },
            supported_env_specs: vec![EnvSpecType::Oci],
            features: DriverFeatures {
                snapshot: false,
                live_restore: false,
                gpu: false,
                prewarm: false,
            },
        }
    }

    async fn spawn(&self, spec: SpawnSpec) -> Result<ExecutionHandle, DriverError> {
        let bot_id = Self::bot_id_from_tenant(&spec.tenant);
        // Incus instance names must be <= 63 chars and contain only
        // alphanumeric characters and hyphens. Prefix + uuid is 48 chars,
        // leaving 15 for the owner id. Owner ids can contain other characters
        // (Clerk user ids carry underscores, e.g. "user_2kQ..."), and Incus
        // rejects the whole create call otherwise — map them to hyphens.
        // User-owned computers are shared across bots.
        let bot_suffix: String = bot_id
            .chars()
            .take(15)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let kind = if spec.tenant.0.starts_with("user-") {
            "user"
        } else {
            "bot"
        };
        let native_id = format!(
            "allternit-{}-{}-{}",
            kind,
            bot_suffix,
            uuid::Uuid::new_v4().simple()
        );
        let computer_spec = self.map_spec(&spec, &native_id);
        let memory_mib = computer_spec.memory_mb;
        let image_alias = normalize_image_alias(&computer_spec.image);

        let host = self
            .pool
            .select_for_spawn(memory_mib, image_alias.as_deref())
            .map_err(map_error)?;
        let substrate = host.substrate.clone();
        let host_url = host.url.clone();
        let vnc_host = host.vnc_host.clone();

        info!(bot_id, native_id, host = %host_url, image = %computer_spec.image, "provisioning Incus desktop");

        let handle = substrate
            .create(computer_spec.clone())
            .await
            .map_err(map_error)?;
        let native_id = handle.native_id;

        // Start the instance and wait until the guest reports Running.
        let _ = substrate.start(&native_id).await.map_err(map_error)?;
        self.wait_for_running_state(&substrate, &native_id).await?;

        // Expose the in-guest x11vnc port on the Incus host.
        let vnc_port = self.expose_vnc(&substrate, &native_id).await?;

        let mut driver_info = HashMap::new();
        driver_info.insert("native_id".to_string(), native_id.clone());
        driver_info.insert("host".to_string(), vnc_host);
        driver_info.insert("host_url".to_string(), host_url);
        driver_info.insert("vnc_port".to_string(), vnc_port.to_string());
        driver_info.insert("provider".to_string(), "incus".to_string());

        Ok(ExecutionHandle {
            id: ExecutionId::new(),
            tenant: spec.tenant,
            driver_info,
            env_spec: spec.env,
        })
    }

    async fn pause_vm(&self, handle: &ExecutionHandle) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .stop(native_id)
            .await
            .map_err(map_error)
            .map(|_| ())
    }

    async fn resume_vm(&self, handle: &ExecutionHandle) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .start(native_id)
            .await
            .map_err(map_error)
            .map(|_| ())
    }

    async fn exec(
        &self,
        handle: &ExecutionHandle,
        cmd: CommandSpec,
    ) -> Result<ExecResult, DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        let started = std::time::Instant::now();
        let result = substrate
            .exec(native_id, &cmd.command, &cmd.env_vars)
            .await
            .map_err(map_error)?;
        Ok(ExecResult {
            exit_code: result.exit_code,
            stdout: Some(result.stdout.into_bytes()),
            stderr: Some(result.stderr.into_bytes()),
            duration_ms: started.elapsed().as_millis() as u64,
            resource_usage: ResourceConsumption::default(),
        })
    }

    async fn stream_logs(
        &self,
        _handle: &ExecutionHandle,
    ) -> Result<Vec<allternit_driver_interface::LogEntry>, DriverError> {
        Err(DriverError::NotSupported {
            feature: "stream_logs".to_string(),
        })
    }

    async fn get_artifacts(
        &self,
        _handle: &ExecutionHandle,
    ) -> Result<Vec<allternit_driver_interface::Artifact>, DriverError> {
        Ok(vec![])
    }

    async fn destroy(&self, handle: &ExecutionHandle) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        // Stop first; Incus refuses to delete a running container.
        let _ = substrate.stop(native_id).await.map_err(map_error);
        substrate.delete(native_id).await.map_err(map_error)?;
        self.vnc_ports.lock().unwrap().remove(native_id);
        Ok(())
    }

    async fn get_consumption(
        &self,
        _handle: &ExecutionHandle,
    ) -> Result<ResourceConsumption, DriverError> {
        Ok(ResourceConsumption::default())
    }

    async fn get_receipt(&self, _handle: &ExecutionHandle) -> Result<Option<Receipt>, DriverError> {
        Ok(None)
    }

    async fn health_check(&self) -> Result<DriverHealth, DriverError> {
        let mut capabilities = vec!["linux".to_string()];
        if std::path::Path::new("/dev/kvm").exists() {
            capabilities.push("windows".to_string());
        }
        Ok(DriverHealth {
            healthy: true,
            message: Some(format!(
                "Incus driver ready ({} host{})",
                self.pool.len(),
                if self.pool.len() == 1 { "" } else { "s" }
            )),
            active_executions: self.vnc_ports.lock().unwrap().len() as u32,
            available_capacity: self.capabilities().max_resources,
            capabilities,
        })
    }

    fn supports_desktop(&self) -> bool {
        true
    }

    async fn get_desktop_endpoint(
        &self,
        handle: &ExecutionHandle,
    ) -> Result<Option<DesktopEndpoint>, DriverError> {
        let native_id = native_id(handle)?;
        let port = self
            .port_for(native_id)
            .ok_or_else(|| DriverError::InternalError {
                message: format!("no VNC port allocated for {}", native_id),
            })?;
        let vnc_host = self.reachable_host(&self.pool.host_for_handle(handle));
        Ok(Some(DesktopEndpoint {
            url: format!("tcp://{}:{}", vnc_host, port),
            protocol: DesktopProtocol::Vnc,
            token: Some(self.vnc_password.clone()),
        }))
    }

    async fn get_desktop_endpoint_by_native_id(
        &self,
        sandbox_id: &str,
    ) -> Result<Option<DesktopEndpoint>, DriverError> {
        if let Some(port) = self.port_for(sandbox_id) {
            return Ok(Some(DesktopEndpoint {
                url: format!("tcp://{}:{}", self.vnc_host, port),
                protocol: DesktopProtocol::Vnc,
                token: Some(self.vnc_password.clone()),
            }));
        }
        // Not in memory — recover the port from the instance config. We don't
        // know which host owns the VM, so ask each one. Only when every host
        // answers "not found" is the instance gone (NotFound); any other
        // failure stays inconclusive (Ok(None)).
        let mut all_not_found = true;
        for host in self.pool.hosts() {
            match host.substrate.get_config(sandbox_id).await {
                Ok(config) => {
                    all_not_found = false;
                    if let Some(port) = parse_vnc_port_from_config(&config) {
                        self.vnc_ports
                            .lock()
                            .unwrap()
                            .insert(sandbox_id.to_string(), port);
                        return Ok(Some(DesktopEndpoint {
                            url: format!("tcp://{}:{}", self.reachable_host(&host), port),
                            protocol: DesktopProtocol::Vnc,
                            token: Some(self.vnc_password.clone()),
                        }));
                    }
                }
                Err(SubstrateError::NotFound(_)) => {}
                Err(e) => {
                    all_not_found = false;
                    warn!(sandbox_id, host = %host.url, error = %e, "failed to recover VNC port from Incus");
                }
            }
        }
        if all_not_found {
            return Err(DriverError::NotFound { id: sandbox_id.to_string() });
        }
        Ok(None)
    }

    async fn register_native_sandbox(
        &self,
        native_id: &str,
        _agent_id: &str,
        host: &str,
    ) -> Result<(), DriverError> {
        // Recover the port from the remote Incus config on the host that owns this VM.
        let host_url = self
            .pool
            .hosts()
            .iter()
            .find(|h| h.url == host || h.vnc_host == host)
            .map(|h| h.url.clone())
            .unwrap_or_else(|| self.pool.hosts()[0].url.clone());
        let substrate = self
            .pool
            .hosts()
            .iter()
            .find(|h| h.url == host_url)
            .map(|h| h.substrate.clone())
            .unwrap_or_else(|| self.pool.hosts()[0].substrate.clone());
        match substrate.get_config(native_id).await {
            Ok(config) => {
                if let Some(port) = parse_vnc_port_from_config(&config) {
                    self.vnc_ports
                        .lock()
                        .unwrap()
                        .insert(native_id.to_string(), port);
                }
            }
            Err(e) => return Err(map_error(e)),
        }
        Ok(())
    }

    async fn pull_file(
        &self,
        handle: &ExecutionHandle,
        path: &str,
    ) -> Result<Vec<u8>, DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate.pull_file(native_id, path).await.map_err(map_error)
    }

    async fn push_file(
        &self,
        handle: &ExecutionHandle,
        path: &str,
        content: Vec<u8>,
    ) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .push_file(native_id, path, content)
            .await
            .map_err(map_error)
    }

    async fn resize_vm(
        &self,
        handle: &ExecutionHandle,
        resources: &ResourceSpec,
    ) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        let current = if resources.disk_mib.is_some() {
            substrate.get_config(native_id).await.map_err(map_error)?
        } else {
            serde_json::json!({})
        };
        let patch = resize_patch(resources, &current)?;
        substrate
            .patch_config(native_id, patch)
            .await
            .map_err(map_error)
    }

    async fn clone_vm(
        &self,
        handle: &ExecutionHandle,
        new_native_id: &str,
    ) -> Result<ExecutionHandle, DriverError> {
        let source = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        let current = substrate.get_config(source).await.map_err(map_error)?;
        let current = current.get("metadata").unwrap_or(&current);
        let mut limits = serde_json::Map::new();
        for key in ["limits.cpu", "limits.memory"] {
            if let Some(value) = current["config"].get(key) {
                limits.insert(key.into(), value.clone());
            }
        }
        let snapshot = format!("clone-{}", uuid::Uuid::new_v4().simple());
        substrate
            .create_snapshot(source, &snapshot, true)
            .await
            .map_err(map_error)?;
        substrate
            .clone_from_snapshot(source, &snapshot, new_native_id, limits.into())
            .await
            .map_err(map_error)?;
        // The snapshot is deliberately retained as the clone's restore point.
        // A copied proxy retains the source listen port. Replace it before starting.
        let vnc_port = match self.expose_vnc(&substrate, new_native_id).await {
            Ok(port) => port,
            Err(error) => {
                if let Err(cleanup) = substrate.delete(new_native_id).await {
                    warn!(%cleanup, new_native_id, "failed to clean up clone after proxy failure");
                }
                return Err(error);
            }
        };
        if let Err(error) = substrate.start(new_native_id).await {
            let _ = substrate.delete(new_native_id).await;
            return Err(map_error(error));
        }
        let mut cloned = handle.clone();
        cloned.id = ExecutionId::new();
        cloned
            .driver_info
            .insert("native_id".into(), new_native_id.into());
        cloned
            .driver_info
            .insert("vnc_port".into(), vnc_port.to_string());
        Ok(cloned)
    }

    async fn clone_from_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
        new_native_id: &str,
        resources: Option<&ResourceSpec>,
        env: &HashMap<String, String>,
    ) -> Result<ExecutionHandle, DriverError> {
        let source = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        // The create-from-snapshot call takes instance config inline: limits
        // requested by the caller override the snapshot's inherited ones, and
        // env vars land as `environment.*` keys (container guests pick these
        // up instance-wide, replacing identity vars baked into the golden
        // image). Disk size is inherited — resizing needs a device patch.
        let mut config = serde_json::Map::new();
        if let Some(resources) = resources {
            if resources.cpu_millis > 0 {
                config.insert(
                    "limits.cpu".into(),
                    (resources.cpu_millis / 1000).to_string().into(),
                );
            }
            if resources.memory_mib > 0 {
                config.insert(
                    "limits.memory".into(),
                    format!("{}MiB", resources.memory_mib).into(),
                );
            }
        }
        for (key, value) in env {
            config.insert(format!("environment.{key}"), value.clone().into());
        }
        substrate
            .clone_from_snapshot(source, snapshot_id, new_native_id, config.into())
            .await
            .map_err(map_error)?;
        // A copied proxy retains the source listen port. Replace it before starting.
        let vnc_port = match self.expose_vnc(&substrate, new_native_id).await {
            Ok(port) => port,
            Err(error) => {
                if let Err(cleanup) = substrate.delete(new_native_id).await {
                    warn!(%cleanup, new_native_id, "failed to clean up clone after proxy failure");
                }
                return Err(error);
            }
        };
        if let Err(error) = substrate.start(new_native_id).await {
            let _ = substrate.delete(new_native_id).await;
            return Err(map_error(error));
        }
        let mut cloned = handle.clone();
        cloned.id = ExecutionId::new();
        cloned
            .driver_info
            .insert("native_id".into(), new_native_id.into());
        cloned
            .driver_info
            .insert("vnc_port".into(), vnc_port.to_string());
        Ok(cloned)
    }

    async fn create_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
        stateful: bool,
    ) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .create_snapshot(native_id, snapshot_id, stateful)
            .await
            .map_err(map_error)
    }

    async fn restore_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
    ) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .restore_snapshot(native_id, snapshot_id)
            .await
            .map_err(map_error)
    }

    async fn delete_snapshot(
        &self,
        handle: &ExecutionHandle,
        snapshot_id: &str,
    ) -> Result<(), DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .delete_snapshot(native_id, snapshot_id)
            .await
            .map_err(map_error)
    }

    async fn list_snapshots(
        &self,
        handle: &ExecutionHandle,
    ) -> Result<Vec<SnapshotInfo>, DriverError> {
        let native_id = native_id(handle)?;
        let substrate = self.substrate_for(handle);
        substrate
            .list_snapshots(native_id)
            .await
            .map_err(map_error)
            .map(|v| {
                v.into_iter()
                    .map(|s| SnapshotInfo {
                        id: s.id,
                        created_at: s.created_at,
                        stateful: s.stateful,
                    })
                    .collect()
            })
    }

    async fn guest_service_url(
        &self,
        handle: &ExecutionHandle,
        guest_port: u16,
    ) -> Result<String, DriverError> {
        let native_id = native_id(handle)?;
        let host = self.pool.host_for_handle(handle);
        let reachable = self.reachable_host(&host);
        // Reuse an existing proxy device for this guest port when one exists.
        if let Some(port) = self
            .existing_proxy_port(&host.substrate, native_id, guest_port)
            .await
        {
            return Ok(format!("http://{}:{}", reachable, port));
        }
        let device_name = format!("svc{}", guest_port);
        let port = self
            .expose_port_on(&host.substrate, native_id, &device_name, guest_port)
            .await?;
        Ok(format!("http://{}:{}", reachable, port))
    }
}

fn native_id(handle: &ExecutionHandle) -> Result<&str, DriverError> {
    handle
        .driver_info
        .get("native_id")
        .map(|s| s.as_str())
        .ok_or_else(|| DriverError::InternalError {
            message: "missing native_id in execution handle".to_string(),
        })
}

fn parse_vnc_port_from_config(config: &serde_json::Value) -> Option<u16> {
    // Incus returns the instance config wrapped in either `metadata` or `data`.
    let payload = config
        .get("metadata")
        .or_else(|| config.get("data"))
        .or_else(|| Some(config));
    let listen = payload
        .and_then(|c| c.get("devices"))
        .and_then(|d| d.get(VNC_DEVICE))
        .and_then(|v| v.get("listen"))
        .and_then(|l| l.as_str())?;
    listen.rsplit(':').next()?.parse().ok()
}

/// Find an existing proxy device whose `connect` targets `guest_port` and
/// return its host-side `listen` port. Incus wraps instance config in
/// `metadata` (or `data` on older daemons).
fn parse_proxy_port_for_guest(config: &serde_json::Value, guest_port: u16) -> Option<u16> {
    let payload = config
        .get("metadata")
        .or_else(|| config.get("data"))
        .or_else(|| Some(config));
    let devices = payload?.get("devices")?.as_object()?;
    for device in devices.values() {
        if device.get("type").and_then(|t| t.as_str()) != Some("proxy") {
            continue;
        }
        let connect = device
            .get("connect")
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        if port_from_addr(connect) == Some(guest_port) {
            if let Some(port) = device
                .get("listen")
                .and_then(|l| l.as_str())
                .and_then(port_from_addr)
            {
                return Some(port);
            }
        }
    }
    None
}

/// Extract the port from a `tcp:host:port` / `host:port` address string.
fn port_from_addr(addr: &str) -> Option<u16> {
    addr.rsplit(':').next()?.parse().ok()
}

/// Strip the `local:` / `images:` prefix so we can compare cached aliases.
fn normalize_image_alias(image: &str) -> Option<String> {
    let alias = image
        .strip_prefix("local:")
        .or_else(|| image.strip_prefix("images:"))
        .unwrap_or(image);
    if alias.is_empty() {
        None
    } else {
        Some(alias.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn incus_host(found: Option<u16>, status: usize) -> (mockito::ServerGuard, Arc<IncusSubstrate>) {
        let mut server = mockito::Server::new_async().await;
        let body = match found {
            Some(port) => serde_json::json!({ "metadata": { "devices": { VNC_DEVICE: { "listen": format!("tcp:0.0.0.0:{port}") } } } }),
            None => serde_json::json!({ "error": "Instance not found" }),
        };
        server
            .mock("GET", mockito::Matcher::Regex("^/1.0/instances/".into()))
            .with_status(status)
            .with_body(body.to_string())
            .create_async()
            .await;
        let substrate = Arc::new(IncusSubstrate::new(server.url()).unwrap());
        (server, substrate)
    }

    #[tokio::test]
    async fn desktop_lookup_asks_every_host_and_uses_the_owning_hosts_address() {
        let (_a, missing) = incus_host(None, 404).await;
        let (b, owner) = incus_host(Some(5911), 200).await;
        let driver = IncusDriver::new(missing, "legacy-vnc");
        driver.add_host(b.url(), owner);
        let ep = driver.get_desktop_endpoint_by_native_id("vm-1").await.unwrap().unwrap();
        assert_eq!(ep.url, format!("tcp://{}:5911", b.host_with_port()));
    }

    #[tokio::test]
    async fn desktop_lookup_is_not_found_only_when_every_host_says_so() {
        let (_a, a) = incus_host(None, 404).await;
        let (b, b_sub) = incus_host(None, 404).await;
        let driver = IncusDriver::new(a, "legacy-vnc");
        driver.add_host(b.url(), b_sub);
        assert!(matches!(
            driver.get_desktop_endpoint_by_native_id("vm-1").await,
            Err(DriverError::NotFound { .. })
        ));

        // One host failing for another reason keeps it inconclusive.
        let (_c, c) = incus_host(None, 404).await;
        let (d, d_sub) = incus_host(None, 500).await;
        let driver = IncusDriver::new(c, "legacy-vnc");
        driver.add_host(d.url(), d_sub);
        assert!(matches!(driver.get_desktop_endpoint_by_native_id("vm-1").await, Ok(None)));
    }

    #[test]
    fn normalize_image_maps_defaults() {
        assert_eq!(normalize_image(""), "local:allternit-desktop");
        assert_eq!(
            normalize_image("ubuntu-24.04-desktop"),
            "local:allternit-desktop"
        );
        assert_eq!(
            normalize_image("ubuntu/24.04/cloud"),
            "images:ubuntu/24.04/cloud"
        );
        assert_eq!(normalize_image("images:alpine/edge"), "images:alpine/edge");
        assert_eq!(
            normalize_image("allternit-desktop"),
            "local:allternit-desktop"
        );
    }

    #[test]
    fn parse_vnc_port_extracts_last_colon_value() {
        let config = serde_json::json!({
            "data": {
                "devices": {
                    "vnc": { "listen": "tcp:0.0.0.0:35900" }
                }
            }
        });
        assert_eq!(parse_vnc_port_from_config(&config), Some(35900));
    }
}

/// Build a limits patch without overwriting unrelated devices or root disk fields.
fn resize_patch(
    resources: &ResourceSpec,
    current: &serde_json::Value,
) -> Result<serde_json::Value, DriverError> {
    let mut limits = serde_json::Map::new();
    if resources.cpu_millis > 0 {
        limits.insert(
            "limits.cpu".into(),
            (resources.cpu_millis / 1000).to_string().into(),
        );
    }
    if resources.memory_mib > 0 {
        limits.insert(
            "limits.memory".into(),
            format!("{}MiB", resources.memory_mib).into(),
        );
    }
    let mut patch = serde_json::json!({"config": limits});
    if let Some(disk) = resources.disk_mib {
        let current = current.get("metadata").unwrap_or(current);
        let mut devices = current["devices"].as_object().cloned().unwrap_or_default();
        let root = devices
            .values_mut()
            .find(|d| d["type"] == "disk" && d["path"] == "/")
            .ok_or_else(|| DriverError::NotSupported {
                feature: "disk resize: instance has no root disk device entry".into(),
            })?;
        root["size"] = format!("{disk}MiB").into();
        patch["devices"] = devices.into();
    }
    Ok(patch)
}

#[cfg(test)]
mod phase_two_tests {
    use super::*;
    #[test]
    fn resize_limits_and_disk_preserve_device_config() {
        let current = serde_json::json!({"metadata":{"devices":{"root":{"type":"disk","path":"/","pool":"default","size":"20480MiB"},"eth0":{"type":"nic","network":"incusbr0"}}}});
        let patch = resize_patch(
            &ResourceSpec {
                cpu_millis: 4000,
                memory_mib: 8192,
                disk_mib: Some(40960),
                ..Default::default()
            },
            &current,
        )
        .unwrap();
        assert_eq!(patch["config"]["limits.cpu"], "4");
        assert_eq!(patch["config"]["limits.memory"], "8192MiB");
        assert_eq!(patch["devices"]["root"]["size"], "40960MiB");
        assert_eq!(patch["devices"]["root"]["pool"], "default");
        assert_eq!(
            patch["devices"]["eth0"],
            current["metadata"]["devices"]["eth0"]
        );
        let patch = resize_patch(
            &ResourceSpec {
                memory_mib: 16384,
                ..Default::default()
            },
            &serde_json::json!({}),
        )
        .unwrap();
        assert!(patch.get("devices").is_none());
        assert!(patch["config"].get("limits.cpu").is_none());
        assert!(matches!(
            resize_patch(
                &ResourceSpec {
                    disk_mib: Some(40960),
                    ..Default::default()
                },
                &serde_json::json!({})
            ),
            Err(DriverError::NotSupported { .. })
        ));
    }
}

#[cfg(test)]
mod phase_two_http_tests {
    use super::*;
    use crate::substrate::HttpClient;
    use serde_json::{json, Value};
    use std::sync::Mutex;
    type Calls = Arc<Mutex<Vec<(reqwest::Method, String, Option<Value>)>>>;
    struct ScriptedClient {
        responses: Mutex<std::collections::VecDeque<(u16, Value)>>,
        calls: Calls,
    }
    #[async_trait]
    impl HttpClient for ScriptedClient {
        async fn request(
            &self,
            method: reqwest::Method,
            path: &str,
            body: Option<Value>,
        ) -> Result<(u16, Value), SubstrateError> {
            self.calls.lock().unwrap().push((method, path.into(), body));
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
    fn driver(responses: Vec<(u16, Value)>) -> (IncusDriver, Calls) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let substrate = IncusSubstrate::with_client(Box::new(ScriptedClient {
            responses: Mutex::new(responses.into()),
            calls: calls.clone(),
        }));
        (IncusDriver::new(Arc::new(substrate), "127.0.0.1"), calls)
    }
    fn handle() -> ExecutionHandle {
        ExecutionHandle {
            id: ExecutionId::new(),
            tenant: TenantId::new("user-test").unwrap(),
            driver_info: HashMap::from([
                ("native_id".into(), "source".into()),
                ("provider".into(), "incus".into()),
            ]),
            env_spec: Default::default(),
        }
    }
    #[tokio::test]
    async fn tart_keeps_default_resize_and_clone_unsupported() {
        let tart = crate::tart::TartDriver::new("http://127.0.0.1:1", "localhost");
        assert!(!tart.capabilities().resize && !tart.capabilities().clone);
        assert!(matches!(
            tart.resize_vm(&handle(), &ResourceSpec::standard()).await,
            Err(DriverError::NotSupported { .. })
        ));
        assert!(matches!(
            tart.clone_vm(&handle(), "copy").await,
            Err(DriverError::NotSupported { .. })
        ));
    }

    #[tokio::test]
    async fn clone_snapshot_copy_limits_new_proxy_and_start() {
        let (driver, calls) = driver(vec![
            (
                200,
                json!({"metadata":{"config":{"limits.cpu":"4","limits.memory":"8192MiB","unrelated":"not-copied"}}}),
            ),
            (202, json!({"operation":"/1.0/operations/snapshot"})),
            (200, json!({"metadata":{"status":"Success"}})),
            (202, json!({"operation":"/1.0/operations/clone"})),
            (200, json!({"metadata":{"status":"Success"}})),
            (200, json!({})), // new proxy
            (202, json!({"operation":"/1.0/operations/start"})),
            (200, json!({"metadata":{"status":"Success"}})),
            (200, json!({"metadata":{"name":"copy","status":"Running"}})),
        ]);
        let source = handle();
        let cloned = driver.clone_vm(&source, "copy").await.unwrap();
        assert_ne!(cloned.id, source.id);
        assert_eq!(cloned.driver_info["native_id"], "copy");
        assert!(cloned.driver_info.contains_key("vnc_port"));
        let calls = calls.lock().unwrap();
        assert_eq!(calls[1].0, reqwest::Method::POST);
        assert_eq!(calls[1].1, "/1.0/instances/source/snapshots");
        let snapshot = calls[1].2.as_ref().unwrap();
        assert_eq!(snapshot["stateful"], true);
        assert!(snapshot["name"].as_str().unwrap().starts_with("clone-"));
        assert_eq!(calls[3].1, "/1.0/instances");
        let create = calls[3].2.as_ref().unwrap();
        assert_eq!(create["name"], "copy");
        assert_eq!(create["source"]["type"], "copy");
        assert_eq!(
            create["source"]["source"],
            format!("source/{}", snapshot["name"].as_str().unwrap())
        );
        assert_eq!(
            create["config"],
            json!({"limits.cpu":"4","limits.memory":"8192MiB"})
        );
        assert_eq!(calls[5].1, "/1.0/instances/copy");
        assert_eq!(calls[6].1, "/1.0/instances/copy/state");
        assert_eq!(calls[6].2.as_ref().unwrap()["action"], "start");
        assert!(!calls.iter().any(|(m, _, _)| m == reqwest::Method::DELETE));
    }
    #[tokio::test]
    async fn clone_snapshot_operation_failure_does_not_create_instance() {
        let (driver, calls) = driver(vec![
            (200, json!({"metadata":{"config":{}}})),
            (202, json!({"operation":"/1.0/operations/snapshot"})),
            (
                200,
                json!({"metadata":{"status":"Failure","err":"cannot checkpoint"}}),
            ),
        ]);
        assert!(driver.clone_vm(&handle(), "copy").await.is_err());
        assert_eq!(calls.lock().unwrap().len(), 3);
    }
    #[tokio::test]
    async fn resize_waits_for_operation_and_propagates_failure() {
        let (driver, calls) = driver(vec![
            (202, json!({"operation":"/1.0/operations/resize"})),
            (
                200,
                json!({"metadata":{"status":"Failure","err":"cannot change limits"}}),
            ),
        ]);
        assert!(driver
            .resize_vm(
                &handle(),
                &ResourceSpec {
                    cpu_millis: 8000,
                    ..Default::default()
                }
            )
            .await
            .is_err());
        let calls = calls.lock().unwrap();
        assert_eq!(calls[0].0, reqwest::Method::PATCH);
        assert_eq!(calls[0].2, Some(json!({"config":{"limits.cpu":"8"}})));
        assert_eq!(calls[1].1, "/1.0/operations/resize/wait?timeout=60");
    }

    #[tokio::test]
    async fn guest_service_url_reuses_existing_proxy_device() {
        let (driver, calls) = driver(vec![(
            200,
            json!({"metadata":{"devices":{
                "vnc": {"type":"proxy","listen":"tcp:0.0.0.0:35900","connect":"tcp:127.0.0.1:5900"},
                "svc6010": {"type":"proxy","listen":"tcp:0.0.0.0:36010","connect":"tcp:127.0.0.1:6010"}
            }}}),
        )]);
        // The reachable host comes from the driver-level fallback, not the
        // synthetic single-host pool name ("legacy").
        let url = driver
            .guest_service_url(&handle(), 6010)
            .await
            .expect("guest service url");
        assert_eq!(url, "http://127.0.0.1:36010");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, reqwest::Method::GET);
        assert_eq!(calls[0].1, "/1.0/instances/source");
    }

    #[tokio::test]
    async fn guest_service_url_allocates_proxy_device_when_missing() {
        let (driver, calls) = driver(vec![
            (
                200,
                json!({"metadata":{"devices":{
                    "vnc": {"type":"proxy","listen":"tcp:0.0.0.0:35900","connect":"tcp:127.0.0.1:5900"}
                }}}),
            ),
            (200, json!({})),
        ]);
        let url = driver
            .guest_service_url(&handle(), 6010)
            .await
            .expect("guest service url");
        assert_eq!(url, "http://127.0.0.1:30000");
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].0, reqwest::Method::PATCH);
        assert_eq!(calls[1].1, "/1.0/instances/source");
        let patch = calls[1].2.as_ref().unwrap();
        assert_eq!(patch["devices"]["svc6010"]["type"], "proxy");
        assert_eq!(patch["devices"]["svc6010"]["listen"], "tcp:0.0.0.0:30000");
        assert_eq!(patch["devices"]["svc6010"]["connect"], "tcp:127.0.0.1:6010");
    }
}
