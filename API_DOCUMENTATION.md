# Horcrux API Documentation

Version: 0.1.0
Base URL: `http://localhost:8006/api`
Authentication: Bearer tokens

## Table of Contents

1. [Authentication](#authentication)
2. [Virtual Machines](#virtual-machines)
3. [Cluster Management](#cluster-management)
4. [Storage](#storage)
5. [Backup & Restore](#backup--restore)
6. [Monitoring](#monitoring)
7. [OpenTelemetry](#opentelemetry)
8. [SDN (Networking)](#sdn-networking)
9. [Firewall](#firewall)
10. [Templates](#templates)
11. [Console Access](#console-access)
12. [Alerts](#alerts)

---

## Authentication

### Login
```http
POST /auth/login
Content-Type: application/json

{
  "username": "admin",
  "password": "password"
}

Response 200:
{
  "token": "eyJ0eXAiOiJKV1QiLCJ...",
  "username": "admin",
  "expires": 1234567890
}
```

### Logout
```http
POST /auth/logout
Authorization: Bearer <token>

Response 200: OK
```

---

## Virtual Machines

### List VMs
```http
GET /vms
Authorization: Bearer <token>

Response 200:
[
  {
    "id": "vm-100",
    "name": "web-server",
    "status": "running",
    "cpu_cores": 4,
    "memory_mb": 8192,
    "architecture": "x86_64"
  }
]
```

### Create VM
```http
POST /vms
Authorization: Bearer <token>
Content-Type: application/json

{
  "id": "vm-101",
  "name": "database",
  "cpu_cores": 8,
  "memory_mb": 16384,
  "disks": [
    {
      "size_gb": 100,
      "storage_pool": "zfs-pool"
    }
  ],
  "architecture": "x86_64"
}

Response 201: Created
```

### Start VM
```http
POST /vms/{id}/start
Authorization: Bearer <token>

Response 200: OK
```

### Stop VM
```http
POST /vms/{id}/stop
Authorization: Bearer <token>

Response 200: OK
```

### Delete VM
```http
DELETE /vms/{id}
Authorization: Bearer <token>

Response 204: No Content
```

---

## Cluster Management

### List Nodes
```http
GET /cluster/nodes
Authorization: Bearer <token>

Response 200:
[
  {
    "id": "node1",
    "name": "pve-node1",
    "online": true,
    "architecture": "x86_64"
  }
]
```

### Add Node
```http
POST /cluster/nodes/{name}
Authorization: Bearer <token>
Content-Type: application/json

{
  "address": "192.168.1.101",
  "port": 8006
}

Response 201: Created
```

### Get Cluster Architecture
```http
GET /cluster/architecture
Authorization: Bearer <token>

Response 200:
{
  "nodes_by_arch": {
    "x86_64": 3,
    "aarch64": 2,
    "riscv64": 1
  },
  "total_nodes": 6
}
```

---

## Storage

### List Storage Pools
```http
GET /storage
Authorization: Bearer <token>

Response 200:
[
  {
    "id": "zfs-pool",
    "name": "ZFS Pool",
    "storage_type": "zfs",
    "available": 500,
    "total": 1000
  }
]
```

### Create Volume
```http
POST /storage/{pool_id}/volumes
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "vm-100-disk0",
  "size_gb": 50
}

Response 201: Created
```

---

## Backup & Restore

### Create Backup
```http
POST /backups
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "vm-100",
  "mode": "snapshot",
  "compression": "zstd",
  "notes": "Weekly backup"
}

Response 201:
{
  "backup_id": "backup-2025-01-01-vm-100"
}
```

### List Backups
```http
GET /backups
Authorization: Bearer <token>

Response 200:
[
  {
    "id": "backup-2025-01-01-vm-100",
    "vm_id": "vm-100",
    "size_mb": 5120,
    "created_at": 1234567890
  }
]
```

### Restore Backup
```http
POST /backups/{id}/restore
Authorization: Bearer <token>
Content-Type: application/json

{
  "target_vm_id": "vm-101"
}

Response 200: OK
```

### External Backup Providers
```http
GET /backups/providers
Authorization: Bearer <token>

Response 200:
[
  {
    "name": "s3-backup",
    "provider_type": "s3",
    "enabled": true
  }
]
```

---

## Monitoring

### Get Node Stats
```http
GET /monitoring/node
Authorization: Bearer <token>

Response 200:
{
  "cpu_usage": 45.2,
  "memory_total": 16000000000,
  "memory_used": 8000000000,
  "uptime": 864000
}
```

### Get VM Stats
```http
GET /monitoring/vms/{id}
Authorization: Bearer <token>

Response 200:
{
  "vm_id": "vm-100",
  "cpu_usage": 25.5,
  "memory_used": 4096000000,
  "disk_read_bytes": 1000000,
  "disk_write_bytes": 500000
}
```

---

## OpenTelemetry

### Get Configuration
```http
GET /observability/config
Authorization: Bearer <token>

Response 200:
{
  "enabled": true,
  "endpoint": "http://localhost:4318",
  "protocol": "http",
  "service_name": "horcrux"
}
```

### Update Configuration
```http
POST /observability/config
Authorization: Bearer <token>
Content-Type: application/json

{
  "enabled": true,
  "endpoint": "https://otlp.example.com:4318",
  "protocol": "http",
  "export_interval_secs": 60
}

Response 200: OK
```

### Export Metrics Now
```http
POST /observability/export/metrics
Authorization: Bearer <token>

Response 200: OK
```

---

## SDN (Networking)

### Create Zone
```http
POST /sdn/zones
Authorization: Bearer <token>
Content-Type: application/json

{
  "id": "zone1",
  "name": "Production Zone",
  "zone_type": "vxlan",
  "nodes": ["node1", "node2"]
}

Response 201: Created
```

### Create VNet
```http
POST /sdn/vnets
Authorization: Bearer <token>
Content-Type: application/json

{
  "id": "vnet100",
  "zone_id": "zone1",
  "name": "Web Network",
  "tag": 100,
  "vnet_type": "vlan"
}

Response 201: Created
```

### Allocate IP
```http
POST /sdn/subnets/{subnet_id}/allocate
Authorization: Bearer <token>
Content-Type: application/json

{
  "assigned_to": "vm-100"
}

Response 200:
{
  "ip": "10.0.1.10",
  "subnet_id": "subnet1"
}
```

---

## Firewall

### Create Rule
```http
POST /firewall/rules
Authorization: Bearer <token>
Content-Type: application/json

{
  "action": "accept",
  "protocol": "tcp",
  "dest_port": 80,
  "direction": "in",
  "comment": "Allow HTTP"
}

Response 201: Created
```

### Apply Firewall Rules
```http
POST /firewall/{scope}/apply
Authorization: Bearer <token>

Response 200: OK
```

---

## Templates

### Create Template
```http
POST /templates
Authorization: Bearer <token>
Content-Type: application/json

{
  "vm_id": "vm-100",
  "name": "Ubuntu 22.04 Template"
}

Response 201:
{
  "id": "tmpl-100"
}
```

### Clone Template
```http
POST /templates/{id}/clone
Authorization: Bearer <token>
Content-Type: application/json

{
  "new_vm_id": "vm-102",
  "clone_type": "linked"
}

Response 201: Created
```

---

## Console Access

### Create VNC Console
```http
POST /console/{vm_id}/vnc
Authorization: Bearer <token>

Response 200:
{
  "ticket": "ABC123...",
  "port": 5900,
  "expires_at": 1234567890
}
```

### Get WebSocket URL
```http
GET /console/{vm_id}/websocket
Authorization: Bearer <token>

Response 200:
{
  "url": "ws://localhost:5901",
  "ticket": "ABC123..."
}
```

---

## Alerts

### Create Alert Rule
```http
POST /alerts/rules
Authorization: Bearer <token>
Content-Type: application/json

{
  "name": "High CPU Alert",
  "metric": "cpu_usage",
  "comparison": "greater_than",
  "threshold": 80.0,
  "target": "vm-*",
  "severity": "warning"
}

Response 201: Created
```

### List Active Alerts
```http
GET /alerts/active
Authorization: Bearer <token>

Response 200:
[
  {
    "rule_id": "rule-1",
    "target": "vm-100",
    "status": "firing",
    "started_at": 1234567890
  }
]
```

---

## Error Responses

All endpoints may return the following error responses:

```http
400 Bad Request
{
  "error": "Invalid configuration: ..."
}

401 Unauthorized
{
  "error": "Authentication required"
}

404 Not Found
{
  "error": "Resource not found"
}

500 Internal Server Error
{
  "error": "Internal server error"
}
```

---

## Rate Limiting

- 1000 requests per hour per API token
- 100 requests per minute for backup operations

## Websockets

Console WebSocket connections use the standard WebSocket protocol:

```javascript
const ws = new WebSocket('ws://localhost:5901?ticket=ABC123');
ws.onmessage = (event) => {
  // Handle VNC data
};
```

---

## SDK & Examples

See `/docs/examples/` for language-specific SDK examples:
- Python
- JavaScript/TypeScript
- Rust
- Go

---

For detailed type definitions and schemas, see `/docs/schemas/openapi.yaml`
