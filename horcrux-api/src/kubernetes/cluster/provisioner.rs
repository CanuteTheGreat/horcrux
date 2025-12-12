//! Cluster provisioning
//!
//! Supports provisioning new Kubernetes clusters using k3s or kubeadm.

use crate::kubernetes::error::{K8sError, K8sResult};
use crate::kubernetes::types::{ClusterProvider, ClusterProvisionRequest, NodeRole, ProvisionNode};

/// Provision a new Kubernetes cluster
pub async fn provision_cluster(request: &ClusterProvisionRequest) -> K8sResult<String> {
    match request.provider {
        ClusterProvider::K3s => provision_k3s(request).await,
        ClusterProvider::Kubeadm => provision_kubeadm(request).await,
        _ => Err(K8sError::ProvisioningError(format!(
            "Unsupported provider: {:?}",
            request.provider
        ))),
    }
}

/// Provision a k3s cluster
async fn provision_k3s(request: &ClusterProvisionRequest) -> K8sResult<String> {
    let control_planes: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::ControlPlane)
        .collect();

    let workers: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::Worker)
        .collect();

    if control_planes.is_empty() {
        return Err(K8sError::ProvisioningError(
            "At least one control plane node is required".to_string(),
        ));
    }

    // Install k3s on the first control plane
    let first_cp = &control_planes[0];
    let k3s_token = uuid::Uuid::new_v4().to_string();

    let install_cmd = build_k3s_server_command(request, &k3s_token, true);
    run_ssh_command(first_cp, &install_cmd).await?;

    // Wait for first node to be ready
    tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

    // Get the kubeconfig from the first node
    let kubeconfig = get_remote_kubeconfig(first_cp).await?;

    // If HA mode, install k3s on additional control planes
    if request.config.ha && control_planes.len() > 1 {
        let server_url = format!("https://{}:6443", first_cp.address);

        for cp in control_planes.iter().skip(1) {
            let join_cmd = build_k3s_server_command(request, &k3s_token, false)
                + &format!(" --server {}", server_url);
            run_ssh_command(cp, &join_cmd).await?;
        }
    }

    // Install k3s agent on worker nodes
    let server_url = format!("https://{}:6443", first_cp.address);
    for worker in workers {
        let agent_cmd = build_k3s_agent_command(&server_url, &k3s_token);
        run_ssh_command(worker, &agent_cmd).await?;
    }

    // Return the kubeconfig (with external IP adjusted)
    let kubeconfig = adjust_kubeconfig_server(&kubeconfig, &first_cp.address)?;

    Ok(kubeconfig)
}

/// Provision a kubeadm cluster
async fn provision_kubeadm(request: &ClusterProvisionRequest) -> K8sResult<String> {
    let control_planes: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::ControlPlane)
        .collect();

    let workers: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::Worker)
        .collect();

    if control_planes.is_empty() {
        return Err(K8sError::ProvisioningError(
            "At least one control plane node is required".to_string(),
        ));
    }

    // Install prerequisites on all nodes
    for node in &request.nodes {
        install_kubeadm_prerequisites(node).await?;
    }

    // Initialize the first control plane
    let first_cp = &control_planes[0];
    let init_cmd = build_kubeadm_init_command(request);
    let output = run_ssh_command(first_cp, &init_cmd).await?;

    // Extract join command from output
    let join_command = extract_join_command(&output)?;

    // Get kubeconfig
    let kubeconfig = get_remote_kubeconfig_kubeadm(first_cp).await?;

    // Install CNI
    let cni = request.config.cni.as_deref().unwrap_or("calico");
    install_cni(first_cp, cni).await?;

    // Join additional control planes (if HA)
    if request.config.ha && control_planes.len() > 1 {
        let cp_join_cmd = format!("{} --control-plane", join_command);
        for cp in control_planes.iter().skip(1) {
            run_ssh_command(cp, &cp_join_cmd).await?;
        }
    }

    // Join worker nodes
    for worker in workers {
        run_ssh_command(worker, &join_command).await?;
    }

    // Adjust kubeconfig for external access
    let kubeconfig = adjust_kubeconfig_server(&kubeconfig, &first_cp.address)?;

    Ok(kubeconfig)
}

/// Build k3s server install command
fn build_k3s_server_command(
    request: &ClusterProvisionRequest,
    token: &str,
    is_first: bool,
) -> String {
    let mut cmd = format!(
        "curl -sfL https://get.k3s.io | K3S_TOKEN={} sh -s - server",
        token
    );

    if let Some(version) = &request.version {
        cmd.push_str(&format!(" INSTALL_K3S_VERSION={}", version));
    }

    if !is_first {
        // Joining an existing cluster
        cmd.push_str(" --cluster-init");
    }

    if let Some(pod_cidr) = &request.config.pod_cidr {
        cmd.push_str(&format!(" --cluster-cidr={}", pod_cidr));
    }

    if let Some(service_cidr) = &request.config.service_cidr {
        cmd.push_str(&format!(" --service-cidr={}", service_cidr));
    }

    // Disable traefik by default (users can install their own ingress)
    cmd.push_str(" --disable traefik");

    cmd
}

/// Build k3s agent install command
fn build_k3s_agent_command(server_url: &str, token: &str) -> String {
    format!(
        "curl -sfL https://get.k3s.io | K3S_URL={} K3S_TOKEN={} sh -",
        server_url, token
    )
}

/// Build kubeadm init command
fn build_kubeadm_init_command(request: &ClusterProvisionRequest) -> String {
    let mut cmd = String::from("sudo kubeadm init");

    if let Some(pod_cidr) = &request.config.pod_cidr {
        cmd.push_str(&format!(" --pod-network-cidr={}", pod_cidr));
    } else {
        // Default for Calico
        cmd.push_str(" --pod-network-cidr=192.168.0.0/16");
    }

    if let Some(service_cidr) = &request.config.service_cidr {
        cmd.push_str(&format!(" --service-cidr={}", service_cidr));
    }

    if let Some(version) = &request.version {
        cmd.push_str(&format!(" --kubernetes-version={}", version));
    }

    cmd
}

/// Install kubeadm prerequisites on a node
async fn install_kubeadm_prerequisites(node: &ProvisionNode) -> K8sResult<()> {
    let prereq_script = r#"
        # Disable swap
        sudo swapoff -a
        sudo sed -i '/swap/d' /etc/fstab

        # Load kernel modules
        cat <<EOF | sudo tee /etc/modules-load.d/k8s.conf
overlay
br_netfilter
EOF
        sudo modprobe overlay
        sudo modprobe br_netfilter

        # Sysctl settings
        cat <<EOF | sudo tee /etc/sysctl.d/k8s.conf
net.bridge.bridge-nf-call-iptables  = 1
net.bridge.bridge-nf-call-ip6tables = 1
net.ipv4.ip_forward                 = 1
EOF
        sudo sysctl --system

        # Install containerd
        sudo apt-get update
        sudo apt-get install -y containerd
        sudo mkdir -p /etc/containerd
        containerd config default | sudo tee /etc/containerd/config.toml
        sudo sed -i 's/SystemdCgroup = false/SystemdCgroup = true/' /etc/containerd/config.toml
        sudo systemctl restart containerd

        # Install kubeadm, kubelet, kubectl
        sudo apt-get install -y apt-transport-https ca-certificates curl gnupg
        curl -fsSL https://pkgs.k8s.io/core:/stable:/v1.30/deb/Release.key | sudo gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg
        echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v1.30/deb/ /' | sudo tee /etc/apt/sources.list.d/kubernetes.list
        sudo apt-get update
        sudo apt-get install -y kubelet kubeadm kubectl
        sudo apt-mark hold kubelet kubeadm kubectl
    "#;

    run_ssh_command(node, prereq_script).await?;
    Ok(())
}

/// Install CNI plugin
async fn install_cni(node: &ProvisionNode, cni: &str) -> K8sResult<()> {
    let install_cmd = match cni {
        "calico" => {
            "kubectl --kubeconfig=/etc/kubernetes/admin.conf apply -f https://raw.githubusercontent.com/projectcalico/calico/v3.26.1/manifests/calico.yaml"
        }
        "flannel" => {
            "kubectl --kubeconfig=/etc/kubernetes/admin.conf apply -f https://github.com/flannel-io/flannel/releases/latest/download/kube-flannel.yml"
        }
        "cilium" => {
            "curl -L --remote-name-all https://github.com/cilium/cilium-cli/releases/latest/download/cilium-linux-amd64.tar.gz && tar xzvfC cilium-linux-amd64.tar.gz /usr/local/bin && cilium install"
        }
        _ => return Err(K8sError::ProvisioningError(format!("Unknown CNI: {}", cni))),
    };

    run_ssh_command(node, install_cmd).await?;
    Ok(())
}

/// Run SSH command on a remote node
async fn run_ssh_command(node: &ProvisionNode, command: &str) -> K8sResult<String> {
    use tokio::process::Command;

    let mut ssh_cmd = Command::new("ssh");

    ssh_cmd
        .arg("-o")
        .arg("StrictHostKeyChecking=no")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-p")
        .arg(node.port.to_string());

    if let Some(key) = &node.ssh_key {
        // Write key to temp file
        let key_file = tempfile::NamedTempFile::new().map_err(|e| {
            K8sError::ProvisioningError(format!("Failed to create temp key file: {}", e))
        })?;

        tokio::fs::write(key_file.path(), key).await.map_err(|e| {
            K8sError::ProvisioningError(format!("Failed to write key file: {}", e))
        })?;

        ssh_cmd.arg("-i").arg(key_file.path());
    }

    ssh_cmd
        .arg(format!("{}@{}", node.user, node.address))
        .arg(command);

    let output = ssh_cmd.output().await.map_err(|e| {
        K8sError::ProvisioningError(format!("SSH command failed: {}", e))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(K8sError::ProvisioningError(format!(
            "SSH command failed: {}",
            stderr
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Get kubeconfig from k3s node
async fn get_remote_kubeconfig(node: &ProvisionNode) -> K8sResult<String> {
    run_ssh_command(node, "sudo cat /etc/rancher/k3s/k3s.yaml").await
}

/// Get kubeconfig from kubeadm node
async fn get_remote_kubeconfig_kubeadm(node: &ProvisionNode) -> K8sResult<String> {
    run_ssh_command(node, "sudo cat /etc/kubernetes/admin.conf").await
}

/// Extract join command from kubeadm init output
fn extract_join_command(output: &str) -> K8sResult<String> {
    // Look for the join command in the output
    let lines: Vec<&str> = output.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.contains("kubeadm join") {
            // Join command might span multiple lines
            let mut join_cmd = line.trim().to_string();
            if i + 1 < lines.len() && lines[i + 1].trim().starts_with("--") {
                join_cmd.push(' ');
                join_cmd.push_str(lines[i + 1].trim());
            }
            return Ok(join_cmd);
        }
    }

    Err(K8sError::ProvisioningError(
        "Could not find join command in kubeadm output".to_string(),
    ))
}

/// Adjust kubeconfig to use external IP
fn adjust_kubeconfig_server(kubeconfig: &str, external_ip: &str) -> K8sResult<String> {
    // Replace 127.0.0.1 or localhost with external IP
    let adjusted = kubeconfig
        .replace("https://127.0.0.1:6443", &format!("https://{}:6443", external_ip))
        .replace("https://localhost:6443", &format!("https://{}:6443", external_ip))
        .replace("server: https://127.0.0.1", &format!("server: https://{}", external_ip))
        .replace("server: https://localhost", &format!("server: https://{}", external_ip));

    Ok(adjusted)
}

/// Destroy a provisioned cluster
pub async fn destroy_cluster(
    nodes: &[ProvisionNode],
    provider: ClusterProvider,
) -> K8sResult<()> {
    match provider {
        ClusterProvider::K3s => destroy_k3s(nodes).await,
        ClusterProvider::Kubeadm => destroy_kubeadm(nodes).await,
        _ => Err(K8sError::ProvisioningError(
            "Can only destroy self-provisioned clusters".to_string(),
        )),
    }
}

/// Uninstall k3s from all nodes
async fn destroy_k3s(nodes: &[ProvisionNode]) -> K8sResult<()> {
    for node in nodes {
        let uninstall_cmd = match node.role {
            NodeRole::ControlPlane => "/usr/local/bin/k3s-uninstall.sh",
            NodeRole::Worker => "/usr/local/bin/k3s-agent-uninstall.sh",
        };

        if let Err(e) = run_ssh_command(node, uninstall_cmd).await {
            tracing::warn!("Failed to uninstall k3s from {}: {}", node.address, e);
        }
    }

    Ok(())
}

/// Reset kubeadm on all nodes
async fn destroy_kubeadm(nodes: &[ProvisionNode]) -> K8sResult<()> {
    for node in nodes {
        let reset_cmd = "sudo kubeadm reset -f && sudo rm -rf /etc/cni/net.d /var/lib/cni /var/lib/kubelet /etc/kubernetes";

        if let Err(e) = run_ssh_command(node, reset_cmd).await {
            tracing::warn!("Failed to reset kubeadm on {}: {}", node.address, e);
        }
    }

    Ok(())
}

// ============================================================================
// Cluster Upgrade Operations
// ============================================================================

/// Upgrade request for a cluster
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClusterUpgradeRequest {
    /// Target Kubernetes version
    pub target_version: String,
    /// Nodes to upgrade (upgrade all if empty)
    pub nodes: Vec<ProvisionNode>,
    /// Cluster provider
    pub provider: ClusterProvider,
    /// Skip drain before upgrade (not recommended)
    #[serde(default)]
    pub skip_drain: bool,
}

/// Upgrade status for tracking progress
#[derive(Debug, Clone, serde::Serialize)]
pub struct UpgradeStatus {
    /// Overall upgrade status
    pub status: UpgradePhase,
    /// Current node being upgraded
    pub current_node: Option<String>,
    /// Nodes completed
    pub completed_nodes: Vec<String>,
    /// Nodes remaining
    pub remaining_nodes: Vec<String>,
    /// Error message if any
    pub error: Option<String>,
}

/// Upgrade phase
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradePhase {
    Pending,
    Draining,
    Upgrading,
    Uncordoning,
    Completed,
    Failed,
}

/// Upgrade a Kubernetes cluster to a new version
pub async fn upgrade_cluster(request: &ClusterUpgradeRequest) -> K8sResult<UpgradeStatus> {
    match request.provider {
        ClusterProvider::K3s => upgrade_k3s(request).await,
        ClusterProvider::Kubeadm => upgrade_kubeadm(request).await,
        _ => Err(K8sError::ProvisioningError(
            "Can only upgrade self-provisioned clusters".to_string(),
        )),
    }
}

/// Upgrade a k3s cluster
async fn upgrade_k3s(request: &ClusterUpgradeRequest) -> K8sResult<UpgradeStatus> {
    let control_planes: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::ControlPlane)
        .collect();

    let workers: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::Worker)
        .collect();

    let mut completed_nodes = Vec::new();

    // Upgrade control plane nodes first (one at a time for HA)
    for cp in &control_planes {
        tracing::info!("Upgrading k3s server on {}", cp.address);

        // K3s upgrade is straightforward - reinstall with new version
        let upgrade_cmd = format!(
            "curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION={} sh -s - server --cluster-init",
            request.target_version
        );

        run_ssh_command(cp, &upgrade_cmd).await?;

        // Wait for node to be ready
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

        completed_nodes.push(cp.address.clone());
    }

    // Upgrade worker nodes
    for worker in &workers {
        tracing::info!("Upgrading k3s agent on {}", worker.address);

        let upgrade_cmd = format!(
            "curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION={} sh -",
            request.target_version
        );

        run_ssh_command(worker, &upgrade_cmd).await?;

        // Wait for agent to restart
        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

        completed_nodes.push(worker.address.clone());
    }

    Ok(UpgradeStatus {
        status: UpgradePhase::Completed,
        current_node: None,
        completed_nodes,
        remaining_nodes: Vec::new(),
        error: None,
    })
}

/// Upgrade a kubeadm cluster
async fn upgrade_kubeadm(request: &ClusterUpgradeRequest) -> K8sResult<UpgradeStatus> {
    let control_planes: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::ControlPlane)
        .collect();

    let workers: Vec<_> = request
        .nodes
        .iter()
        .filter(|n| n.role == NodeRole::Worker)
        .collect();

    let mut completed_nodes = Vec::new();

    // Extract minor version for apt package (e.g., "v1.30.1" -> "1.30")
    let version_parts: Vec<&str> = request
        .target_version
        .trim_start_matches('v')
        .split('.')
        .collect();

    if version_parts.len() < 2 {
        return Err(K8sError::ProvisioningError(
            "Invalid version format".to_string(),
        ));
    }

    let minor_version = format!("{}.{}", version_parts[0], version_parts[1]);
    let full_version = request.target_version.trim_start_matches('v');

    // Upgrade first control plane node
    if let Some(first_cp) = control_planes.first() {
        tracing::info!("Upgrading first control plane: {}", first_cp.address);

        // Update kubeadm
        let update_kubeadm = format!(
            r#"
            curl -fsSL https://pkgs.k8s.io/core:/stable:/v{}/deb/Release.key | sudo gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg --yes
            echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v{}/deb/ /' | sudo tee /etc/apt/sources.list.d/kubernetes.list
            sudo apt-get update
            sudo apt-get install -y kubeadm={}-*
            "#,
            minor_version, minor_version, full_version
        );
        run_ssh_command(first_cp, &update_kubeadm).await?;

        // Plan and apply upgrade
        let upgrade_cmd = format!(
            "sudo kubeadm upgrade apply v{} -y",
            full_version
        );
        run_ssh_command(first_cp, &upgrade_cmd).await?;

        // Upgrade kubelet and kubectl
        let upgrade_kubelet = format!(
            r#"
            sudo apt-get install -y kubelet={}-* kubectl={}-*
            sudo systemctl daemon-reload
            sudo systemctl restart kubelet
            "#,
            full_version, full_version
        );
        run_ssh_command(first_cp, &upgrade_kubelet).await?;

        completed_nodes.push(first_cp.address.clone());
    }

    // Upgrade additional control plane nodes
    for cp in control_planes.iter().skip(1) {
        tracing::info!("Upgrading additional control plane: {}", cp.address);

        // Drain node if not skipped
        if !request.skip_drain {
            drain_node_ssh(control_planes.first().unwrap(), &cp.address).await?;
        }

        // Update packages and upgrade node
        let upgrade_cmd = format!(
            r#"
            curl -fsSL https://pkgs.k8s.io/core:/stable:/v{}/deb/Release.key | sudo gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg --yes
            echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v{}/deb/ /' | sudo tee /etc/apt/sources.list.d/kubernetes.list
            sudo apt-get update
            sudo apt-get install -y kubeadm={}-*
            sudo kubeadm upgrade node
            sudo apt-get install -y kubelet={}-* kubectl={}-*
            sudo systemctl daemon-reload
            sudo systemctl restart kubelet
            "#,
            minor_version, minor_version, full_version, full_version, full_version
        );
        run_ssh_command(cp, &upgrade_cmd).await?;

        // Uncordon node
        if !request.skip_drain {
            uncordon_node_ssh(control_planes.first().unwrap(), &cp.address).await?;
        }

        completed_nodes.push(cp.address.clone());
    }

    // Upgrade worker nodes
    for worker in &workers {
        tracing::info!("Upgrading worker node: {}", worker.address);

        // Drain node
        if !request.skip_drain {
            drain_node_ssh(control_planes.first().unwrap(), &worker.address).await?;
        }

        // Update packages and upgrade node
        let upgrade_cmd = format!(
            r#"
            curl -fsSL https://pkgs.k8s.io/core:/stable:/v{}/deb/Release.key | sudo gpg --dearmor -o /etc/apt/keyrings/kubernetes-apt-keyring.gpg --yes
            echo 'deb [signed-by=/etc/apt/keyrings/kubernetes-apt-keyring.gpg] https://pkgs.k8s.io/core:/stable:/v{}/deb/ /' | sudo tee /etc/apt/sources.list.d/kubernetes.list
            sudo apt-get update
            sudo apt-get install -y kubeadm={}-*
            sudo kubeadm upgrade node
            sudo apt-get install -y kubelet={}-* kubectl={}-*
            sudo systemctl daemon-reload
            sudo systemctl restart kubelet
            "#,
            minor_version, minor_version, full_version, full_version, full_version
        );
        run_ssh_command(worker, &upgrade_cmd).await?;

        // Uncordon node
        if !request.skip_drain {
            uncordon_node_ssh(control_planes.first().unwrap(), &worker.address).await?;
        }

        completed_nodes.push(worker.address.clone());
    }

    Ok(UpgradeStatus {
        status: UpgradePhase::Completed,
        current_node: None,
        completed_nodes,
        remaining_nodes: Vec::new(),
        error: None,
    })
}

/// Drain a node via SSH to a control plane
async fn drain_node_ssh(cp_node: &ProvisionNode, node_name: &str) -> K8sResult<()> {
    let drain_cmd = format!(
        "kubectl --kubeconfig=/etc/kubernetes/admin.conf drain {} --ignore-daemonsets --delete-emptydir-data --force",
        node_name
    );
    run_ssh_command(cp_node, &drain_cmd).await?;
    Ok(())
}

/// Uncordon a node via SSH to a control plane
async fn uncordon_node_ssh(cp_node: &ProvisionNode, node_name: &str) -> K8sResult<()> {
    let uncordon_cmd = format!(
        "kubectl --kubeconfig=/etc/kubernetes/admin.conf uncordon {}",
        node_name
    );
    run_ssh_command(cp_node, &uncordon_cmd).await?;
    Ok(())
}

// ============================================================================
// Node Management Operations
// ============================================================================

/// Add a new node to an existing cluster
pub async fn add_node(
    node: &ProvisionNode,
    join_info: &NodeJoinInfo,
    provider: ClusterProvider,
) -> K8sResult<()> {
    match provider {
        ClusterProvider::K3s => add_k3s_node(node, join_info).await,
        ClusterProvider::Kubeadm => add_kubeadm_node(node, join_info).await,
        _ => Err(K8sError::ProvisioningError(
            "Can only add nodes to self-provisioned clusters".to_string(),
        )),
    }
}

/// Information needed to join a node to the cluster
#[derive(Debug, Clone, serde::Deserialize)]
pub struct NodeJoinInfo {
    /// Join token
    pub token: String,
    /// API server address
    pub api_server: String,
    /// CA certificate hash (for kubeadm)
    pub ca_cert_hash: Option<String>,
    /// For kubeadm: whether to join as control plane
    pub control_plane: bool,
    /// Certificate key for control plane join (kubeadm)
    pub certificate_key: Option<String>,
}

/// Add a k3s node
async fn add_k3s_node(node: &ProvisionNode, join_info: &NodeJoinInfo) -> K8sResult<()> {
    let install_cmd = match node.role {
        NodeRole::ControlPlane => {
            format!(
                "curl -sfL https://get.k3s.io | K3S_URL={} K3S_TOKEN={} sh -s - server",
                join_info.api_server, join_info.token
            )
        }
        NodeRole::Worker => {
            format!(
                "curl -sfL https://get.k3s.io | K3S_URL={} K3S_TOKEN={} sh -",
                join_info.api_server, join_info.token
            )
        }
    };

    run_ssh_command(node, &install_cmd).await?;
    Ok(())
}

/// Add a kubeadm node
async fn add_kubeadm_node(node: &ProvisionNode, join_info: &NodeJoinInfo) -> K8sResult<()> {
    // Install prerequisites first
    install_kubeadm_prerequisites(node).await?;

    // Build join command
    let mut join_cmd = format!(
        "sudo kubeadm join {} --token {} --discovery-token-ca-cert-hash {}",
        join_info.api_server,
        join_info.token,
        join_info.ca_cert_hash.as_deref().unwrap_or("sha256:placeholder")
    );

    if join_info.control_plane {
        join_cmd.push_str(" --control-plane");
        if let Some(cert_key) = &join_info.certificate_key {
            join_cmd.push_str(&format!(" --certificate-key {}", cert_key));
        }
    }

    run_ssh_command(node, &join_cmd).await?;
    Ok(())
}

/// Remove a node from the cluster
pub async fn remove_node(
    cp_node: &ProvisionNode,
    node_to_remove: &ProvisionNode,
    provider: ClusterProvider,
) -> K8sResult<()> {
    let node_name = &node_to_remove.address;

    // Drain the node first
    drain_node_ssh(cp_node, node_name).await?;

    // Delete the node from the cluster
    let delete_cmd = format!(
        "kubectl --kubeconfig=/etc/kubernetes/admin.conf delete node {}",
        node_name
    );
    run_ssh_command(cp_node, &delete_cmd).await?;

    // Uninstall kubernetes from the removed node
    match provider {
        ClusterProvider::K3s => {
            let uninstall_cmd = match node_to_remove.role {
                NodeRole::ControlPlane => "/usr/local/bin/k3s-uninstall.sh",
                NodeRole::Worker => "/usr/local/bin/k3s-agent-uninstall.sh",
            };
            let _ = run_ssh_command(node_to_remove, uninstall_cmd).await;
        }
        ClusterProvider::Kubeadm => {
            let reset_cmd = "sudo kubeadm reset -f && sudo rm -rf /etc/cni/net.d";
            let _ = run_ssh_command(node_to_remove, reset_cmd).await;
        }
        _ => {}
    }

    Ok(())
}

/// Generate a new join token (kubeadm)
pub async fn generate_join_token(cp_node: &ProvisionNode) -> K8sResult<NodeJoinInfo> {
    // Generate token
    let token_output = run_ssh_command(
        cp_node,
        "kubeadm token create --print-join-command",
    ).await?;

    // Parse the join command to extract token and CA hash
    let parts: Vec<&str> = token_output.split_whitespace().collect();

    let api_server = parts.iter()
        .position(|&p| p.contains(":6443"))
        .and_then(|i| parts.get(i))
        .map(|s| s.to_string())
        .unwrap_or_default();

    let token = parts.iter()
        .position(|&p| p == "--token")
        .and_then(|i| parts.get(i + 1))
        .map(|s| s.to_string())
        .unwrap_or_default();

    let ca_cert_hash = parts.iter()
        .position(|&p| p == "--discovery-token-ca-cert-hash")
        .and_then(|i| parts.get(i + 1))
        .map(|s| s.to_string());

    Ok(NodeJoinInfo {
        token,
        api_server,
        ca_cert_hash,
        control_plane: false,
        certificate_key: None,
    })
}

/// Get k3s join token
pub async fn get_k3s_token(cp_node: &ProvisionNode) -> K8sResult<String> {
    run_ssh_command(cp_node, "sudo cat /var/lib/rancher/k3s/server/node-token").await
}
