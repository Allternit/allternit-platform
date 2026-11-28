//! Canonical AllternitOS Layer 1 contracts exposed to the Cloud commercial layer.
//!
//! This crate provides JSON-serializable Rust structs and optimizer traits for the
//! canonical schemas in `contracts/fabric-os/`. It is intentionally lightweight so
//! the Cloud donor repo can consume canonical types without pulling in the full
//! node-agent / runtime / harness dependency tree.
//!
//! The platform repo owns these contracts (`spec/Contracts/os/v1`). Cloud should import these types and stop defining
//! parallel view structs in `os_mapping.rs`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Money amount in minor units (e.g. USD cents).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Money {
    pub currency: String,
    pub minor_units: u64,
}

/// Minimum resource requirements used by the scheduler to filter offers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Requirements {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_vcpu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_memory_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_gpu_vram_mib: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_mib: Option<u64>,
}

/// Capability family + actions advertised by a resource class.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceClassCapability {
    pub capability: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

/// Canonical resource class / SKU.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceClass {
    pub id: String,
    pub version: String,
    pub kind: String,
    pub class: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub owner: String,
    pub reliability_tier: String,
    #[serde(default, skip_serializing_if = "Requirements::is_default")]
    pub requirements: Requirements,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retail_price_per_hour: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retail_price_per_request: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retail_price_per_token: Option<Money>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<ResourceClassCapability>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

impl Requirements {
    fn is_default(&self) -> bool {
        self.min_vcpu.is_none()
            && self.min_memory_mib.is_none()
            && self.min_gpu_vram_mib.is_none()
            && self.gpu_model.is_none()
            && self.network.is_none()
            && self.storage_mib.is_none()
    }
}

/// Concrete provider offer discovered by a substrate adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Offer {
    pub id: String,
    pub provider_kind: String,
    pub region: String,
    pub instance_type: String,
    pub resource_class_id: String,
    pub vcpu: u32,
    pub memory_mib: u64,
    pub gpu_vram_mib: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu_model: Option<String>,
    pub price_per_hour: u64,
    pub currency: String,
    pub reliability_score: f64,
    pub interruptible: bool,
    pub estimated_ready_secs: u64,
    pub available_until: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_metadata: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Record of where a Fabric resource actually ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    pub id: String,
    pub resource_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub offer_id: String,
    pub provider_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_resource_id: Option<String>,
    pub region: String,
    pub instance_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retail_price_per_hour: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_cost_per_hour: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retail_price_per_request: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_cost_per_request: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retail_price_per_token: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_cost_per_token: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hold_id: Option<String>,
    pub status: String,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub termination_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Work dispatched from the control plane to a Private Fabric node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub node_id: String,
    pub resource_id: String,
    pub kind: String,
    pub class: String,
    #[serde(default)]
    pub requested_vcpu: i64,
    #[serde(default)]
    pub requested_memory_mib: i64,
    #[serde(default)]
    pub requested_gpu_vram_mib: i64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Raw usage event emitted by a provider adapter or Fabric node daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageEvent {
    pub id: String,
    pub resource_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub event_type: String,
    pub quantity: f64,
    pub unit: String,
    pub measured_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub labels: HashMap<String, String>,
}

/// Input for recording a new usage event (no generated fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordUsageEvent {
    pub resource_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub event_type: String,
    pub quantity: f64,
    pub unit: String,
    pub measured_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_event_id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    pub labels: HashMap<String, String>,
}

/// CPU description in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CpuInfo {
    pub vendor: String,
    pub model: String,
    pub cores: u32,
    pub threads: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sockets: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_frequency_mhz: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub numa_nodes: Option<u32>,
}

/// Memory description in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub memory_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mhz: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_bandwidth_gbps: Option<f64>,
}

/// Storage device in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageInfo {
    pub id: String,
    pub storage_type: String,
    pub capacity_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequential_read_mbps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequential_write_mbps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endurance_class: Option<String>,
}

/// Network interface in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkInfo {
    pub id: String,
    pub network_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_mbps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rdma_capable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv4: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipv6: Option<String>,
}

/// Accelerator in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcceleratorInfo {
    pub id: String,
    pub vendor: String,
    pub model: String,
    pub accelerator_type: String,
    pub memory_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tensor_formats: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pcie: Option<PcieInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mig_capable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sr_iov_capable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vfio_capable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_tflops_fp16: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_bandwidth_gbps: Option<f64>,
}

/// PCIe location for an accelerator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcieInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bus: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
}

/// NUMA node in a node topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NumaNode {
    pub node_id: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu_cores: Vec<u32>,
    pub memory_bytes: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accelerator_ids: Vec<String>,
}

/// PCIe segment in a node topology.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcieSegment {
    pub segment: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub devices: Vec<String>,
}

/// Node topology description.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeTopology {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub numa_topology: Vec<NumaNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pcie_topology: Vec<PcieSegment>,
}

/// Hardware description in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeHardware {
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub storage: Vec<StorageInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub network: Vec<NetworkInfo>,
}

/// Software versions in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSoftware {
    pub fabric_os_version: String,
    pub kernel_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub libvirt_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containerd_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qemu_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wireguard_version: Option<String>,
}

/// Fabric identity and reachability in a node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeFabric {
    pub wireguard_public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fabric_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rack: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_token_hash: Option<String>,
}

/// Measured bandwidths for a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MeasuredBandwidth {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host_to_device_gbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_to_host_gbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_to_device_gbps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bandwidth_gbps: Option<f64>,
}

/// Health alert for a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthAlert {
    pub severity: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Health snapshot for a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NodeHealth {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alerts: Vec<HealthAlert>,
}

/// Worker advertised by a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostedWorker {
    pub worker_id: String,
    pub name: String,
    pub version: String,
    pub runtime_class: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_ref: Option<PackageRef>,
}

/// Function advertised by a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostedFunction {
    pub function_id: String,
    pub worker_id: String,
    pub name: String,
    pub capability: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
}

/// Capability advertised by a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostedCapability {
    pub capability: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub worker_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

/// Package reference for a hosted worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageRef {
    pub package_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub digest: String,
}

/// Workers, functions, and capabilities hosted by a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Workers {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workers: Vec<HostedWorker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub functions: Vec<HostedFunction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<HostedCapability>,
}

/// Canonical node capability record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeCapabilityRecord {
    pub schema_version: String,
    pub node_id: String,
    pub recorded_at: DateTime<Utc>,
    pub hardware: NodeHardware,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accelerators: Vec<AcceleratorInfo>,
    #[serde(default, skip_serializing_if = "NodeTopology::is_default")]
    pub topology: NodeTopology,
    pub software: NodeSoftware,
    pub fabric: NodeFabric,
    #[serde(default, skip_serializing_if = "MeasuredBandwidth::is_default")]
    pub measured_bandwidth: MeasuredBandwidth,
    #[serde(default, skip_serializing_if = "NodeHealth::is_default")]
    pub health: NodeHealth,
    #[serde(default, skip_serializing_if = "Workers::is_default")]
    pub workers: Workers,
}

impl NodeTopology {
    fn is_default(&self) -> bool {
        self.numa_topology.is_empty() && self.pcie_topology.is_empty()
    }
}

impl MeasuredBandwidth {
    fn is_default(&self) -> bool {
        self.host_to_device_gbps.is_none()
            && self.device_to_host_gbps.is_none()
            && self.device_to_device_gbps.is_none()
            && self.memory_bandwidth_gbps.is_none()
    }
}

impl NodeHealth {
    fn is_default(&self) -> bool {
        self.status.is_empty() && self.last_checked_at.is_none() && self.alerts.is_empty()
    }
}

impl Workers {
    fn is_default(&self) -> bool {
        self.workers.is_empty() && self.functions.is_empty() && self.capabilities.is_empty()
    }
}

pub mod optimizer;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_class_serializes_against_schema_shape() {
        let class = ResourceClass {
            id: "gpu.m".to_string(),
            version: "1.0.0".to_string(),
            kind: "gpu".to_string(),
            class: "m".to_string(),
            display_name: "GPU M".to_string(),
            description: None,
            owner: "prn_allternitos_system".to_string(),
            reliability_tier: "standard".to_string(),
            requirements: Requirements {
                min_vcpu: Some(8),
                min_memory_mib: Some(32768),
                min_gpu_vram_mib: Some(49152),
                gpu_model: None,
                network: Some(true),
                storage_mib: None,
            },
            retail_price_per_hour: Some(Money {
                currency: "USD".to_string(),
                minor_units: 129,
            }),
            retail_price_per_request: None,
            retail_price_per_token: None,
            capabilities: vec![ResourceClassCapability {
                capability: "model.generate".to_string(),
                actions: vec!["generate".to_string()],
            }],
            created_at: Utc::now(),
            updated_at: None,
            labels: HashMap::new(),
        };
        let json = serde_json::to_value(&class).unwrap();
        assert_eq!(json["id"], "gpu.m");
        assert_eq!(json["kind"], "gpu");
        assert_eq!(json["requirements"]["min_vcpu"], 8);
    }

    #[test]
    fn usage_event_round_trips() {
        let event = UsageEvent {
            id: "uev_01".to_string(),
            resource_id: "res_01".to_string(),
            placement_id: Some("plc_01".to_string()),
            node_id: None,
            event_type: "compute.seconds".to_string(),
            quantity: 60.0,
            unit: "seconds".to_string(),
            measured_at: Utc::now(),
            processed_at: None,
            cost_event_id: None,
            metadata: serde_json::Map::new(),
            created_at: None,
            labels: HashMap::new(),
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: UsageEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, event.id);
    }
}
