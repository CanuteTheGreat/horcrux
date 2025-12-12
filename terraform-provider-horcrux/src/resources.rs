//! Terraform Resources for Horcrux
//!
//! Defines the resources that can be managed via Terraform.

use crate::client::HorcruxClient;
use crate::schema::{
    AttributeType, Diagnostic, NestedBlock, NestingMode, ResourceSchema, SchemaAttribute,
    SchemaBlock,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Result type for resource operations
pub type ResourceResult<T> = Result<T, Vec<Diagnostic>>;

/// Resource state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceState {
    pub values: HashMap<String, Value>,
}

impl ResourceState {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key)
    }

    pub fn get_string(&self, key: &str) -> Option<String> {
        self.values.get(key).and_then(|v| v.as_str()).map(String::from)
    }

    pub fn get_i64(&self, key: &str) -> Option<i64> {
        self.values.get(key).and_then(|v| v.as_i64())
    }

    pub fn get_u64(&self, key: &str) -> Option<u64> {
        self.values.get(key).and_then(|v| v.as_u64())
    }

    pub fn get_bool(&self, key: &str) -> Option<bool> {
        self.values.get(key).and_then(|v| v.as_bool())
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.values.insert(key.to_string(), value);
    }
}

impl Default for ResourceState {
    fn default() -> Self {
        Self::new()
    }
}

/// Resource trait
#[async_trait]
pub trait Resource: Send + Sync {
    /// Resource type name
    fn type_name(&self) -> &str;

    /// Get the schema for this resource
    fn schema(&self) -> ResourceSchema;

    /// Create a new resource
    async fn create(
        &self,
        client: &HorcruxClient,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState>;

    /// Read an existing resource
    async fn read(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
    ) -> ResourceResult<ResourceState>;

    /// Update an existing resource
    async fn update(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState>;

    /// Delete a resource
    async fn delete(&self, client: &HorcruxClient, current: &ResourceState) -> ResourceResult<()>;

    /// Plan changes
    fn plan_change(
        &self,
        current: Option<&ResourceState>,
        proposed: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        // Default implementation: return proposed state
        let _ = current;
        Ok(proposed.clone())
    }
}

// ============================================================================
// VM Resource
// ============================================================================

pub struct VmResource;

impl VmResource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for VmResource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Resource for VmResource {
    fn type_name(&self) -> &str {
        "horcrux_vm"
    }

    fn schema(&self) -> ResourceSchema {
        let block = SchemaBlock::new()
            .with_attribute(
                "id",
                SchemaAttribute::string()
                    .with_description("VM ID (e.g., vm-100)")
                    .required(),
            )
            .with_attribute(
                "name",
                SchemaAttribute::string()
                    .with_description("Display name of the VM")
                    .required(),
            )
            .with_attribute(
                "hypervisor",
                SchemaAttribute::string()
                    .with_description("Hypervisor type (Qemu, Lxd, Incus)")
                    .optional()
                    .with_default(serde_json::json!("Qemu")),
            )
            .with_attribute(
                "architecture",
                SchemaAttribute::string()
                    .with_description("CPU architecture (X86_64, Aarch64, Riscv64)")
                    .optional()
                    .with_default(serde_json::json!("X86_64")),
            )
            .with_attribute(
                "cpus",
                SchemaAttribute::number()
                    .with_description("Number of CPU cores")
                    .required(),
            )
            .with_attribute(
                "memory",
                SchemaAttribute::number()
                    .with_description("Memory in MB")
                    .required(),
            )
            .with_attribute(
                "disk_size",
                SchemaAttribute::number()
                    .with_description("Disk size in GB")
                    .required(),
            )
            .with_attribute(
                "description",
                SchemaAttribute::string()
                    .with_description("VM description")
                    .optional(),
            )
            .with_attribute(
                "tags",
                SchemaAttribute::list(AttributeType::String)
                    .with_description("Tags for the VM")
                    .optional(),
            )
            .with_attribute(
                "status",
                SchemaAttribute::string()
                    .with_description("Current VM status")
                    .computed(),
            )
            .with_attribute(
                "node",
                SchemaAttribute::string()
                    .with_description("Node where VM is running")
                    .computed(),
            )
            .with_description("Manages a Horcrux virtual machine");

        ResourceSchema::new(1, block)
    }

    async fn create(
        &self,
        client: &HorcruxClient,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let request = crate::client::CreateVmRequest {
            id: planned.get_string("id").unwrap_or_default(),
            name: planned.get_string("name").unwrap_or_default(),
            hypervisor: planned.get_string("hypervisor").unwrap_or_else(|| "Qemu".to_string()),
            architecture: planned.get_string("architecture").unwrap_or_else(|| "X86_64".to_string()),
            cpus: planned.get_u64("cpus").unwrap_or(1) as u32,
            memory: planned.get_u64("memory").unwrap_or(1024),
            disk_size: planned.get_u64("disk_size").unwrap_or(20),
            description: planned.get_string("description"),
            tags: planned
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        };

        match client.create_vm(&request).await {
            Ok(vm) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(vm.id));
                state.set("name", serde_json::json!(vm.name));
                state.set("hypervisor", serde_json::json!(vm.hypervisor));
                state.set("architecture", serde_json::json!(vm.architecture));
                state.set("cpus", serde_json::json!(vm.cpus));
                state.set("memory", serde_json::json!(vm.memory));
                state.set("disk_size", serde_json::json!(vm.disk_size));
                state.set("status", serde_json::json!(vm.status));
                if let Some(node) = vm.node {
                    state.set("node", serde_json::json!(node));
                }
                if let Some(desc) = vm.description {
                    state.set("description", serde_json::json!(desc));
                }
                if !vm.tags.is_empty() {
                    state.set("tags", serde_json::json!(vm.tags));
                }
                Ok(state)
            }
            Err(e) => Err(vec![Diagnostic::error(&format!("Failed to create VM: {}", e))]),
        }
    }

    async fn read(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("VM ID is required")]
        })?;

        match client.get_vm(&id).await {
            Ok(vm) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(vm.id));
                state.set("name", serde_json::json!(vm.name));
                state.set("hypervisor", serde_json::json!(vm.hypervisor));
                state.set("architecture", serde_json::json!(vm.architecture));
                state.set("cpus", serde_json::json!(vm.cpus));
                state.set("memory", serde_json::json!(vm.memory));
                state.set("disk_size", serde_json::json!(vm.disk_size));
                state.set("status", serde_json::json!(vm.status));
                if let Some(node) = vm.node {
                    state.set("node", serde_json::json!(node));
                }
                if let Some(desc) = vm.description {
                    state.set("description", serde_json::json!(desc));
                }
                if !vm.tags.is_empty() {
                    state.set("tags", serde_json::json!(vm.tags));
                }
                Ok(state)
            }
            Err(crate::client::ClientError::NotFound(_)) => {
                // Resource no longer exists
                Ok(ResourceState::new())
            }
            Err(e) => Err(vec![Diagnostic::error(&format!("Failed to read VM: {}", e))]),
        }
    }

    async fn update(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("VM ID is required")]
        })?;

        let request = crate::client::UpdateVmRequest {
            name: planned.get_string("name"),
            cpus: planned.get_u64("cpus").map(|v| v as u32),
            memory: planned.get_u64("memory"),
            description: planned.get_string("description"),
            tags: planned
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                }),
        };

        match client.update_vm(&id, &request).await {
            Ok(vm) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(vm.id));
                state.set("name", serde_json::json!(vm.name));
                state.set("hypervisor", serde_json::json!(vm.hypervisor));
                state.set("architecture", serde_json::json!(vm.architecture));
                state.set("cpus", serde_json::json!(vm.cpus));
                state.set("memory", serde_json::json!(vm.memory));
                state.set("disk_size", serde_json::json!(vm.disk_size));
                state.set("status", serde_json::json!(vm.status));
                if let Some(node) = vm.node {
                    state.set("node", serde_json::json!(node));
                }
                if let Some(desc) = vm.description {
                    state.set("description", serde_json::json!(desc));
                }
                if !vm.tags.is_empty() {
                    state.set("tags", serde_json::json!(vm.tags));
                }
                Ok(state)
            }
            Err(e) => Err(vec![Diagnostic::error(&format!("Failed to update VM: {}", e))]),
        }
    }

    async fn delete(&self, client: &HorcruxClient, current: &ResourceState) -> ResourceResult<()> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("VM ID is required")]
        })?;

        match client.delete_vm(&id).await {
            Ok(()) => Ok(()),
            Err(crate::client::ClientError::NotFound(_)) => Ok(()), // Already deleted
            Err(e) => Err(vec![Diagnostic::error(&format!("Failed to delete VM: {}", e))]),
        }
    }
}

// ============================================================================
// Container Resource
// ============================================================================

pub struct ContainerResource;

impl ContainerResource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ContainerResource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Resource for ContainerResource {
    fn type_name(&self) -> &str {
        "horcrux_container"
    }

    fn schema(&self) -> ResourceSchema {
        let port_block = SchemaBlock::new()
            .with_attribute(
                "host_port",
                SchemaAttribute::number()
                    .with_description("Port on the host")
                    .required(),
            )
            .with_attribute(
                "container_port",
                SchemaAttribute::number()
                    .with_description("Port in the container")
                    .required(),
            )
            .with_attribute(
                "protocol",
                SchemaAttribute::string()
                    .with_description("Protocol (tcp/udp)")
                    .optional()
                    .with_default(serde_json::json!("tcp")),
            );

        let block = SchemaBlock::new()
            .with_attribute(
                "id",
                SchemaAttribute::string()
                    .with_description("Container ID")
                    .required(),
            )
            .with_attribute(
                "name",
                SchemaAttribute::string()
                    .with_description("Container name")
                    .required(),
            )
            .with_attribute(
                "runtime",
                SchemaAttribute::string()
                    .with_description("Container runtime (Docker, Podman, Lxc)")
                    .required(),
            )
            .with_attribute(
                "image",
                SchemaAttribute::string()
                    .with_description("Container image")
                    .required(),
            )
            .with_attribute(
                "cpus",
                SchemaAttribute::number()
                    .with_description("CPU limit (e.g., 0.5 for half a core)")
                    .optional(),
            )
            .with_attribute(
                "memory",
                SchemaAttribute::number()
                    .with_description("Memory limit in MB")
                    .optional(),
            )
            .with_attribute(
                "environment",
                SchemaAttribute::map(AttributeType::String)
                    .with_description("Environment variables")
                    .optional(),
            )
            .with_attribute(
                "command",
                SchemaAttribute::list(AttributeType::String)
                    .with_description("Command to run")
                    .optional(),
            )
            .with_block(
                "port",
                NestedBlock {
                    nesting_mode: NestingMode::List,
                    block: port_block,
                    min_items: None,
                    max_items: None,
                },
            )
            .with_attribute(
                "status",
                SchemaAttribute::string()
                    .with_description("Container status")
                    .computed(),
            )
            .with_description("Manages a Horcrux container");

        ResourceSchema::new(1, block)
    }

    async fn create(
        &self,
        client: &HorcruxClient,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let request = crate::client::CreateContainerRequest {
            id: planned.get_string("id").unwrap_or_default(),
            name: planned.get_string("name").unwrap_or_default(),
            runtime: planned.get_string("runtime").unwrap_or_else(|| "Docker".to_string()),
            image: planned.get_string("image").unwrap_or_default(),
            cpus: planned.get("cpus").and_then(|v| v.as_f64()),
            memory: planned.get_u64("memory"),
            ports: Vec::new(), // Would need to parse from nested blocks
            environment: planned
                .get("environment")
                .and_then(|v| v.as_object())
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            command: planned
                .get("command")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect()),
        };

        match client.create_container(&request).await {
            Ok(container) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(container.id));
                state.set("name", serde_json::json!(container.name));
                state.set("runtime", serde_json::json!(container.runtime));
                state.set("image", serde_json::json!(container.image));
                state.set("status", serde_json::json!(container.status));
                if let Some(cpus) = container.cpus {
                    state.set("cpus", serde_json::json!(cpus));
                }
                if let Some(memory) = container.memory {
                    state.set("memory", serde_json::json!(memory));
                }
                Ok(state)
            }
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to create container: {}",
                e
            ))]),
        }
    }

    async fn read(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("Container ID is required")]
        })?;

        match client.get_container(&id).await {
            Ok(container) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(container.id));
                state.set("name", serde_json::json!(container.name));
                state.set("runtime", serde_json::json!(container.runtime));
                state.set("image", serde_json::json!(container.image));
                state.set("status", serde_json::json!(container.status));
                if let Some(cpus) = container.cpus {
                    state.set("cpus", serde_json::json!(cpus));
                }
                if let Some(memory) = container.memory {
                    state.set("memory", serde_json::json!(memory));
                }
                Ok(state)
            }
            Err(crate::client::ClientError::NotFound(_)) => Ok(ResourceState::new()),
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to read container: {}",
                e
            ))]),
        }
    }

    async fn update(
        &self,
        _client: &HorcruxClient,
        _current: &ResourceState,
        _planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        // Containers typically need to be recreated for updates
        Err(vec![Diagnostic::error(
            "Container updates require recreation. Use lifecycle { create_before_destroy = true }",
        )])
    }

    async fn delete(&self, client: &HorcruxClient, current: &ResourceState) -> ResourceResult<()> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("Container ID is required")]
        })?;

        match client.delete_container(&id).await {
            Ok(()) => Ok(()),
            Err(crate::client::ClientError::NotFound(_)) => Ok(()),
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to delete container: {}",
                e
            ))]),
        }
    }
}

// ============================================================================
// Storage Pool Resource
// ============================================================================

pub struct StoragePoolResource;

impl StoragePoolResource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for StoragePoolResource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Resource for StoragePoolResource {
    fn type_name(&self) -> &str {
        "horcrux_storage_pool"
    }

    fn schema(&self) -> ResourceSchema {
        let block = SchemaBlock::new()
            .with_attribute(
                "id",
                SchemaAttribute::string()
                    .with_description("Storage pool ID")
                    .required(),
            )
            .with_attribute(
                "name",
                SchemaAttribute::string()
                    .with_description("Storage pool name")
                    .required(),
            )
            .with_attribute(
                "pool_type",
                SchemaAttribute::string()
                    .with_description("Pool type (Directory, Zfs, Lvm, Ceph, Nfs)")
                    .required(),
            )
            .with_attribute(
                "path",
                SchemaAttribute::string()
                    .with_description("Path for directory-based storage")
                    .optional(),
            )
            .with_attribute(
                "total_bytes",
                SchemaAttribute::number()
                    .with_description("Total capacity in bytes")
                    .computed(),
            )
            .with_attribute(
                "used_bytes",
                SchemaAttribute::number()
                    .with_description("Used space in bytes")
                    .computed(),
            )
            .with_attribute(
                "available_bytes",
                SchemaAttribute::number()
                    .with_description("Available space in bytes")
                    .computed(),
            )
            .with_description("Manages a Horcrux storage pool");

        ResourceSchema::new(1, block)
    }

    async fn create(
        &self,
        client: &HorcruxClient,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let request = crate::client::CreateStoragePoolRequest {
            id: planned.get_string("id").unwrap_or_default(),
            name: planned.get_string("name").unwrap_or_default(),
            pool_type: planned.get_string("pool_type").unwrap_or_else(|| "Directory".to_string()),
            path: planned.get_string("path"),
            ceph_config: None,
            nfs_config: None,
        };

        match client.create_storage_pool(&request).await {
            Ok(pool) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(pool.id));
                state.set("name", serde_json::json!(pool.name));
                state.set("pool_type", serde_json::json!(pool.pool_type));
                if let Some(path) = pool.path {
                    state.set("path", serde_json::json!(path));
                }
                state.set("total_bytes", serde_json::json!(pool.total_bytes));
                state.set("used_bytes", serde_json::json!(pool.used_bytes));
                state.set("available_bytes", serde_json::json!(pool.available_bytes));
                Ok(state)
            }
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to create storage pool: {}",
                e
            ))]),
        }
    }

    async fn read(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("Storage pool ID is required")]
        })?;

        match client.get_storage_pool(&id).await {
            Ok(pool) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(pool.id));
                state.set("name", serde_json::json!(pool.name));
                state.set("pool_type", serde_json::json!(pool.pool_type));
                if let Some(path) = pool.path {
                    state.set("path", serde_json::json!(path));
                }
                state.set("total_bytes", serde_json::json!(pool.total_bytes));
                state.set("used_bytes", serde_json::json!(pool.used_bytes));
                state.set("available_bytes", serde_json::json!(pool.available_bytes));
                Ok(state)
            }
            Err(crate::client::ClientError::NotFound(_)) => Ok(ResourceState::new()),
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to read storage pool: {}",
                e
            ))]),
        }
    }

    async fn update(
        &self,
        _client: &HorcruxClient,
        _current: &ResourceState,
        _planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        Err(vec![Diagnostic::error(
            "Storage pool updates are not supported. Recreate the resource.",
        )])
    }

    async fn delete(&self, client: &HorcruxClient, current: &ResourceState) -> ResourceResult<()> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("Storage pool ID is required")]
        })?;

        match client.delete_storage_pool(&id).await {
            Ok(()) => Ok(()),
            Err(crate::client::ClientError::NotFound(_)) => Ok(()),
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to delete storage pool: {}",
                e
            ))]),
        }
    }
}

// ============================================================================
// Firewall Rule Resource
// ============================================================================

pub struct FirewallRuleResource;

impl FirewallRuleResource {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FirewallRuleResource {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Resource for FirewallRuleResource {
    fn type_name(&self) -> &str {
        "horcrux_firewall_rule"
    }

    fn schema(&self) -> ResourceSchema {
        let block = SchemaBlock::new()
            .with_attribute(
                "id",
                SchemaAttribute::string()
                    .with_description("Rule ID")
                    .computed(),
            )
            .with_attribute(
                "name",
                SchemaAttribute::string()
                    .with_description("Rule name")
                    .required(),
            )
            .with_attribute(
                "action",
                SchemaAttribute::string()
                    .with_description("Action (Accept, Drop, Reject)")
                    .required(),
            )
            .with_attribute(
                "direction",
                SchemaAttribute::string()
                    .with_description("Direction (in, out)")
                    .optional()
                    .with_default(serde_json::json!("in")),
            )
            .with_attribute(
                "protocol",
                SchemaAttribute::string()
                    .with_description("Protocol (Tcp, Udp, Icmp)")
                    .optional(),
            )
            .with_attribute(
                "port",
                SchemaAttribute::number()
                    .with_description("Port number")
                    .optional(),
            )
            .with_attribute(
                "source",
                SchemaAttribute::string()
                    .with_description("Source CIDR")
                    .optional(),
            )
            .with_attribute(
                "destination",
                SchemaAttribute::string()
                    .with_description("Destination CIDR")
                    .optional(),
            )
            .with_attribute(
                "enabled",
                SchemaAttribute::bool()
                    .with_description("Whether rule is enabled")
                    .optional()
                    .with_default(serde_json::json!(true)),
            )
            .with_attribute(
                "priority",
                SchemaAttribute::number()
                    .with_description("Rule priority (lower = higher priority)")
                    .optional()
                    .with_default(serde_json::json!(0)),
            )
            .with_description("Manages a Horcrux firewall rule");

        ResourceSchema::new(1, block)
    }

    async fn create(
        &self,
        client: &HorcruxClient,
        planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let request = crate::client::CreateFirewallRuleRequest {
            name: planned.get_string("name").unwrap_or_default(),
            action: planned.get_string("action").unwrap_or_else(|| "Drop".to_string()),
            direction: planned.get_string("direction").unwrap_or_else(|| "in".to_string()),
            protocol: planned.get_string("protocol"),
            port: planned.get_u64("port").map(|v| v as u16),
            source: planned.get_string("source"),
            destination: planned.get_string("destination"),
            enabled: planned.get_bool("enabled").unwrap_or(true),
            priority: planned.get_i64("priority").unwrap_or(0) as i32,
        };

        match client.create_firewall_rule(&request).await {
            Ok(rule) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(rule.id));
                state.set("name", serde_json::json!(rule.name));
                state.set("action", serde_json::json!(rule.action));
                state.set("direction", serde_json::json!(rule.direction));
                if let Some(protocol) = rule.protocol {
                    state.set("protocol", serde_json::json!(protocol));
                }
                if let Some(port) = rule.port {
                    state.set("port", serde_json::json!(port));
                }
                if let Some(source) = rule.source {
                    state.set("source", serde_json::json!(source));
                }
                if let Some(destination) = rule.destination {
                    state.set("destination", serde_json::json!(destination));
                }
                state.set("enabled", serde_json::json!(rule.enabled));
                state.set("priority", serde_json::json!(rule.priority));
                Ok(state)
            }
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to create firewall rule: {}",
                e
            ))]),
        }
    }

    async fn read(
        &self,
        client: &HorcruxClient,
        current: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("Firewall rule ID is required")]
        })?;

        match client.get_firewall_rule(&id).await {
            Ok(rule) => {
                let mut state = ResourceState::new();
                state.set("id", serde_json::json!(rule.id));
                state.set("name", serde_json::json!(rule.name));
                state.set("action", serde_json::json!(rule.action));
                state.set("direction", serde_json::json!(rule.direction));
                if let Some(protocol) = rule.protocol {
                    state.set("protocol", serde_json::json!(protocol));
                }
                if let Some(port) = rule.port {
                    state.set("port", serde_json::json!(port));
                }
                if let Some(source) = rule.source {
                    state.set("source", serde_json::json!(source));
                }
                if let Some(destination) = rule.destination {
                    state.set("destination", serde_json::json!(destination));
                }
                state.set("enabled", serde_json::json!(rule.enabled));
                state.set("priority", serde_json::json!(rule.priority));
                Ok(state)
            }
            Err(crate::client::ClientError::NotFound(_)) => Ok(ResourceState::new()),
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to read firewall rule: {}",
                e
            ))]),
        }
    }

    async fn update(
        &self,
        _client: &HorcruxClient,
        _current: &ResourceState,
        _planned: &ResourceState,
    ) -> ResourceResult<ResourceState> {
        Err(vec![Diagnostic::error(
            "Firewall rule updates require recreation",
        )])
    }

    async fn delete(&self, client: &HorcruxClient, current: &ResourceState) -> ResourceResult<()> {
        let id = current.get_string("id").ok_or_else(|| {
            vec![Diagnostic::error("Firewall rule ID is required")]
        })?;

        match client.delete_firewall_rule(&id).await {
            Ok(()) => {
                // Apply firewall after deletion
                let _ = client.apply_firewall().await;
                Ok(())
            }
            Err(crate::client::ClientError::NotFound(_)) => Ok(()),
            Err(e) => Err(vec![Diagnostic::error(&format!(
                "Failed to delete firewall rule: {}",
                e
            ))]),
        }
    }
}

/// Get all available resources
pub fn get_all_resources() -> Vec<Box<dyn Resource>> {
    vec![
        Box::new(VmResource::new()),
        Box::new(ContainerResource::new()),
        Box::new(StoragePoolResource::new()),
        Box::new(FirewallRuleResource::new()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_state() {
        let mut state = ResourceState::new();
        state.set("name", serde_json::json!("test-vm"));
        state.set("cpus", serde_json::json!(4));
        state.set("memory", serde_json::json!(8192));

        assert_eq!(state.get_string("name"), Some("test-vm".to_string()));
        assert_eq!(state.get_u64("cpus"), Some(4));
        assert_eq!(state.get_u64("memory"), Some(8192));
    }

    #[test]
    fn test_vm_resource_schema() {
        let resource = VmResource::new();
        let schema = resource.schema();

        assert!(schema.block.attributes.contains_key("id"));
        assert!(schema.block.attributes.contains_key("name"));
        assert!(schema.block.attributes.contains_key("cpus"));
        assert!(schema.block.attributes.contains_key("memory"));
    }

    #[test]
    fn test_container_resource_schema() {
        let resource = ContainerResource::new();
        let schema = resource.schema();

        assert!(schema.block.attributes.contains_key("id"));
        assert!(schema.block.attributes.contains_key("runtime"));
        assert!(schema.block.attributes.contains_key("image"));
    }
}
