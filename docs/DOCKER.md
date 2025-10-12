# Docker Deployment Guide

Run Horcrux in containers for easy development and testing.

## Quick Start

### Prerequisites

- Docker 20.10+ installed
- Docker Compose 2.0+ installed
- KVM support on host (for VM functionality)

### Start Horcrux

```bash
# Clone the repository
git clone https://github.com/CanuteTheGreat/horcrux.git
cd horcrux

# Start Horcrux API
docker-compose up -d

# Check status
docker-compose ps

# View logs
docker-compose logs -f horcrux-api

# Access API
curl http://localhost:8006/api/health
```

That's it! Horcrux API is now running at http://localhost:8006

## Docker Compose Services

### Core Services

#### horcrux-api (Always runs)
- Main API server
- Ports: 8006 (API), 5900-5910 (VNC)
- Volumes: Data and logs persistence
- Privileged mode for KVM access

### Optional Services

#### prometheus (Profile: monitoring)
```bash
docker-compose --profile monitoring up -d
```
- Metrics collection
- Access at http://localhost:9090

#### grafana (Profile: monitoring)
```bash
docker-compose --profile monitoring up -d
```
- Metrics visualization
- Access at http://localhost:3000
- Default credentials: admin/admin

#### horcrux-cli (Profile: cli)
```bash
docker-compose --profile cli run horcrux-cli vm list
```
- CLI tool for management
- Runs on-demand

## Usage Examples

### Create a VM

Using API:
```bash
curl -X POST http://localhost:8006/api/vms \
  -H "Content-Type: application/json" \
  -d '{
    "name": "test-vm",
    "hypervisor": "Qemu",
    "architecture": "X86_64",
    "cpus": 2,
    "memory": 2048,
    "disk_size": 20
  }'
```

Using CLI:
```bash
docker-compose --profile cli run horcrux-cli vm create \
  --name test-vm \
  --cpus 2 \
  --memory 2048 \
  --disk-size 20
```

### List VMs

```bash
# Using API
curl http://localhost:8006/api/vms

# Using CLI
docker-compose --profile cli run horcrux-cli vm list
```

### Start/Stop VMs

```bash
# Start VM
docker-compose --profile cli run horcrux-cli vm start test-vm

# Stop VM
docker-compose --profile cli run horcrux-cli vm stop test-vm
```

### View Logs

```bash
# All logs
docker-compose logs

# API logs only
docker-compose logs horcrux-api

# Follow logs
docker-compose logs -f horcrux-api

# Last 100 lines
docker-compose logs --tail=100 horcrux-api
```

## Configuration

### Environment Variables

Edit `docker-compose.yml` to customize:

```yaml
environment:
  - RUST_LOG=debug  # Change log level
  - DATABASE_URL=sqlite:///var/lib/horcrux/horcrux.db
```

### Custom Configuration File

```bash
# Copy and edit config
cp deploy/config.toml.example my-config.toml
vi my-config.toml

# Mount custom config
docker-compose run \
  -v $(pwd)/my-config.toml:/etc/horcrux/config.toml \
  horcrux-api
```

### Volumes

Persistent data is stored in Docker volumes:

```bash
# List volumes
docker volume ls | grep horcrux

# Inspect volume
docker volume inspect horcrux_horcrux-data

# Backup data
docker run --rm \
  -v horcrux_horcrux-data:/data \
  -v $(pwd):/backup \
  alpine tar czf /backup/horcrux-backup.tar.gz /data

# Restore data
docker run --rm \
  -v horcrux_horcrux-data:/data \
  -v $(pwd):/backup \
  alpine tar xzf /backup/horcrux-backup.tar.gz -C /
```

## KVM Support

### Check KVM Availability

```bash
# On host
ls -la /dev/kvm

# Should show: crw-rw-rw- 1 root kvm
```

### Enable KVM (if not available)

```bash
# Load KVM module
sudo modprobe kvm
sudo modprobe kvm_intel  # or kvm_amd for AMD

# Add user to kvm group
sudo usermod -aG kvm $USER

# Set permissions
sudo chmod 666 /dev/kvm
```

### Verify in Container

```bash
docker-compose exec horcrux-api ls -la /dev/kvm
```

## Building Images

### Build API Image

```bash
# Build
docker build -t horcrux/api:latest .

# Build with no cache
docker build --no-cache -t horcrux/api:latest .

# Build specific version
docker build -t horcrux/api:0.1.0 .
```

### Build CLI Image

```bash
docker build -f Dockerfile.cli -t horcrux/cli:latest .
```

### Multi-arch Build

```bash
# Setup buildx
docker buildx create --name horcrux-builder --use

# Build for multiple architectures
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t horcrux/api:latest \
  --push .
```

## Production Deployment

### Using Docker Compose

```yaml
# docker-compose.prod.yml
version: '3.8'

services:
  horcrux-api:
    image: horcrux/api:latest
    restart: always
    ports:
      - "8006:8006"
    volumes:
      - /data/horcrux:/var/lib/horcrux
      - /var/log/horcrux:/var/log/horcrux
    environment:
      - RUST_LOG=info
    privileged: true
    devices:
      - /dev/kvm:/dev/kvm
```

Deploy:
```bash
docker-compose -f docker-compose.prod.yml up -d
```

### Using Docker Swarm

```bash
# Initialize swarm
docker swarm init

# Create stack
docker stack deploy -c docker-compose.yml horcrux

# Check services
docker service ls

# Scale services
docker service scale horcrux_horcrux-api=3

# Remove stack
docker stack rm horcrux
```

## Troubleshooting

### Container Won't Start

**Check logs:**
```bash
docker-compose logs horcrux-api
```

**Check if port is in use:**
```bash
sudo lsof -i :8006
```

**Check KVM access:**
```bash
docker-compose exec horcrux-api ls -la /dev/kvm
```

### Database Issues

**Reset database:**
```bash
docker-compose down
docker volume rm horcrux_horcrux-data
docker-compose up -d
```

### Permission Denied

**Ensure KVM permissions:**
```bash
sudo chmod 666 /dev/kvm
sudo usermod -aG kvm $USER
```

**Restart Docker:**
```bash
sudo systemctl restart docker
docker-compose restart
```

### High Memory Usage

**Check container stats:**
```bash
docker stats horcrux-api
```

**Limit memory:**
```yaml
services:
  horcrux-api:
    mem_limit: 4g
    mem_reservation: 2g
```

### Networking Issues

**Check container network:**
```bash
docker network inspect horcrux_horcrux-net
```

**Test connectivity:**
```bash
docker-compose exec horcrux-api curl http://localhost:8006/api/health
```

## Development Workflow

### Live Development

```bash
# Mount source code
docker-compose run \
  -v $(pwd):/app \
  horcrux-api cargo watch -x run
```

### Run Tests

```bash
# Run tests in container
docker-compose run horcrux-api cargo test

# Run specific test
docker-compose run horcrux-api cargo test test_vm_lifecycle
```

### Shell Access

```bash
# Get shell in running container
docker-compose exec horcrux-api /bin/bash

# Or start new container with shell
docker-compose run horcrux-api /bin/bash
```

## Cleanup

### Stop Services

```bash
# Stop all services
docker-compose down

# Stop and remove volumes
docker-compose down -v

# Stop and remove images
docker-compose down --rmi all
```

### Remove Everything

```bash
# Stop and remove all
docker-compose down -v --rmi all

# Prune system
docker system prune -a --volumes
```

## Best Practices

### Security

1. **Don't run as root in production**
2. **Use secrets for sensitive data**
3. **Enable TLS for API**
4. **Regularly update images**
5. **Scan images for vulnerabilities**

```bash
# Scan image
docker scan horcrux/api:latest
```

### Performance

1. **Use volumes for persistent data**
2. **Limit container resources**
3. **Use multi-stage builds**
4. **Enable BuildKit for faster builds**

```bash
export DOCKER_BUILDKIT=1
```

### Monitoring

1. **Enable Prometheus metrics**
2. **Use health checks**
3. **Monitor container logs**
4. **Track resource usage**

```bash
docker-compose --profile monitoring up -d
```

## Additional Resources

- [Docker Documentation](https://docs.docker.com/)
- [Docker Compose Reference](https://docs.docker.com/compose/compose-file/)
- [Horcrux Quick Start](../QUICKSTART.md)
- [Horcrux Deployment Guide](../DEPLOYMENT.md)

---

**Need help?** [GitHub Issues](https://github.com/CanuteTheGreat/horcrux/issues)
