# Terraform Provider for Horcrux

Manage Horcrux virtualization resources using Terraform infrastructure as code.

## Installation

### Building from Source

```bash
# Build the provider
cargo build -p terraform-provider-horcrux --release

# Install to Terraform plugins directory
mkdir -p ~/.terraform.d/plugins/horcrux.local/canutethegreat/horcrux/0.1.0/linux_amd64
cp target/release/terraform-provider-horcrux ~/.terraform.d/plugins/horcrux.local/canutethegreat/horcrux/0.1.0/linux_amd64/
```

### Using the Provider

Add to your Terraform configuration:

```hcl
terraform {
  required_providers {
    horcrux = {
      source  = "horcrux.local/canutethegreat/horcrux"
      version = "0.1.0"
    }
  }
}
```

## Provider Configuration

```hcl
provider "horcrux" {
  endpoint = "http://localhost:8006"  # Horcrux API endpoint

  # Authentication option 1: API token
  api_token = "your-api-token"

  # Authentication option 2: Username/password
  # username = "admin"
  # password = "secret"

  # Optional: Skip TLS verification (not recommended for production)
  # insecure = false
}
```

### Environment Variables

The provider supports environment variables for sensitive configuration:

```bash
export HORCRUX_ENDPOINT="http://localhost:8006"
export HORCRUX_API_TOKEN="your-api-token"
# Or
export HORCRUX_USERNAME="admin"
export HORCRUX_PASSWORD="secret"
```

## Resources

### horcrux_vm

Manages a virtual machine.

#### Example Usage

```hcl
resource "horcrux_vm" "web_server" {
  id           = "vm-100"
  name         = "web-server"
  hypervisor   = "Qemu"      # Qemu, Lxd, Incus
  architecture = "X86_64"    # X86_64, Aarch64, Riscv64

  cpus      = 4
  memory    = 8192   # MB
  disk_size = 50     # GB

  description = "Production web server"
  tags        = ["production", "web"]
}

# Multiple VMs using count
resource "horcrux_vm" "worker" {
  count = 3

  id           = "vm-${200 + count.index}"
  name         = "worker-${count.index + 1}"
  cpus         = 2
  memory       = 4096
  disk_size    = 20

  tags = ["worker", "cluster"]
}
```

#### Argument Reference

| Argument | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `id` | string | Yes | - | VM ID (e.g., vm-100) |
| `name` | string | Yes | - | Display name |
| `hypervisor` | string | No | Qemu | Hypervisor type (Qemu, Lxd, Incus) |
| `architecture` | string | No | X86_64 | CPU architecture |
| `cpus` | number | Yes | - | Number of CPU cores |
| `memory` | number | Yes | - | Memory in MB |
| `disk_size` | number | Yes | - | Disk size in GB |
| `description` | string | No | - | VM description |
| `tags` | list(string) | No | [] | Tags for organization |

#### Attribute Reference

| Attribute | Type | Description |
|-----------|------|-------------|
| `status` | string | Current VM status (running, stopped, etc.) |
| `node` | string | Node where VM is running |

---

### horcrux_container

Manages a container.

#### Example Usage

```hcl
resource "horcrux_container" "nginx" {
  id      = "ct-100"
  name    = "nginx"
  runtime = "Docker"  # Docker, Podman, Lxc
  image   = "nginx:latest"

  cpus   = 0.5
  memory = 512

  environment = {
    NGINX_HOST = "example.com"
    NGINX_PORT = "80"
  }

  port {
    host_port      = 8080
    container_port = 80
    protocol       = "tcp"
  }

  port {
    host_port      = 8443
    container_port = 443
    protocol       = "tcp"
  }
}

# Redis container
resource "horcrux_container" "redis" {
  id      = "ct-101"
  name    = "redis"
  runtime = "Docker"
  image   = "redis:7-alpine"

  memory = 256

  port {
    host_port      = 6379
    container_port = 6379
  }
}
```

#### Argument Reference

| Argument | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `id` | string | Yes | - | Container ID |
| `name` | string | Yes | - | Container name |
| `runtime` | string | Yes | - | Runtime (Docker, Podman, Lxc) |
| `image` | string | Yes | - | Container image |
| `cpus` | number | No | - | CPU limit (e.g., 0.5) |
| `memory` | number | No | - | Memory limit in MB |
| `environment` | map(string) | No | {} | Environment variables |
| `command` | list(string) | No | - | Command to run |
| `port` | block | No | - | Port mappings |

#### Port Block

| Argument | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `host_port` | number | Yes | - | Port on the host |
| `container_port` | number | Yes | - | Port in container |
| `protocol` | string | No | tcp | Protocol (tcp/udp) |

---

### horcrux_storage_pool

Manages a storage pool.

#### Example Usage

```hcl
# Directory storage
resource "horcrux_storage_pool" "local" {
  id        = "pool-local"
  name      = "local"
  pool_type = "Directory"
  path      = "/var/lib/horcrux/images"
}

# ZFS storage
resource "horcrux_storage_pool" "zfs_pool" {
  id        = "pool-zfs"
  name      = "zfs-data"
  pool_type = "Zfs"
  path      = "rpool/horcrux"
}
```

#### Argument Reference

| Argument | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `id` | string | Yes | - | Storage pool ID |
| `name` | string | Yes | - | Pool name |
| `pool_type` | string | Yes | - | Type (Directory, Zfs, Lvm, Ceph, Nfs) |
| `path` | string | No | - | Path for directory/ZFS storage |

#### Attribute Reference

| Attribute | Type | Description |
|-----------|------|-------------|
| `total_bytes` | number | Total capacity |
| `used_bytes` | number | Used space |
| `available_bytes` | number | Available space |

---

### horcrux_firewall_rule

Manages a firewall rule.

#### Example Usage

```hcl
# Allow SSH
resource "horcrux_firewall_rule" "allow_ssh" {
  name      = "allow-ssh"
  action    = "Accept"
  direction = "in"
  protocol  = "Tcp"
  port      = 22
  source    = "10.0.0.0/8"
  enabled   = true
  priority  = 100
}

# Allow HTTPS
resource "horcrux_firewall_rule" "allow_https" {
  name      = "allow-https"
  action    = "Accept"
  direction = "in"
  protocol  = "Tcp"
  port      = 443
  enabled   = true
  priority  = 110
}

# Drop all other inbound
resource "horcrux_firewall_rule" "drop_all" {
  name      = "drop-all"
  action    = "Drop"
  direction = "in"
  enabled   = true
  priority  = 1000  # Lower priority = processed last
}
```

#### Argument Reference

| Argument | Type | Required | Default | Description |
|----------|------|----------|---------|-------------|
| `name` | string | Yes | - | Rule name |
| `action` | string | Yes | - | Action (Accept, Drop, Reject) |
| `direction` | string | No | in | Direction (in, out) |
| `protocol` | string | No | - | Protocol (Tcp, Udp, Icmp) |
| `port` | number | No | - | Port number |
| `source` | string | No | - | Source CIDR |
| `destination` | string | No | - | Destination CIDR |
| `enabled` | bool | No | true | Whether rule is enabled |
| `priority` | number | No | 0 | Priority (lower = higher priority) |

#### Attribute Reference

| Attribute | Type | Description |
|-----------|------|-------------|
| `id` | string | Generated rule ID |

---

## Complete Example

```hcl
terraform {
  required_providers {
    horcrux = {
      source  = "horcrux.local/canutethegreat/horcrux"
      version = "0.1.0"
    }
  }
}

provider "horcrux" {
  endpoint  = "http://192.168.1.100:8006"
  api_token = var.horcrux_token
}

variable "horcrux_token" {
  description = "Horcrux API token"
  type        = string
  sensitive   = true
}

# Storage pool for VMs
resource "horcrux_storage_pool" "vm_storage" {
  id        = "pool-vms"
  name      = "vm-storage"
  pool_type = "Directory"
  path      = "/var/lib/horcrux/vms"
}

# Web server VM
resource "horcrux_vm" "web" {
  id           = "vm-100"
  name         = "web-server"
  cpus         = 4
  memory       = 8192
  disk_size    = 50
  description  = "Production web server"
  tags         = ["production", "web"]

  depends_on = [horcrux_storage_pool.vm_storage]
}

# Database VM
resource "horcrux_vm" "database" {
  id           = "vm-101"
  name         = "database"
  cpus         = 8
  memory       = 16384
  disk_size    = 200
  description  = "PostgreSQL database server"
  tags         = ["production", "database"]

  depends_on = [horcrux_storage_pool.vm_storage]
}

# Redis cache container
resource "horcrux_container" "redis" {
  id      = "ct-100"
  name    = "redis"
  runtime = "Docker"
  image   = "redis:7-alpine"
  memory  = 512

  port {
    host_port      = 6379
    container_port = 6379
  }
}

# Firewall rules
resource "horcrux_firewall_rule" "allow_web" {
  name      = "allow-web"
  action    = "Accept"
  protocol  = "Tcp"
  port      = 443
  enabled   = true
  priority  = 100
}

resource "horcrux_firewall_rule" "allow_ssh_internal" {
  name      = "allow-ssh-internal"
  action    = "Accept"
  protocol  = "Tcp"
  port      = 22
  source    = "10.0.0.0/8"
  enabled   = true
  priority  = 110
}

# Outputs
output "web_vm_status" {
  value = horcrux_vm.web.status
}

output "database_vm_status" {
  value = horcrux_vm.database.status
}

output "redis_container_status" {
  value = horcrux_container.redis.status
}
```

## Importing Existing Resources

Import existing resources into Terraform state:

```bash
# Import a VM
terraform import horcrux_vm.existing vm-100

# Import a container
terraform import horcrux_container.existing ct-100

# Import a storage pool
terraform import horcrux_storage_pool.existing pool-local

# Import a firewall rule
terraform import horcrux_firewall_rule.existing rule-id-123
```

## State Management

The provider stores resource state in the Terraform state file. For team environments, configure a remote backend:

```hcl
terraform {
  backend "s3" {
    bucket = "my-terraform-state"
    key    = "horcrux/terraform.tfstate"
    region = "us-east-1"
  }
}
```

## Debugging

Enable debug logging:

```bash
export TF_LOG=DEBUG
terraform apply
```

Provider-specific logs:

```bash
export TF_LOG_PROVIDER=DEBUG
terraform apply
```

## Limitations

- Container updates require recreation (use `lifecycle { create_before_destroy = true }`)
- Storage pool updates are not supported (recreate the resource)
- Firewall rule updates require recreation

## Contributing

The provider source code is in `terraform-provider-horcrux/`. To contribute:

1. Make changes to the Rust source
2. Run tests: `cargo test -p terraform-provider-horcrux`
3. Build: `cargo build -p terraform-provider-horcrux --release`
4. Test with Terraform
