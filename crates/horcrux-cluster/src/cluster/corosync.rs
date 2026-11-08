//! Corosync integration for cluster management
//! Corosync provides the cluster communication layer and quorum
use super::node::Node;
use horcrux_common::Result;
use tokio::process::Command;
use tracing::{error, info};

/// Corosync manager
pub struct CorosyncManager {
    /// Cluster name + voting members written to corosync.conf by this
    /// manager. `None` until `init_cluster` ran on this process.
    members: tokio::sync::Mutex<Option<(String, Vec<(Node, u32)>)>>,
}

const DEFAULT_CONF: &str = "/etc/corosync/corosync.conf";

fn conf_path() -> String {
    std::env::var("HORCROUX_COROSYNC_CONF").unwrap_or_else(|_| DEFAULT_CONF.to_string())
}

impl Default for CorosyncManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CorosyncManager {
    pub fn new() -> Self {
        Self {
            members: tokio::sync::Mutex::new(None),
        }
    }

    /// Regenerate corosync.conf from the current member list (quorum block
    /// from `quorum::plan`: an even voter count gets `auto_tie_breaker`),
    /// write it to `path`, return the plan warnings.
    async fn write_members(
        path: &str,
        cluster_name: &str,
        nodes: &[(Node, u32)],
    ) -> Result<Vec<String>> {
        let (cfg, plan) = generate_multi_node_config(cluster_name, nodes);
        tokio::fs::write(path, cfg).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to write Corosync config: {}", e))
        })?;
        Ok(plan.warnings)
    }

    /// Ask a running corosync to re-read its config (best effort).
    async fn reload() {
        match Command::new("corosync-cfgtool").arg("-R").output().await {
            Ok(o) if o.status.success() => {}
            Ok(o) => tracing::warn!(
                "corosync-cfgtool -R failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ),
            Err(e) => tracing::warn!("corosync-cfgtool not runnable: {}", e),
        }
    }

    /// Initialize a new cluster with Corosync
    pub async fn init_cluster(&self, cluster_name: &str, node: &Node) -> Result<()> {
        info!("Initializing Corosync cluster: {}", cluster_name);

        // Generate Corosync configuration
        let config = self.generate_config(cluster_name, node);
        *self.members.lock().await = Some((cluster_name.to_string(), vec![(node.clone(), 1)]));

        // Write config to /etc/corosync/corosync.conf
        tokio::fs::write(conf_path(), config).await.map_err(|e| {
            horcrux_common::Error::System(format!("Failed to write Corosync config: {}", e))
        })?;

        // Start Corosync service
        let output = Command::new("systemctl")
            .arg("start")
            .arg("corosync")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to start Corosync: {}", e))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!("Failed to start Corosync: {}", stderr);
            return Err(horcrux_common::Error::System(format!(
                "Failed to start Corosync: {}",
                stderr
            )));
        }

        // Start Pacemaker (for HA)
        Command::new("systemctl")
            .arg("start")
            .arg("pacemaker")
            .output()
            .await
            .ok();

        info!("Corosync cluster initialized successfully");
        Ok(())
    }

    /// Join an existing cluster
    pub async fn join_cluster(
        &self,
        cluster_name: &str,
        _node_name: &str,
        master_ip: &str,
    ) -> Result<()> {
        info!("Joining cluster {} via {}", cluster_name, master_ip);

        // In a real implementation:
        // 1. Fetch cluster config from master node
        // 2. Add this node to the config
        // 3. Start Corosync with the updated config
        // 4. Sync with cluster state

        Ok(())
    }

    /// Add a node to the cluster
    pub async fn add_node(&self, node: &Node) -> Result<()> {
        info!("Adding node {} to Corosync cluster", node.name);
        let mut guard = self.members.lock().await;
        let Some((cluster, nodes)) = guard.as_mut() else {
            return Ok(());
        };
        if nodes.iter().any(|(n, _)| n.name == node.name) {
            return Ok(());
        }
        nodes.push((node.clone(), 1));
        for w in Self::write_members(&conf_path(), cluster, nodes).await? {
            tracing::warn!("quorum: {}", w);
        }
        drop(guard);
        Self::reload().await;
        Ok(())
    }

    /// Remove a node from the cluster
    pub async fn remove_node(&self, node: &Node) -> Result<()> {
        info!("Removing node {} from Corosync cluster", node.name);
        let mut guard = self.members.lock().await;
        let Some((cluster, nodes)) = guard.as_mut() else {
            return Ok(());
        };
        let before = nodes.len();
        nodes.retain(|(n, _)| n.name != node.name);
        if nodes.len() == before {
            return Ok(());
        }
        for w in Self::write_members(&conf_path(), cluster, nodes).await? {
            tracing::warn!("quorum: {}", w);
        }
        drop(guard);
        Self::reload().await;
        Ok(())
    }

    /// Check if cluster has quorum
    pub async fn check_quorum(&self) -> Result<bool> {
        let output = Command::new("corosync-quorumtool")
            .arg("-s")
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to check quorum: {}", e)))?;

        if !output.status.success() {
            return Ok(false);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse output to check if we have quorum
        // Quorum status line looks like: "Quorate:          Yes"
        Ok(stdout.contains("Quorate") && stdout.contains("Yes"))
    }

    /// Generate Corosync configuration file
    fn generate_config(&self, cluster_name: &str, node: &Node) -> String {
        format!(
            r#"totem {{
    version: 2
    cluster_name: {cluster_name}
    transport: knet
    crypto_cipher: aes256
    crypto_hash: sha256
}}

nodelist {{
    node {{
        ring0_addr: {node_ip}
        name: {node_name}
        nodeid: {node_id}
    }}
}}

quorum {{
    provider: corosync_votequorum
    expected_votes: 1
}}

logging {{
    to_logfile: yes
    logfile: /var/log/corosync/corosync.log
    to_syslog: yes
    timestamp: on
}}
"#,
            cluster_name = cluster_name,
            node_ip = node.ip,
            node_name = node.name,
            node_id = node.id,
        )
    }

    /// Check if Corosync is available
    pub fn check_corosync_available() -> bool {
        std::process::Command::new("corosync-quorumtool")
            .arg("-V")
            .output()
            .is_ok()
    }

    /// Get Corosync version
    pub async fn get_corosync_version() -> Result<String> {
        let output = Command::new("corosync-quorumtool")
            .arg("-V")
            .output()
            .await
            .map_err(|e| {
                horcrux_common::Error::System(format!("Failed to run corosync-quorumtool: {}", e))
            })?;

        if !output.status.success() {
            return Err(horcrux_common::Error::System(
                "Corosync not found or not working".to_string(),
            ));
        }

        let version = String::from_utf8_lossy(&output.stdout);
        Ok(version.lines().next().unwrap_or("Unknown").to_string())
    }
}

/// Render a full multi-node corosync.conf. `nodes` pairs each node with its
/// `quorum_votes` (0 = non-voting). The `quorum {}` block comes from
/// `quorum::plan`, so an even voter count (e.g. 10) gets `auto_tie_breaker`.
/// Pure: writes nothing. Returns the config text and the plan (for warnings).
pub fn generate_multi_node_config(
    cluster_name: &str,
    nodes: &[(Node, u32)],
) -> (String, super::quorum::QuorumPlan) {
    use super::quorum::{corosync_quorum_block, plan, Voter};
    let voters: Vec<Voter> = nodes
        .iter()
        .map(|(n, v)| Voter {
            name: n.name.clone(),
            node_id: n.id,
            votes: *v,
        })
        .collect();
    let qp = plan(&voters);
    let mut s = format!(
        "totem {{\n    version: 2\n    cluster_name: {cluster_name}\n    transport: knet\n    crypto_cipher: aes256\n    crypto_hash: sha256\n}}\n\nnodelist {{\n"
    );
    for (n, v) in nodes {
        s.push_str(&format!(
            "    node {{\n        ring0_addr: {}\n        name: {}\n        nodeid: {}\n        quorum_votes: {}\n    }}\n",
            n.ip, n.name, n.id, v
        ));
    }
    s.push_str("}\n\n");
    s.push_str(&corosync_quorum_block(&qp));
    s.push_str("\nlogging {\n    to_logfile: yes\n    logfile: /var/log/corosync/corosync.log\n    to_syslog: yes\n    timestamp: on\n}\n");
    (s, qp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::node::Node;

    fn node(id: u32) -> Node {
        Node::new(id, format!("n{id}"), format!("10.0.0.{id}"))
    }

    #[test]
    fn ten_node_config_enables_tie_breaker_and_lists_all_nodes() {
        let nodes: Vec<(Node, u32)> = (1..=10).map(|i| (node(i), 1)).collect();
        let (cfg, plan) = generate_multi_node_config("lab", &nodes);
        assert_eq!(plan.total_votes, 10);
        assert!(plan.auto_tie_breaker);
        assert_eq!(cfg.matches("ring0_addr:").count(), 10);
        assert!(cfg.contains("expected_votes: 10"));
        assert!(cfg.contains("auto_tie_breaker: 1"));
        assert!(cfg.contains("quorum_votes: 1"));
    }

    #[test]
    fn odd_node_config_has_no_tie_breaker() {
        let nodes: Vec<(Node, u32)> = (1..=3).map(|i| (node(i), 1)).collect();
        let (cfg, plan) = generate_multi_node_config("lab", &nodes);
        assert!(!plan.auto_tie_breaker);
        assert!(!cfg.contains("auto_tie_breaker"));
    }

    #[tokio::test]
    async fn write_members_renders_plan_to_file() {
        let path = std::env::temp_dir().join(format!("hx-corosync-{}.conf", std::process::id()));
        let path = path.to_str().unwrap().to_string();
        let mut nodes: Vec<(Node, u32)> = (1..=4).map(|i| (node(i), 1)).collect();
        let warns = CorosyncManager::write_members(&path, "lab", &nodes)
            .await
            .unwrap();
        let cfg = std::fs::read_to_string(&path).unwrap();
        assert!(cfg.contains("auto_tie_breaker: 1"), "4 voters is even");
        assert!(warns.iter().any(|w| w.contains("even")));
        nodes.pop();
        CorosyncManager::write_members(&path, "lab", &nodes)
            .await
            .unwrap();
        let cfg = std::fs::read_to_string(&path).unwrap();
        assert!(!cfg.contains("auto_tie_breaker"), "3 voters is odd");
        assert_eq!(cfg.matches("ring0_addr:").count(), 3);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn add_remove_node_before_init_is_a_noop() {
        let m = CorosyncManager::new();
        m.add_node(&node(2)).await.unwrap();
        m.remove_node(&node(2)).await.unwrap();
    }
}
