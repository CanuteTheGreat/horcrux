///! Audit middleware for automatic logging of HTTP requests and RBAC checks
///!
///! Automatically captures and logs security-relevant HTTP operations

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;
use tracing::{debug, info};

use super::{create_event, AuditEvent, AuditEventType, AuditResult, AuditSeverity};

/// Extract source IP from request
fn extract_source_ip(request: &Request) -> Option<String> {
    // Try X-Forwarded-For header first (for proxies)
    if let Some(forwarded) = request.headers().get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            // Take the first IP in the list
            return Some(forwarded_str.split(',').next()?.trim().to_string());
        }
    }

    // Try X-Real-IP header
    if let Some(real_ip) = request.headers().get("x-real-ip") {
        if let Ok(ip_str) = real_ip.to_str() {
            return Some(ip_str.to_string());
        }
    }

    // TODO: Could also extract from connection info if available
    None
}

/// Extract username from request extensions (set by auth middleware)
fn extract_username(request: &Request) -> Option<String> {
    // The auth middleware should set the authenticated user in extensions
    request
        .extensions()
        .get::<crate::middleware::auth::AuthUser>()
        .map(|user| user.username.clone())
}

/// Determine audit event type from HTTP method and path
fn determine_event_type(method: &str, path: &str) -> Option<AuditEventType> {
    let path_lower = path.to_lowercase();

    // Authentication events
    if path_lower.contains("/auth/login") {
        return Some(if method == "POST" {
            AuditEventType::Login
        } else {
            return None;
        });
    }

    if path_lower.contains("/auth/logout") {
        return Some(AuditEventType::Logout);
    }

    if path_lower.contains("/auth/password") {
        return Some(AuditEventType::PasswordChanged);
    }

    // VM operations
    if path_lower.contains("/vms") {
        if method == "POST" {
            return Some(AuditEventType::VmCreated);
        } else if method == "DELETE" {
            return Some(AuditEventType::VmDeleted);
        }
    }

    if path_lower.contains("/vms/") && path_lower.contains("/start") {
        return Some(AuditEventType::VmStarted);
    }

    if path_lower.contains("/vms/") && path_lower.contains("/stop") {
        return Some(AuditEventType::VmStopped);
    }

    if path_lower.contains("/vms/") && path_lower.contains("/restart") {
        return Some(AuditEventType::VmRestarted);
    }

    if path_lower.contains("/vms/") && path_lower.contains("/migrate") {
        return Some(AuditEventType::VmMigrated);
    }

    // Storage operations
    if path_lower.contains("/storage/pools") && method == "POST" {
        return Some(AuditEventType::StoragePoolCreated);
    }

    if path_lower.contains("/storage/pools") && method == "DELETE" {
        return Some(AuditEventType::StoragePoolDeleted);
    }

    if path_lower.contains("/storage/volumes") && method == "POST" {
        return Some(AuditEventType::VolumeCreated);
    }

    if path_lower.contains("/storage/volumes") && method == "DELETE" {
        return Some(AuditEventType::VolumeDeleted);
    }

    if path_lower.contains("/snapshots") && method == "POST" {
        return Some(AuditEventType::SnapshotCreated);
    }

    if path_lower.contains("/snapshots") && method == "DELETE" {
        return Some(AuditEventType::SnapshotDeleted);
    }

    // Backup operations
    if path_lower.contains("/backups") {
        if method == "POST" {
            return Some(AuditEventType::BackupCreated);
        } else if method == "DELETE" {
            return Some(AuditEventType::BackupDeleted);
        }
    }

    if path_lower.contains("/backups/") && path_lower.contains("/restore") {
        return Some(AuditEventType::BackupRestored);
    }

    // Cluster operations
    if path_lower.contains("/cluster/nodes") {
        if method == "POST" {
            return Some(AuditEventType::NodeAdded);
        } else if method == "DELETE" {
            return Some(AuditEventType::NodeRemoved);
        }
    }

    // User/Role management
    if path_lower.contains("/roles") || path_lower.contains("/permissions") {
        if method == "POST" {
            return Some(AuditEventType::RoleAssigned);
        } else if method == "DELETE" {
            return Some(AuditEventType::RoleRevoked);
        }
    }

    // Configuration changes
    if path_lower.contains("/config") && (method == "PUT" || method == "PATCH") {
        return Some(AuditEventType::ConfigChanged);
    }

    // Firewall operations
    if path_lower.contains("/firewall/rules") {
        if method == "POST" {
            return Some(AuditEventType::FirewallRuleAdded);
        } else if method == "DELETE" {
            return Some(AuditEventType::FirewallRuleDeleted);
        }
    }

    None
}

/// Determine severity from HTTP status code
fn determine_severity(status: u16) -> AuditSeverity {
    match status {
        200..=299 => AuditSeverity::Info,
        400..=499 => {
            if status == 401 || status == 403 {
                AuditSeverity::Warning
            } else {
                AuditSeverity::Info
            }
        }
        500..=599 => AuditSeverity::Error,
        _ => AuditSeverity::Info,
    }
}

/// Determine result from HTTP status code
fn determine_result(status: u16) -> AuditResult {
    match status {
        206 => AuditResult::Partial, // Partial Content
        200..=299 => AuditResult::Success,
        _ => AuditResult::Failure,
    }
}

/// Audit middleware that logs HTTP requests
pub async fn audit_middleware<S>(
    State(audit_logger): State<Arc<super::AuditLogger>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_string();
    let username = extract_username(&request);
    let source_ip = extract_source_ip(&request);

    debug!(
        method = %method,
        path = %path,
        username = ?username,
        source_ip = ?source_ip,
        "Processing request"
    );

    // Call the next middleware/handler
    let response = next.run(request).await;

    // Determine if this is worth auditing
    if let Some(event_type) = determine_event_type(&method, &path) {
        let status = response.status().as_u16();
        let severity = determine_severity(status);
        let result = determine_result(status);

        let event = AuditEvent {
            timestamp: chrono::Utc::now(),
            event_type,
            severity,
            user: username.clone(),
            source_ip: source_ip.clone(),
            resource: None, // Could extract resource ID from path
            action: format!("{} {}", method, path),
            result,
            details: Some(format!("HTTP {} {}", status, method)),
            session_id: None,
        };

        // Log the audit event (asynchronous, won't fail the request)
        audit_logger.log(event).await;

        info!(
            method = %method,
            path = %path,
            status = status,
            username = ?username,
            "Request audited"
        );
    }

    response
}

/// Helper function to log RBAC permission checks
pub async fn log_permission_check(
    audit_logger: &super::AuditLogger,
    username: &str,
    permission: &str,
    resource: Option<String>,
    granted: bool,
    source_ip: Option<String>,
) {
    let event = AuditEvent {
        timestamp: chrono::Utc::now(),
        event_type: if granted {
            AuditEventType::PermissionGranted
        } else {
            AuditEventType::PermissionDenied
        },
        severity: if granted {
            AuditSeverity::Info
        } else {
            AuditSeverity::Warning
        },
        user: Some(username.to_string()),
        source_ip,
        resource,
        action: format!("check_permission:{}", permission),
        result: if granted {
            AuditResult::Success
        } else {
            AuditResult::Failure
        },
        details: Some(format!(
            "Permission '{}' {} for user '{}'",
            permission,
            if granted { "granted" } else { "denied" },
            username
        )),
        session_id: None,
    };

    audit_logger.log(event).await;
}

/// Helper function to log authentication attempts
pub async fn log_auth_attempt(
    audit_logger: &super::AuditLogger,
    username: &str,
    success: bool,
    source_ip: Option<String>,
    failure_reason: Option<String>,
) {
    let event = AuditEvent {
        timestamp: chrono::Utc::now(),
        event_type: if success {
            AuditEventType::Login
        } else {
            AuditEventType::LoginFailed
        },
        severity: if success {
            AuditSeverity::Info
        } else {
            AuditSeverity::Warning
        },
        user: Some(username.to_string()),
        source_ip,
        resource: Some(format!("user:{}", username)),
        action: if success {
            "login_success".to_string()
        } else {
            "login_failed".to_string()
        },
        result: if success {
            AuditResult::Success
        } else {
            AuditResult::Failure
        },
        details: failure_reason,
        session_id: None,
    };

    audit_logger.log(event).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determine_event_type_auth() {
        assert_eq!(
            determine_event_type("POST", "/api/auth/login"),
            Some(AuditEventType::Login)
        );
        assert_eq!(
            determine_event_type("POST", "/api/auth/logout"),
            Some(AuditEventType::Logout)
        );
        assert_eq!(
            determine_event_type("PUT", "/api/auth/password"),
            Some(AuditEventType::PasswordChanged)
        );
    }

    #[test]
    fn test_determine_event_type_vm() {
        assert_eq!(
            determine_event_type("POST", "/api/vms"),
            Some(AuditEventType::VmCreated)
        );
        assert_eq!(
            determine_event_type("DELETE", "/api/vms/vm-100"),
            Some(AuditEventType::VmDeleted)
        );
        assert_eq!(
            determine_event_type("POST", "/api/vms/vm-100/start"),
            Some(AuditEventType::VmStarted)
        );
        assert_eq!(
            determine_event_type("POST", "/api/vms/vm-100/stop"),
            Some(AuditEventType::VmStopped)
        );
    }

    #[test]
    fn test_determine_event_type_storage() {
        assert_eq!(
            determine_event_type("POST", "/api/storage/pools"),
            Some(AuditEventType::StoragePoolCreated)
        );
        assert_eq!(
            determine_event_type("DELETE", "/api/storage/pools/pool1"),
            Some(AuditEventType::StoragePoolDeleted)
        );
        assert_eq!(
            determine_event_type("POST", "/api/snapshots"),
            Some(AuditEventType::SnapshotCreated)
        );
    }

    #[test]
    fn test_determine_severity() {
        assert_eq!(determine_severity(200), AuditSeverity::Info);
        assert_eq!(determine_severity(401), AuditSeverity::Warning);
        assert_eq!(determine_severity(403), AuditSeverity::Warning);
        assert_eq!(determine_severity(404), AuditSeverity::Info);
        assert_eq!(determine_severity(500), AuditSeverity::Error);
    }

    #[test]
    fn test_determine_result() {
        assert_eq!(determine_result(200), AuditResult::Success);
        assert_eq!(determine_result(201), AuditResult::Success);
        assert_eq!(determine_result(206), AuditResult::Partial);
        assert_eq!(determine_result(400), AuditResult::Failure);
        assert_eq!(determine_result(500), AuditResult::Failure);
    }
}
