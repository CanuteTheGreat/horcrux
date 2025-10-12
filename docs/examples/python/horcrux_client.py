"""
Horcrux API Client for Python

A complete Python client library for interacting with the Horcrux virtualization API.

Example usage:
    from horcrux_client import HorcruxClient

    client = HorcruxClient("http://localhost:8006", "admin", "password")

    # List VMs
    vms = client.list_vms()

    # Create and start a VM
    vm = client.create_vm("web-server", cpus=4, memory=8192, disk_size=50)
    client.start_vm(vm['id'])
"""

import requests
import json
from typing import Dict, List, Optional, Any
from datetime import datetime
import time


class HorcruxError(Exception):
    """Base exception for Horcrux API errors"""
    def __init__(self, message: str, status_code: int = None, response: Dict = None):
        self.message = message
        self.status_code = status_code
        self.response = response
        super().__init__(self.message)


class HorcruxClient:
    """
    Complete client for the Horcrux virtualization API.

    Provides methods for managing VMs, containers, storage, clustering,
    backups, and all other Horcrux features.
    """

    def __init__(self, base_url: str, username: str = None, password: str = None,
                 api_key: str = None, verify_ssl: bool = True):
        """
        Initialize the Horcrux client.

        Args:
            base_url: Base URL of the Horcrux API (e.g., "http://localhost:8006")
            username: Username for authentication (if using password auth)
            password: Password for authentication (if using password auth)
            api_key: API key for authentication (alternative to username/password)
            verify_ssl: Whether to verify SSL certificates
        """
        self.base_url = base_url.rstrip('/')
        self.session = requests.Session()
        self.session.verify = verify_ssl
        self.token = None

        if api_key:
            self.session.headers.update({'X-API-Key': api_key})
        elif username and password:
            self.login(username, password)

    def login(self, username: str, password: str, realm: str = "local") -> Dict:
        """
        Authenticate with username and password.

        Args:
            username: Username
            password: Password
            realm: Authentication realm ("local", "ldap", or "ad")

        Returns:
            Authentication response with token
        """
        response = self._post('/api/auth/login', {
            'username': username,
            'password': password,
            'realm': realm
        })

        self.token = response['token']
        self.session.headers.update({'Authorization': f'Bearer {self.token}'})
        return response

    def logout(self) -> None:
        """Logout and invalidate the current session."""
        if self.token:
            self._post('/api/auth/logout')
            self.token = None
            del self.session.headers['Authorization']

    # ========================================
    # Virtual Machines
    # ========================================

    def list_vms(self, status: str = None, hypervisor: str = None) -> List[Dict]:
        """
        List all virtual machines.

        Args:
            status: Filter by status ("running", "stopped", "paused")
            hypervisor: Filter by hypervisor ("Qemu", "Lxd", "Incus")

        Returns:
            List of VM objects
        """
        params = {}
        if status:
            params['status'] = status
        if hypervisor:
            params['hypervisor'] = hypervisor

        return self._get('/api/vms', params=params)

    def get_vm(self, vm_id: str) -> Dict:
        """Get details for a specific VM."""
        return self._get(f'/api/vms/{vm_id}')

    def create_vm(self, name: str, cpus: int = 2, memory: int = 2048,
                  disk_size: int = 20, hypervisor: str = "Qemu",
                  architecture: str = "X86_64", **kwargs) -> Dict:
        """
        Create a new virtual machine.

        Args:
            name: VM name
            cpus: Number of CPU cores
            memory: Memory in MB
            disk_size: Disk size in GB
            hypervisor: Hypervisor type ("Qemu", "Lxd", "Incus")
            architecture: Architecture ("X86_64", "Aarch64", "Riscv64", "Ppc64le")
            **kwargs: Additional VM configuration

        Returns:
            Created VM object
        """
        data = {
            'name': name,
            'cpus': cpus,
            'memory': memory,
            'disk_size': disk_size,
            'hypervisor': hypervisor,
            'architecture': architecture,
            **kwargs
        }
        return self._post('/api/vms', data)

    def start_vm(self, vm_id: str) -> Dict:
        """Start a VM."""
        return self._post(f'/api/vms/{vm_id}/start')

    def stop_vm(self, vm_id: str, force: bool = False, timeout: int = 60) -> Dict:
        """
        Stop a VM.

        Args:
            vm_id: VM ID
            force: Force stop (kill) instead of graceful shutdown
            timeout: Seconds to wait before force stop
        """
        return self._post(f'/api/vms/{vm_id}/stop', {
            'force': force,
            'timeout': timeout
        })

    def delete_vm(self, vm_id: str, purge: bool = True) -> Dict:
        """
        Delete a VM.

        Args:
            vm_id: VM ID
            purge: Delete disk images
        """
        return self._delete(f'/api/vms/{vm_id}', params={'purge': purge})

    # ========================================
    # VM Snapshots
    # ========================================

    def list_snapshots(self, vm_id: str) -> List[Dict]:
        """List all snapshots for a VM."""
        return self._get(f'/api/vms/{vm_id}/snapshots')

    def create_snapshot(self, vm_id: str, name: str, description: str = None,
                       include_memory: bool = False) -> Dict:
        """
        Create a VM snapshot.

        Args:
            vm_id: VM ID
            name: Snapshot name
            description: Optional description
            include_memory: Include RAM state (for running VMs)
        """
        data = {
            'name': name,
            'include_memory': include_memory
        }
        if description:
            data['description'] = description

        return self._post(f'/api/vms/{vm_id}/snapshots', data)

    def restore_snapshot(self, vm_id: str, snapshot_id: str,
                        restore_memory: bool = True) -> Dict:
        """Restore a VM to a snapshot."""
        return self._post(f'/api/vms/{vm_id}/snapshots/{snapshot_id}/restore', {
            'restore_memory': restore_memory
        })

    def delete_snapshot(self, vm_id: str, snapshot_id: str) -> Dict:
        """Delete a snapshot."""
        return self._delete(f'/api/vms/{vm_id}/snapshots/{snapshot_id}')

    # ========================================
    # VM Cloning
    # ========================================

    def clone_vm(self, vm_id: str, new_name: str, new_id: str = None,
                 full_clone: bool = True, start: bool = False) -> Dict:
        """
        Clone a VM.

        Args:
            vm_id: Source VM ID
            new_name: Name for the clone
            new_id: Optional ID for the clone
            full_clone: True for full copy, False for linked clone
            start: Start the VM after cloning
        """
        data = {
            'name': new_name,
            'mode': 'full' if full_clone else 'linked',
            'start': start
        }
        if new_id:
            data['id'] = new_id

        return self._post(f'/api/vms/{vm_id}/clone', data)

    def clone_vm_cross_node(self, vm_id: str, target_node: str,
                           new_id: str, new_name: str) -> Dict:
        """Clone a VM to another node in the cluster."""
        return self._post(f'/api/vms/{vm_id}/clone-cross-node', {
            'target_node': target_node,
            'new_vm_id': new_id,
            'new_name': new_name
        })

    def get_clone_job(self, job_id: str) -> Dict:
        """Get clone job status."""
        return self._get(f'/api/clone-jobs/{job_id}')

    def list_clone_jobs(self) -> List[Dict]:
        """List all clone jobs."""
        return self._get('/api/clone-jobs')

    # ========================================
    # Containers
    # ========================================

    def list_containers(self) -> List[Dict]:
        """List all containers."""
        return self._get('/api/containers')

    def create_container(self, name: str, runtime: str = "Lxc",
                        image: str = "ubuntu:22.04", cpus: int = 2,
                        memory: int = 2048, **kwargs) -> Dict:
        """
        Create a new container.

        Args:
            name: Container name
            runtime: Runtime ("Lxc", "Lxd", "Incus", "Docker", "Podman")
            image: Container image
            cpus: Number of CPUs
            memory: Memory in MB
        """
        data = {
            'name': name,
            'runtime': runtime,
            'image': image,
            'cpus': cpus,
            'memory': memory,
            **kwargs
        }
        return self._post('/api/containers', data)

    def start_container(self, container_id: str) -> Dict:
        """Start a container."""
        return self._post(f'/api/containers/{container_id}/start')

    def stop_container(self, container_id: str) -> Dict:
        """Stop a container."""
        return self._post(f'/api/containers/{container_id}/stop')

    def delete_container(self, container_id: str) -> Dict:
        """Delete a container."""
        return self._delete(f'/api/containers/{container_id}')

    def exec_in_container(self, container_id: str, command: List[str],
                         user: str = None, working_dir: str = None) -> Dict:
        """
        Execute a command in a container.

        Args:
            container_id: Container ID
            command: Command and arguments as list
            user: User to run as
            working_dir: Working directory
        """
        data = {'command': command}
        if user:
            data['user'] = user
        if working_dir:
            data['working_dir'] = working_dir

        return self._post(f'/api/containers/{container_id}/exec', data)

    # ========================================
    # Storage
    # ========================================

    def list_storage_pools(self) -> List[Dict]:
        """List all storage pools."""
        return self._get('/api/storage/pools')

    def get_storage_pool(self, pool_id: str) -> Dict:
        """Get storage pool details."""
        return self._get(f'/api/storage/pools/{pool_id}')

    def add_storage_pool(self, name: str, pool_type: str, config: Dict) -> Dict:
        """
        Add a storage pool.

        Args:
            name: Pool name
            pool_type: Type ("Zfs", "Ceph", "Lvm", "Directory")
            config: Pool configuration
        """
        return self._post('/api/storage/pools', {
            'name': name,
            'type': pool_type,
            'config': config
        })

    def remove_storage_pool(self, pool_id: str) -> Dict:
        """Remove a storage pool."""
        return self._delete(f'/api/storage/pools/{pool_id}')

    # ========================================
    # Backups
    # ========================================

    def list_backups(self, vm_id: str = None) -> List[Dict]:
        """List backups."""
        params = {'vm_id': vm_id} if vm_id else {}
        return self._get('/api/backups', params=params)

    def create_backup(self, vm_id: str, backup_type: str = "full",
                     compression: str = "zstd", description: str = None) -> Dict:
        """
        Create a VM backup.

        Args:
            vm_id: VM ID
            backup_type: "full" or "incremental"
            compression: "gzip", "zstd", "lz4", or "none"
            description: Optional description
        """
        data = {
            'vm_id': vm_id,
            'type': backup_type,
            'compression': compression
        }
        if description:
            data['description'] = description

        return self._post('/api/backups', data)

    def restore_backup(self, backup_id: str, target_vm_id: str = None) -> Dict:
        """Restore a backup."""
        data = {}
        if target_vm_id:
            data['target_vm_id'] = target_vm_id

        return self._post(f'/api/backups/{backup_id}/restore', data)

    def delete_backup(self, backup_id: str) -> Dict:
        """Delete a backup."""
        return self._delete(f'/api/backups/{backup_id}')

    # ========================================
    # Clustering
    # ========================================

    def list_cluster_nodes(self) -> List[Dict]:
        """List all cluster nodes."""
        return self._get('/api/cluster/nodes')

    def add_cluster_node(self, name: str, address: str, architecture: str) -> Dict:
        """
        Add a node to the cluster.

        Args:
            name: Node name
            address: IP address or hostname
            architecture: Node architecture
        """
        return self._post(f'/api/cluster/nodes/{name}', {
            'address': address,
            'architecture': architecture
        })

    def get_cluster_architecture(self) -> Dict:
        """Get cluster architecture summary."""
        return self._get('/api/cluster/architecture')

    # ========================================
    # High Availability
    # ========================================

    def list_ha_resources(self) -> List[Dict]:
        """List HA resources."""
        return self._get('/api/ha/resources')

    def add_ha_resource(self, vm_id: str, priority: int = 100,
                       group: str = None) -> Dict:
        """
        Add a VM to HA management.

        Args:
            vm_id: VM ID
            priority: Priority (0-255, higher = more important)
            group: Optional HA group
        """
        data = {'vm_id': vm_id, 'priority': priority}
        if group:
            data['group'] = group

        return self._post('/api/ha/resources', data)

    def remove_ha_resource(self, vm_id: str) -> Dict:
        """Remove a VM from HA management."""
        return self._delete(f'/api/ha/resources/{vm_id}')

    def get_ha_status(self) -> Dict:
        """Get HA system status."""
        return self._get('/api/ha/status')

    # ========================================
    # Migration
    # ========================================

    def migrate_vm(self, vm_id: str, target_node: str, online: bool = True,
                  bandwidth_limit: int = None) -> Dict:
        """
        Migrate a VM to another node.

        Args:
            vm_id: VM ID
            target_node: Target node name
            online: Live migration (VM stays running)
            bandwidth_limit: Network bandwidth limit in Mbps
        """
        data = {
            'target_node': target_node,
            'online': online
        }
        if bandwidth_limit:
            data['bandwidth_limit_mbps'] = bandwidth_limit

        return self._post(f'/api/migrate/{vm_id}', data)

    def get_migration_status(self, vm_id: str) -> Dict:
        """Get migration status."""
        return self._get(f'/api/migrate/{vm_id}/status')

    # ========================================
    # Monitoring
    # ========================================

    def get_node_stats(self) -> Dict:
        """Get node resource statistics."""
        return self._get('/api/monitoring/node')

    def get_vm_stats(self, vm_id: str) -> Dict:
        """Get VM resource statistics."""
        return self._get(f'/api/monitoring/vms/{vm_id}')

    def get_all_vm_stats(self) -> List[Dict]:
        """Get statistics for all VMs."""
        return self._get('/api/monitoring/vms')

    def get_metric_history(self, metric: str, start: datetime = None,
                          end: datetime = None, interval: int = 60) -> Dict:
        """
        Get historical metrics.

        Args:
            metric: Metric name (e.g., "cpu_usage", "memory_usage")
            start: Start time
            end: End time
            interval: Sample interval in seconds
        """
        params = {'interval': interval}
        if start:
            params['start'] = start.isoformat()
        if end:
            params['end'] = end.isoformat()

        return self._get(f'/api/monitoring/history/{metric}', params=params)

    # ========================================
    # Alerts
    # ========================================

    def list_alert_rules(self) -> List[Dict]:
        """List all alert rules."""
        return self._get('/api/alerts/rules')

    def create_alert_rule(self, name: str, metric: str, threshold: float,
                         condition: str = "greater_than", severity: str = "warning",
                         target_type: str = "node", target_id: str = "all") -> Dict:
        """
        Create an alert rule.

        Args:
            name: Rule name
            metric: Metric to monitor
            threshold: Threshold value
            condition: "greater_than", "less_than", or "equals"
            severity: "info", "warning", or "critical"
            target_type: "node", "vm", "container", or "storage"
            target_id: Target ID or "all"
        """
        return self._post('/api/alerts/rules', {
            'name': name,
            'metric': metric,
            'threshold': threshold,
            'condition': condition,
            'severity': severity,
            'target_type': target_type,
            'target_id': target_id,
            'enabled': True
        })

    def list_active_alerts(self) -> List[Dict]:
        """List active alerts."""
        return self._get('/api/alerts/active')

    def acknowledge_alert(self, rule_id: str, target: str, comment: str = None) -> Dict:
        """Acknowledge an alert."""
        data = {}
        if comment:
            data['comment'] = comment

        return self._post(f'/api/alerts/{rule_id}/{target}/acknowledge', data)

    # ========================================
    # Firewall
    # ========================================

    def list_firewall_rules(self) -> List[Dict]:
        """List all firewall rules."""
        return self._get('/api/firewall/rules')

    def add_firewall_rule(self, name: str, action: str, protocol: str,
                         port: int = None, source: str = "0.0.0.0/0",
                         enabled: bool = True) -> Dict:
        """
        Add a firewall rule.

        Args:
            name: Rule name
            action: "Accept", "Drop", or "Reject"
            protocol: "Tcp", "Udp", "Icmp", or "All"
            port: Port number (for TCP/UDP)
            source: Source CIDR
            enabled: Whether rule is enabled
        """
        data = {
            'name': name,
            'action': action,
            'protocol': protocol,
            'source': source,
            'enabled': enabled
        }
        if port:
            data['port'] = port

        return self._post('/api/firewall/rules', data)

    def delete_firewall_rule(self, rule_id: str) -> Dict:
        """Delete a firewall rule."""
        return self._delete(f'/api/firewall/rules/{rule_id}')

    def apply_firewall_rules(self, scope: str = "datacenter") -> Dict:
        """
        Apply firewall rules.

        Args:
            scope: "datacenter", "node", or "vm/:vm_id"
        """
        return self._post(f'/api/firewall/{scope}/apply')

    # ========================================
    # GPU Passthrough
    # ========================================

    def list_gpu_devices(self) -> List[Dict]:
        """List all GPU devices."""
        return self._get('/api/gpu/devices')

    def scan_gpu_devices(self) -> Dict:
        """Scan for GPU devices."""
        return self._post('/api/gpu/devices/scan')

    def bind_gpu_to_vfio(self, pci_address: str) -> Dict:
        """Bind a GPU to VFIO for passthrough."""
        return self._post(f'/api/gpu/devices/{pci_address}/bind-vfio')

    def unbind_gpu_from_vfio(self, pci_address: str) -> Dict:
        """Unbind a GPU from VFIO."""
        return self._post(f'/api/gpu/devices/{pci_address}/unbind-vfio')

    def check_iommu_status(self) -> Dict:
        """Check IOMMU status."""
        return self._get('/api/gpu/iommu-status')

    # ========================================
    # Helper Methods
    # ========================================

    def wait_for_vm_status(self, vm_id: str, target_status: str,
                          timeout: int = 300, interval: int = 5) -> bool:
        """
        Wait for a VM to reach a specific status.

        Args:
            vm_id: VM ID
            target_status: Target status to wait for
            timeout: Maximum time to wait in seconds
            interval: Check interval in seconds

        Returns:
            True if status reached, False if timeout
        """
        start_time = time.time()

        while time.time() - start_time < timeout:
            try:
                vm = self.get_vm(vm_id)
                if vm['status'] == target_status:
                    return True
            except HorcruxError:
                pass

            time.sleep(interval)

        return False

    def wait_for_backup(self, backup_id: str, timeout: int = 3600) -> Dict:
        """Wait for a backup to complete."""
        start_time = time.time()

        while time.time() - start_time < timeout:
            backup = self._get(f'/api/backups/{backup_id}')

            if backup['status'] == 'completed':
                return backup
            elif backup['status'] == 'failed':
                raise HorcruxError(f"Backup failed: {backup.get('error', 'Unknown error')}")

            time.sleep(5)

        raise HorcruxError("Backup timeout")

    # ========================================
    # Internal HTTP Methods
    # ========================================

    def _request(self, method: str, path: str, data: Dict = None,
                params: Dict = None) -> Any:
        """Make an HTTP request to the API."""
        url = f"{self.base_url}{path}"

        try:
            response = self.session.request(
                method=method,
                url=url,
                json=data,
                params=params,
                timeout=30
            )

            # Handle empty responses
            if response.status_code == 204 or not response.content:
                return {}

            # Try to parse JSON
            try:
                result = response.json()
            except json.JSONDecodeError:
                result = {'data': response.text}

            # Check for errors
            if response.status_code >= 400:
                error_msg = result.get('message', result.get('error', 'Unknown error'))
                raise HorcruxError(
                    message=error_msg,
                    status_code=response.status_code,
                    response=result
                )

            return result

        except requests.exceptions.RequestException as e:
            raise HorcruxError(f"Request failed: {str(e)}")

    def _get(self, path: str, params: Dict = None) -> Any:
        """Make a GET request."""
        return self._request('GET', path, params=params)

    def _post(self, path: str, data: Dict = None) -> Any:
        """Make a POST request."""
        return self._request('POST', path, data=data)

    def _put(self, path: str, data: Dict = None) -> Any:
        """Make a PUT request."""
        return self._request('PUT', path, data=data)

    def _delete(self, path: str, params: Dict = None) -> Any:
        """Make a DELETE request."""
        return self._request('DELETE', path, params=params)

    def __enter__(self):
        """Context manager entry."""
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        """Context manager exit."""
        self.logout()
