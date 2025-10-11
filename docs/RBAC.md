# Role-Based Access Control (RBAC)

Horcrux implements a comprehensive RBAC system modeled after Proxmox VE's permission model and Kubernetes RBAC.

## Architecture

The RBAC system consists of three main components:

1. **Users** - Authenticated users with assigned roles
2. **Roles** - Named collections of permissions
3. **Permissions** - Define access to specific resource paths with privilege levels

## Built-in Roles

### Administrator
- **Description**: Full system access
- **Permissions**: All privileges on all resources (/)
- **Privileges**:
  - VmAllocate, VmConfig, VmPowerMgmt, VmMigrate, VmSnapshot, VmBackup, VmAudit
  - DatastoreAllocate, DatastoreAudit
  - SysModify, SysAudit
  - UserModify
  - PoolAllocate

### VmAdmin
- **Description**: VM management access
- **Permissions**: VM operations on /api/vms/**
- **Privileges**:
  - VmAllocate, VmConfig, VmPowerMgmt, VmSnapshot, VmBackup, VmAudit

### VmUser
- **Description**: Basic VM access (start/stop/view)
- **Permissions**: Limited VM operations on /api/vms/**
- **Privileges**:
  - VmPowerMgmt, VmAudit

### StorageAdmin
- **Description**: Storage pool and datastore management
- **Permissions**: Storage operations on /api/storage/**
- **Privileges**:
  - DatastoreAllocate, DatastoreAudit, PoolAllocate

### Auditor
- **Description**: Read-only system access
- **Permissions**: Read operations on /api/**
- **Privileges**:
  - VmAudit, DatastoreAudit, SysAudit

## Privilege Types

### VM Privileges
- **VmAudit**: View VM configuration and status
- **VmAllocate**: Create and delete VMs
- **VmConfig**: Modify VM configuration
- **VmPowerMgmt**: Start, stop, restart VMs
- **VmMigrate**: Migrate VMs between nodes
- **VmSnapshot**: Create and manage VM snapshots
- **VmBackup**: Create and restore VM backups

### Storage Privileges
- **DatastoreAudit**: View storage pools and datastores
- **DatastoreAllocate**: Create and delete storage volumes
- **PoolAllocate**: Manage storage pools

### System Privileges
- **SysAudit**: View system configuration and status
- **SysModify**: Modify system settings
- **UserModify**: Manage users and permissions

## Path Matching

RBAC uses path-based permissions with wildcard support:

- **Exact match**: `/api/vms/100` matches only VM 100
- **Single-level wildcard**: `/api/vms/*` matches `/api/vms/100`, `/api/vms/101`, etc.
- **Recursive wildcard**: `/api/vms/**` matches all paths under `/api/vms/`, including nested paths
- **Root path**: `/` matches everything

## Usage in API Handlers

### Using the require_privilege! macro

```rust
use horcrux_common::auth::Privilege;
use crate::middleware::auth::AuthUser;

async fn start_vm(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<u32>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<StatusCode, ApiError> {
    // Check if user has VmPowerMgmt privilege for this VM
    require_privilege!(
        state,
        auth_user,
        &format!("/api/vms/{}", vm_id),
        Privilege::VmPowerMgmt
    )?;

    // Permission granted - proceed with operation
    // ...
    Ok(StatusCode::OK)
}
```

### Manual privilege checking

```rust
use crate::middleware::rbac::check_user_privilege;

async fn custom_operation(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<Response>, ApiError> {
    let has_permission = check_user_privilege(
        &state,
        &auth_user,
        "/api/vms/100",
        Privilege::VmConfig,
    )
    .await
    .map_err(|_| ApiError::Forbidden("RBAC check failed".to_string()))?;

    if !has_permission {
        return Err(ApiError::Forbidden("Insufficient privileges".to_string()));
    }

    // Proceed with operation
    // ...
}
```

## Authentication Integration

RBAC works in conjunction with the authentication middleware:

1. **Auth Middleware** (`middleware::auth`) validates JWT tokens, session cookies, or API keys and sets `AuthUser` in request extensions
2. **RBAC Middleware** (`middleware::rbac`) ensures authentication is present (optional global enforcement)
3. **Handler-level RBAC** checks specific privileges for the requested operation and resource

## Database Schema

### Users Table
```sql
CREATE TABLE users (
    id TEXT PRIMARY KEY,
    username TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    email TEXT,
    role TEXT NOT NULL,  -- Default role (Administrator, VmAdmin, etc.)
    realm TEXT DEFAULT 'horcrux',
    enabled INTEGER DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);
```

### API Keys Table
```sql
CREATE TABLE api_keys (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    expires_at INTEGER,
    enabled INTEGER DEFAULT 1,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_used_at TIMESTAMP,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
```

## Configuration

### Loading Custom Roles

By default, roles are defined in-memory. To load roles from a configuration file or database:

```rust
// In production, implement:
async fn load_roles_from_config() -> HashMap<String, Role> {
    // Load from /etc/horcrux/roles.yaml or database
}

// Or store roles in database:
CREATE TABLE roles (
    name TEXT PRIMARY KEY,
    description TEXT,
    permissions TEXT  -- JSON array of permissions
);
```

### Assigning Roles to Users

Currently, users have a single `role` field. To support multiple roles:

```rust
// User struct already has a roles Vec:
pub struct User {
    pub id: String,
    pub username: String,
    pub role: String,       // Primary role
    pub roles: Vec<String>, // Additional roles
    // ...
}
```

## Examples

### Creating a Custom Role

```rust
use horcrux_common::auth::{Role, Permission, Privilege};

let backup_operator = Role {
    name: "BackupOperator".to_string(),
    description: "Can create and restore backups".to_string(),
    permissions: vec![
        Permission {
            path: "/api/vms/**".to_string(),
            privileges: vec![Privilege::VmAudit, Privilege::VmBackup],
        },
        Permission {
            path: "/api/storage/**".to_string(),
            privileges: vec![Privilege::DatastoreAudit],
        },
    ],
};
```

### Checking Permissions

```rust
let rbac = RbacManager::new();
let has_permission = rbac.check_permission(
    &user,
    &roles,
    "/api/vms/100",
    Privilege::VmPowerMgmt,
).await?;
```

## Future Enhancements

- [ ] Resource pools with delegated permissions
- [ ] User groups for easier permission management
- [ ] Permission inheritance and override rules
- [ ] Audit logging for permission checks
- [ ] API for managing roles and permissions
- [ ] UI for role assignment and permission visualization
- [ ] Integration with LDAP/Active Directory groups
- [ ] Time-based access restrictions
- [ ] IP-based access controls

## Security Considerations

1. **Default Deny**: If no role matches, access is denied
2. **Least Privilege**: Assign minimum necessary privileges
3. **API Key Rotation**: Regularly rotate API keys and set expiration dates
4. **Audit Trail**: Log all permission checks and failures
5. **Session Expiration**: Sessions expire after 24 hours by default
6. **Secure Secrets**: JWT secret and API key hashes use strong cryptography
