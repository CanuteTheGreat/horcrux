# Kubernetes Integration Guide

Horcrux provides comprehensive Kubernetes cluster management, enabling you to manage K8s workloads alongside VMs and containers from a unified platform.

## Table of Contents

- [Overview](#overview)
- [Prerequisites](#prerequisites)
- [Enabling Kubernetes Support](#enabling-kubernetes-support)
- [Cluster Management](#cluster-management)
- [Workload Management](#workload-management)
- [Networking](#networking)
- [Configuration & Storage](#configuration--storage)
- [Helm Integration](#helm-integration)
- [Observability](#observability)
- [WebSocket Events](#websocket-events)
- [CLI Commands](#cli-commands)
- [API Reference](#api-reference)

## Overview

Horcrux's Kubernetes integration provides:

- **Multi-cluster management**: Connect and manage multiple K8s clusters
- **Cluster provisioning**: Deploy k3s or kubeadm clusters via SSH
- **Workload orchestration**: Full lifecycle management of pods, deployments, services, etc.
- **Helm support**: Install, upgrade, and manage Helm releases
- **Real-time monitoring**: Metrics, events, and log streaming via WebSocket
- **Secure credential storage**: Kubeconfig stored in Vault or encrypted in database

## Prerequisites

- Kubernetes cluster (v1.28+) or nodes for provisioning
- `kubectl` access (for cluster connection)
- SSH access (for cluster provisioning)
- Helm 3.x (for Helm features)

## Enabling Kubernetes Support

Kubernetes support is enabled via the `kubernetes` feature flag:

```bash
# Build with Kubernetes support
cargo build -p horcrux-api --features kubernetes --release

# Or using the Makefile
make build-k8s
```

### Cargo.toml Dependencies

```toml
[dependencies]
kube = { version = "0.98", features = ["runtime", "derive", "ws"], optional = true }
k8s-openapi = { version = "0.24", features = ["v1_32"], optional = true }

[features]
kubernetes = ["kube", "k8s-openapi"]
```

## Cluster Management

### Connecting to an Existing Cluster

```bash
# Via CLI
horcrux k8s connect --name production --kubeconfig ~/.kube/config --context my-cluster

# Via API
curl -X POST http://localhost:8006/api/k8s/clusters \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "name": "production",
    "kubeconfig": "<base64-encoded-kubeconfig>",
    "context": "my-cluster"
  }'
```

### Provisioning a New Cluster

#### k3s Cluster

```bash
# Via API
curl -X POST http://localhost:8006/api/k8s/clusters/provision \
  -H "Content-Type: application/json" \
  -d '{
    "name": "dev-cluster",
    "provider": "k3s",
    "nodes": [
      {
        "address": "192.168.1.10",
        "role": "control-plane",
        "ssh_user": "root",
        "ssh_key_path": "/root/.ssh/id_rsa"
      },
      {
        "address": "192.168.1.11",
        "role": "worker",
        "ssh_user": "root",
        "ssh_key_path": "/root/.ssh/id_rsa"
      }
    ],
    "k3s_options": {
      "version": "v1.30.0+k3s1",
      "disable": ["traefik"],
      "extra_args": ["--disable-cloud-controller"]
    }
  }'
```

#### kubeadm Cluster

```bash
curl -X POST http://localhost:8006/api/k8s/clusters/provision \
  -H "Content-Type: application/json" \
  -d '{
    "name": "prod-cluster",
    "provider": "kubeadm",
    "nodes": [
      {
        "address": "192.168.1.20",
        "role": "control-plane",
        "ssh_user": "ubuntu"
      }
    ],
    "kubeadm_options": {
      "kubernetes_version": "1.30.0",
      "pod_network_cidr": "10.244.0.0/16",
      "service_cidr": "10.96.0.0/12",
      "cni_plugin": "calico"
    }
  }'
```

### Cluster Operations

```bash
# List clusters
GET /api/k8s/clusters

# Get cluster details
GET /api/k8s/clusters/:cluster_id

# Check cluster health
GET /api/k8s/clusters/:cluster_id/health

# Disconnect cluster
DELETE /api/k8s/clusters/:cluster_id

# Reconnect cluster
POST /api/k8s/clusters/:cluster_id/reconnect

# Upgrade cluster
POST /api/k8s/clusters/:cluster_id/upgrade
{
  "target_version": "1.31.0"
}
```

## Workload Management

### Pods

```bash
# List pods in namespace
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods

# Get pod details
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:name

# Delete pod
DELETE /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:name

# Get pod logs
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:name/logs?container=app&tail=100

# Execute command in pod (WebSocket)
WS /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:name/exec
{
  "command": ["sh", "-c", "ls -la"],
  "container": "app",
  "tty": true,
  "stdin": true
}
```

### Deployments

```bash
# List deployments
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments

# Create deployment
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments
{
  "name": "nginx",
  "replicas": 3,
  "image": "nginx:1.25",
  "ports": [{"container_port": 80}],
  "resources": {
    "requests": {"cpu": "100m", "memory": "128Mi"},
    "limits": {"cpu": "500m", "memory": "512Mi"}
  }
}

# Scale deployment
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:name/scale
{
  "replicas": 5
}

# Restart deployment (rolling restart)
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:name/restart

# Rollback deployment
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:name/rollback
{
  "revision": 2
}

# Update deployment
PUT /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:name
{
  "image": "nginx:1.26",
  "replicas": 4
}

# Delete deployment
DELETE /api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:name
```

### StatefulSets

```bash
# List StatefulSets
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets

# Create StatefulSet
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets
{
  "name": "postgres",
  "replicas": 3,
  "service_name": "postgres-headless",
  "image": "postgres:16",
  "volume_claim_templates": [
    {
      "name": "data",
      "storage_class": "standard",
      "size": "10Gi"
    }
  ]
}

# Scale StatefulSet
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name/scale
{
  "replicas": 5
}
```

### DaemonSets

```bash
# List DaemonSets
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets

# Create DaemonSet
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets
{
  "name": "fluentd",
  "image": "fluentd:v1.16",
  "node_selector": {"kubernetes.io/os": "linux"}
}
```

### Jobs and CronJobs

```bash
# Create Job
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs
{
  "name": "backup-job",
  "image": "backup-tool:latest",
  "command": ["backup", "--full"],
  "backoff_limit": 3,
  "ttl_seconds_after_finished": 3600
}

# Create CronJob
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs
{
  "name": "daily-backup",
  "schedule": "0 2 * * *",
  "image": "backup-tool:latest",
  "command": ["backup", "--incremental"],
  "concurrency_policy": "Forbid"
}
```

## Networking

### Services

```bash
# List services
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/services

# Create ClusterIP service
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/services
{
  "name": "web-service",
  "service_type": "ClusterIP",
  "selector": {"app": "web"},
  "ports": [
    {"port": 80, "target_port": 8080, "protocol": "TCP"}
  ]
}

# Create LoadBalancer service
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/services
{
  "name": "web-lb",
  "service_type": "LoadBalancer",
  "selector": {"app": "web"},
  "ports": [{"port": 443, "target_port": 8443}],
  "load_balancer_ip": "10.0.0.100"
}
```

### Ingress

```bash
# List ingresses
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses

# Create ingress
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses
{
  "name": "web-ingress",
  "ingress_class": "nginx",
  "rules": [
    {
      "host": "app.example.com",
      "paths": [
        {
          "path": "/",
          "path_type": "Prefix",
          "backend": {
            "service_name": "web-service",
            "service_port": 80
          }
        }
      ]
    }
  ],
  "tls": [
    {
      "hosts": ["app.example.com"],
      "secret_name": "app-tls"
    }
  ]
}
```

### NetworkPolicies

```bash
# Create NetworkPolicy
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/networkpolicies
{
  "name": "deny-all",
  "pod_selector": {},
  "policy_types": ["Ingress", "Egress"],
  "ingress": [],
  "egress": []
}

# Allow specific ingress
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/networkpolicies
{
  "name": "allow-web",
  "pod_selector": {"app": "web"},
  "ingress": [
    {
      "from": [
        {"namespace_selector": {"name": "frontend"}}
      ],
      "ports": [{"port": 80, "protocol": "TCP"}]
    }
  ]
}
```

## Configuration & Storage

### ConfigMaps

```bash
# List ConfigMaps
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps

# Create ConfigMap
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps
{
  "name": "app-config",
  "data": {
    "DATABASE_URL": "postgres://db:5432/app",
    "LOG_LEVEL": "info"
  }
}
```

### Secrets

```bash
# List Secrets
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets

# Create Secret
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets
{
  "name": "app-secrets",
  "type": "Opaque",
  "data": {
    "API_KEY": "base64-encoded-value",
    "DB_PASSWORD": "base64-encoded-value"
  }
}

# Create TLS Secret
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets
{
  "name": "tls-cert",
  "type": "kubernetes.io/tls",
  "data": {
    "tls.crt": "base64-cert",
    "tls.key": "base64-key"
  }
}
```

### PersistentVolumeClaims

```bash
# List PVCs
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs

# Create PVC
POST /api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs
{
  "name": "data-pvc",
  "storage_class": "standard",
  "access_modes": ["ReadWriteOnce"],
  "storage": "10Gi"
}
```

### StorageClasses

```bash
# List StorageClasses (cluster-scoped)
GET /api/k8s/clusters/:cluster_id/storageclasses

# Create StorageClass
POST /api/k8s/clusters/:cluster_id/storageclasses
{
  "name": "fast-ssd",
  "provisioner": "kubernetes.io/gce-pd",
  "parameters": {
    "type": "pd-ssd"
  },
  "reclaim_policy": "Retain",
  "allow_volume_expansion": true
}
```

## Helm Integration

### Repository Management

```bash
# List Helm repositories
GET /api/k8s/helm/repos

# Add repository
POST /api/k8s/helm/repos
{
  "name": "bitnami",
  "url": "https://charts.bitnami.com/bitnami"
}

# Update repositories
POST /api/k8s/helm/repos/update

# Search charts
GET /api/k8s/helm/charts/search?query=nginx
```

### Release Management

```bash
# List releases in cluster
GET /api/k8s/clusters/:cluster_id/helm/releases?namespace=default

# Install chart
POST /api/k8s/clusters/:cluster_id/helm/releases
{
  "name": "my-nginx",
  "chart": "bitnami/nginx",
  "namespace": "default",
  "version": "15.0.0",
  "values": {
    "replicaCount": 3,
    "service": {
      "type": "LoadBalancer"
    }
  }
}

# Upgrade release
PUT /api/k8s/clusters/:cluster_id/helm/releases/:name
{
  "chart": "bitnami/nginx",
  "version": "15.1.0",
  "values": {
    "replicaCount": 5
  }
}

# Get release history
GET /api/k8s/clusters/:cluster_id/helm/releases/:name/history

# Rollback release
POST /api/k8s/clusters/:cluster_id/helm/releases/:name/rollback
{
  "revision": 2
}

# Get release values
GET /api/k8s/clusters/:cluster_id/helm/releases/:name/values

# Uninstall release
DELETE /api/k8s/clusters/:cluster_id/helm/releases/:name
```

## Observability

### Metrics

```bash
# Get node metrics
GET /api/k8s/clusters/:cluster_id/metrics/nodes

# Get pod metrics
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/metrics/pods
```

Response example:
```json
{
  "nodes": [
    {
      "name": "node-1",
      "cpu_usage": "250m",
      "memory_usage": "1.5Gi",
      "cpu_percent": 12.5,
      "memory_percent": 37.5
    }
  ]
}
```

### Events

```bash
# List cluster events
GET /api/k8s/clusters/:cluster_id/events?namespace=default&limit=100

# Stream events via WebSocket
WS /api/k8s/clusters/:cluster_id/events/watch
```

### Logs

```bash
# Get pod logs
GET /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:name/logs?container=app&tail=100&follow=false

# Stream logs via WebSocket
WS /api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:name/logs/stream
```

## WebSocket Events

Subscribe to real-time K8s events via WebSocket:

```javascript
const ws = new WebSocket('ws://localhost:8006/api/ws');

// Subscribe to K8s topics
ws.send(JSON.stringify({
  type: 'subscribe',
  topics: ['k8s:pods', 'k8s:deployments', 'k8s:events', 'helm:releases']
}));

ws.onmessage = (event) => {
  const data = JSON.parse(event.data);
  switch (data.type) {
    case 'K8sPodStatusChanged':
      console.log(`Pod ${data.pod_name} status: ${data.old_status} -> ${data.new_status}`);
      break;
    case 'K8sDeploymentScaled':
      console.log(`Deployment ${data.name} scaled: ${data.old_replicas} -> ${data.new_replicas}`);
      break;
    case 'K8sEvent':
      console.log(`Event: ${data.reason} - ${data.message}`);
      break;
    case 'HelmReleaseStatusChanged':
      console.log(`Helm release ${data.release_name}: ${data.status}`);
      break;
  }
};
```

### Available Topics

| Topic | Events |
|-------|--------|
| `k8s:pods` | Pod status changes |
| `k8s:deployments` | Deployment scaling, rollouts |
| `k8s:events` | Kubernetes events |
| `k8s:logs` | Log lines (when streaming) |
| `k8s:nodes` | Node status changes |
| `k8s:clusters` | Cluster connect/disconnect |
| `helm:releases` | Helm release status changes |

## CLI Commands

```bash
# Cluster management
horcrux k8s clusters list
horcrux k8s clusters connect --name prod --kubeconfig ~/.kube/config
horcrux k8s clusters disconnect prod-cluster-id
horcrux k8s clusters health prod-cluster-id

# Workloads
horcrux k8s pods list --cluster prod --namespace default
horcrux k8s deployments list --cluster prod --namespace default
horcrux k8s deployments scale nginx --cluster prod --namespace default --replicas 5

# Helm
horcrux k8s helm repos add bitnami https://charts.bitnami.com/bitnami
horcrux k8s helm install my-nginx bitnami/nginx --cluster prod --namespace default
horcrux k8s helm upgrade my-nginx bitnami/nginx --cluster prod --set replicaCount=3
horcrux k8s helm rollback my-nginx 2 --cluster prod

# Exec into pod
horcrux k8s exec my-pod --cluster prod --namespace default -- /bin/sh

# Port forward
horcrux k8s port-forward my-pod 8080:80 --cluster prod --namespace default
```

## API Reference

### Clusters

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/k8s/clusters` | GET | List all clusters |
| `/api/k8s/clusters` | POST | Connect cluster |
| `/api/k8s/clusters/provision` | POST | Provision new cluster |
| `/api/k8s/clusters/:id` | GET | Get cluster details |
| `/api/k8s/clusters/:id` | DELETE | Disconnect cluster |
| `/api/k8s/clusters/:id/health` | GET | Health check |
| `/api/k8s/clusters/:id/reconnect` | POST | Reconnect cluster |
| `/api/k8s/clusters/:id/upgrade` | POST | Upgrade cluster |

### Namespaces

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/k8s/clusters/:id/namespaces` | GET | List namespaces |
| `/api/k8s/clusters/:id/namespaces` | POST | Create namespace |
| `/api/k8s/clusters/:id/namespaces/:ns` | DELETE | Delete namespace |

### Workloads

| Endpoint | Method | Description |
|----------|--------|-------------|
| `.../pods` | GET | List pods |
| `.../pods/:name` | GET | Get pod |
| `.../pods/:name` | DELETE | Delete pod |
| `.../pods/:name/logs` | GET | Get logs |
| `.../pods/:name/exec` | WS | Execute command |
| `.../deployments` | GET/POST | List/Create deployments |
| `.../deployments/:name` | GET/PUT/DELETE | CRUD deployment |
| `.../deployments/:name/scale` | POST | Scale deployment |
| `.../deployments/:name/restart` | POST | Restart deployment |
| `.../deployments/:name/rollback` | POST | Rollback deployment |
| `.../statefulsets` | GET/POST | List/Create statefulsets |
| `.../statefulsets/:name/scale` | POST | Scale statefulset |
| `.../daemonsets` | GET/POST/DELETE | CRUD daemonsets |
| `.../jobs` | GET/POST/DELETE | CRUD jobs |
| `.../cronjobs` | GET/POST/DELETE | CRUD cronjobs |

### Networking

| Endpoint | Method | Description |
|----------|--------|-------------|
| `.../services` | GET/POST | List/Create services |
| `.../services/:name` | GET/PUT/DELETE | CRUD service |
| `.../ingresses` | GET/POST | List/Create ingresses |
| `.../ingresses/:name` | GET/PUT/DELETE | CRUD ingress |
| `.../networkpolicies` | GET/POST/DELETE | CRUD network policies |

### Config & Storage

| Endpoint | Method | Description |
|----------|--------|-------------|
| `.../configmaps` | GET/POST | List/Create configmaps |
| `.../configmaps/:name` | GET/PUT/DELETE | CRUD configmap |
| `.../secrets` | GET/POST | List/Create secrets |
| `.../secrets/:name` | GET/PUT/DELETE | CRUD secret |
| `.../pvcs` | GET/POST | List/Create PVCs |
| `.../pvcs/:name` | GET/DELETE | Get/Delete PVC |
| `/api/k8s/clusters/:id/storageclasses` | GET/POST/DELETE | StorageClasses |

### Helm

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/k8s/helm/repos` | GET/POST | List/Add repos |
| `/api/k8s/helm/charts/search` | GET | Search charts |
| `/api/k8s/clusters/:id/helm/releases` | GET/POST | List/Install releases |
| `/api/k8s/clusters/:id/helm/releases/:name` | GET/PUT/DELETE | CRUD release |
| `/api/k8s/clusters/:id/helm/releases/:name/rollback` | POST | Rollback release |
| `/api/k8s/clusters/:id/helm/releases/:name/history` | GET | Release history |
| `/api/k8s/clusters/:id/helm/releases/:name/values` | GET | Release values |

### Metrics & Events

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/api/k8s/clusters/:id/metrics/nodes` | GET | Node metrics |
| `.../metrics/pods` | GET | Pod metrics |
| `/api/k8s/clusters/:id/events` | GET | List events |
| `/api/k8s/clusters/:id/events/watch` | WS | Stream events |

## Troubleshooting

### Common Issues

**Connection refused**
```bash
# Check kubeconfig
kubectl cluster-info --kubeconfig ~/.kube/config

# Verify API server is accessible
curl -k https://kubernetes-api:6443/healthz
```

**Permission denied**
```bash
# Check RBAC permissions
kubectl auth can-i --list

# Verify service account has required permissions
kubectl get clusterrolebindings -o wide | grep <service-account>
```

**Helm chart not found**
```bash
# Update repositories
horcrux k8s helm repos update

# Search for chart
horcrux k8s helm search <chart-name>
```

### Debug Mode

Enable debug logging for Kubernetes operations:

```toml
# config.toml
[logging]
level = "debug"
kubernetes = "trace"
```

## Security Considerations

1. **Kubeconfig Storage**: Kubeconfigs are stored encrypted in the database or in HashiCorp Vault when enabled
2. **RBAC**: Use Kubernetes RBAC to limit Horcrux's permissions to required resources only
3. **Network Policies**: Consider restricting Horcrux API server access to control plane network
4. **Audit Logging**: All K8s operations are logged with user attribution
5. **Secret Handling**: Secrets are never logged and are encrypted at rest
