///! Input validation and sanitization module
///! Provides comprehensive validation for all user inputs

use horcrux_common::Error;
use regex::Regex;
use std::sync::LazyLock;

/// Maximum allowed lengths for various fields
pub const MAX_NAME_LENGTH: usize = 255;
pub const MAX_DESCRIPTION_LENGTH: usize = 1000;
pub const MAX_PATH_LENGTH: usize = 4096;
pub const MAX_EMAIL_LENGTH: usize = 320;
pub const MAX_USERNAME_LENGTH: usize = 64;
pub const MIN_USERNAME_LENGTH: usize = 3;
pub const MIN_PASSWORD_LENGTH: usize = 8;
pub const MAX_PASSWORD_LENGTH: usize = 128;

/// Regex patterns for validation
static VM_NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap()
});

static USERNAME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap()
});

static EMAIL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}$").unwrap()
});

static SNAPSHOT_NAME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap()
});

static IP_ADDR_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$").unwrap()
});

static HOSTNAME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(\.[a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*$").unwrap()
});

static MAC_ADDRESS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9A-Fa-f]{2}[:-]){5}([0-9A-Fa-f]{2})$").unwrap()
});

/// Validation result type
pub type ValidationResult<T> = Result<T, Error>;

/// VM name validation
pub fn validate_vm_name(name: &str) -> ValidationResult<()> {
    if name.is_empty() {
        return Err(Error::Validation("VM name cannot be empty".to_string()));
    }

    if name.len() > MAX_NAME_LENGTH {
        return Err(Error::Validation(
            format!("VM name too long (max {} characters)", MAX_NAME_LENGTH)
        ));
    }

    if !VM_NAME_REGEX.is_match(name) {
        return Err(Error::Validation(
            "VM name can only contain alphanumeric characters, hyphens, and underscores".to_string()
        ));
    }

    // Prevent names that could be confused with system paths
    if name.starts_with('/') || name.starts_with('.') || name.contains("..") {
        return Err(Error::Validation(
            "VM name cannot start with '/' or '.' or contain '..'".to_string()
        ));
    }

    Ok(())
}

/// VM ID validation
pub fn validate_vm_id(id: &str) -> ValidationResult<()> {
    if id.is_empty() {
        return Err(Error::Validation("VM ID cannot be empty".to_string()));
    }

    // VM IDs are typically numeric or alphanumeric
    if !id.chars().all(|c| c.is_alphanumeric() || c == '-') {
        return Err(Error::Validation(
            "VM ID can only contain alphanumeric characters and hyphens".to_string()
        ));
    }

    Ok(())
}

/// Memory validation (in MB)
pub fn validate_memory(memory: u64) -> ValidationResult<()> {
    const MIN_MEMORY_MB: u64 = 128;  // 128 MB minimum
    const MAX_MEMORY_MB: u64 = 1048576;  // 1 TB maximum

    if memory < MIN_MEMORY_MB {
        return Err(Error::Validation(
            format!("Memory too low (minimum {} MB)", MIN_MEMORY_MB)
        ));
    }

    if memory > MAX_MEMORY_MB {
        return Err(Error::Validation(
            format!("Memory too high (maximum {} MB)", MAX_MEMORY_MB)
        ));
    }

    // Memory should be a power of 2 or at least aligned to 128MB
    if memory % 128 != 0 {
        return Err(Error::Validation(
            "Memory should be aligned to 128 MB".to_string()
        ));
    }

    Ok(())
}

/// CPU count validation
pub fn validate_cpus(cpus: u32) -> ValidationResult<()> {
    const MIN_CPUS: u32 = 1;
    const MAX_CPUS: u32 = 256;

    if cpus < MIN_CPUS {
        return Err(Error::Validation(
            format!("CPU count too low (minimum {})", MIN_CPUS)
        ));
    }

    if cpus > MAX_CPUS {
        return Err(Error::Validation(
            format!("CPU count too high (maximum {})", MAX_CPUS)
        ));
    }

    Ok(())
}

/// Disk size validation (in bytes)
pub fn validate_disk_size(size: u64) -> ValidationResult<()> {
    const MIN_DISK_SIZE: u64 = 1_073_741_824;  // 1 GB minimum
    const MAX_DISK_SIZE: u64 = 10_995_116_277_760;  // 10 TB maximum

    if size < MIN_DISK_SIZE {
        return Err(Error::Validation(
            format!("Disk size too small (minimum {} GB)", MIN_DISK_SIZE / 1_073_741_824)
        ));
    }

    if size > MAX_DISK_SIZE {
        return Err(Error::Validation(
            format!("Disk size too large (maximum {} TB)", MAX_DISK_SIZE / 1_099_511_627_776)
        ));
    }

    Ok(())
}

/// Username validation
pub fn validate_username(username: &str) -> ValidationResult<()> {
    if username.len() < MIN_USERNAME_LENGTH {
        return Err(Error::Validation(
            format!("Username too short (minimum {} characters)", MIN_USERNAME_LENGTH)
        ));
    }

    if username.len() > MAX_USERNAME_LENGTH {
        return Err(Error::Validation(
            format!("Username too long (maximum {} characters)", MAX_USERNAME_LENGTH)
        ));
    }

    if !USERNAME_REGEX.is_match(username) {
        return Err(Error::Validation(
            "Username can only contain alphanumeric characters, hyphens, and underscores".to_string()
        ));
    }

    // Reserved usernames
    const RESERVED_USERNAMES: &[&str] = &[
        "root", "admin", "administrator", "system", "daemon",
        "nobody", "guest", "test", "default"
    ];

    if RESERVED_USERNAMES.contains(&username.to_lowercase().as_str()) {
        return Err(Error::Validation(
            "This username is reserved".to_string()
        ));
    }

    Ok(())
}

/// Password validation
pub fn validate_password(password: &str) -> ValidationResult<()> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err(Error::Validation(
            format!("Password too short (minimum {} characters)", MIN_PASSWORD_LENGTH)
        ));
    }

    if password.len() > MAX_PASSWORD_LENGTH {
        return Err(Error::Validation(
            format!("Password too long (maximum {} characters)", MAX_PASSWORD_LENGTH)
        ));
    }

    // Check for at least one uppercase, one lowercase, one digit
    let has_uppercase = password.chars().any(|c| c.is_uppercase());
    let has_lowercase = password.chars().any(|c| c.is_lowercase());
    let has_digit = password.chars().any(|c| c.is_numeric());

    if !has_uppercase || !has_lowercase || !has_digit {
        return Err(Error::Validation(
            "Password must contain at least one uppercase letter, one lowercase letter, and one digit".to_string()
        ));
    }

    // Check for common weak passwords
    const WEAK_PASSWORDS: &[&str] = &[
        "Password1", "Password123", "Admin123", "Welcome1",
        "Qwerty123", "Abc12345", "12345678"
    ];

    if WEAK_PASSWORDS.contains(&password) {
        return Err(Error::Validation(
            "This password is too common and insecure".to_string()
        ));
    }

    Ok(())
}

/// Email validation
pub fn validate_email(email: &str) -> ValidationResult<()> {
    if email.is_empty() {
        return Err(Error::Validation("Email cannot be empty".to_string()));
    }

    if email.len() > MAX_EMAIL_LENGTH {
        return Err(Error::Validation(
            format!("Email too long (maximum {} characters)", MAX_EMAIL_LENGTH)
        ));
    }

    if !EMAIL_REGEX.is_match(email) {
        return Err(Error::Validation(
            "Invalid email format".to_string()
        ));
    }

    Ok(())
}

/// Snapshot name validation
pub fn validate_snapshot_name(name: &str) -> ValidationResult<()> {
    if name.is_empty() {
        return Err(Error::Validation("Snapshot name cannot be empty".to_string()));
    }

    if name.len() > MAX_NAME_LENGTH {
        return Err(Error::Validation(
            format!("Snapshot name too long (max {} characters)", MAX_NAME_LENGTH)
        ));
    }

    if !SNAPSHOT_NAME_REGEX.is_match(name) {
        return Err(Error::Validation(
            "Snapshot name can only contain alphanumeric characters, hyphens, and underscores".to_string()
        ));
    }

    Ok(())
}

/// Description validation
pub fn validate_description(description: &str) -> ValidationResult<()> {
    if description.len() > MAX_DESCRIPTION_LENGTH {
        return Err(Error::Validation(
            format!("Description too long (max {} characters)", MAX_DESCRIPTION_LENGTH)
        ));
    }

    // Check for potentially malicious content
    if description.contains("<script") || description.contains("javascript:") {
        return Err(Error::Validation(
            "Description contains potentially malicious content".to_string()
        ));
    }

    Ok(())
}

/// Path validation (prevents path traversal attacks)
pub fn validate_path(path: &str) -> ValidationResult<()> {
    if path.is_empty() {
        return Err(Error::Validation("Path cannot be empty".to_string()));
    }

    if path.len() > MAX_PATH_LENGTH {
        return Err(Error::Validation(
            format!("Path too long (max {} characters)", MAX_PATH_LENGTH)
        ));
    }

    // Prevent path traversal attacks
    if path.contains("..") {
        return Err(Error::Validation(
            "Path cannot contain '..'".to_string()
        ));
    }

    // Prevent null bytes
    if path.contains('\0') {
        return Err(Error::Validation(
            "Path cannot contain null bytes".to_string()
        ));
    }

    Ok(())
}

/// IP address validation
pub fn validate_ip_address(ip: &str) -> ValidationResult<()> {
    if !IP_ADDR_REGEX.is_match(ip) {
        return Err(Error::Validation(
            "Invalid IP address format".to_string()
        ));
    }

    Ok(())
}

/// Hostname validation
pub fn validate_hostname(hostname: &str) -> ValidationResult<()> {
    if hostname.is_empty() {
        return Err(Error::Validation("Hostname cannot be empty".to_string()));
    }

    if hostname.len() > 253 {
        return Err(Error::Validation(
            "Hostname too long (max 253 characters)".to_string()
        ));
    }

    if !HOSTNAME_REGEX.is_match(hostname) {
        return Err(Error::Validation(
            "Invalid hostname format".to_string()
        ));
    }

    Ok(())
}

/// MAC address validation
pub fn validate_mac_address(mac: &str) -> ValidationResult<()> {
    if !MAC_ADDRESS_REGEX.is_match(mac) {
        return Err(Error::Validation(
            "Invalid MAC address format (expected: XX:XX:XX:XX:XX:XX or XX-XX-XX-XX-XX-XX)".to_string()
        ));
    }

    Ok(())
}

/// Port number validation
pub fn validate_port(port: u16) -> ValidationResult<()> {
    const MIN_PORT: u16 = 1;
    const MAX_PORT: u16 = 65535;
    const PRIVILEGED_PORT: u16 = 1024;

    if port < MIN_PORT || port > MAX_PORT {
        return Err(Error::Validation(
            format!("Port must be between {} and {}", MIN_PORT, MAX_PORT)
        ));
    }

    // Warn about privileged ports (informational only)
    if port < PRIVILEGED_PORT {
        tracing::warn!("Using privileged port {}, requires root privileges", port);
    }

    Ok(())
}

/// CIDR notation validation
pub fn validate_cidr(cidr: &str) -> ValidationResult<()> {
    let parts: Vec<&str> = cidr.split('/').collect();

    if parts.len() != 2 {
        return Err(Error::Validation(
            "Invalid CIDR notation (expected: IP/PREFIX)".to_string()
        ));
    }

    validate_ip_address(parts[0])?;

    let prefix: u8 = parts[1].parse().map_err(|_| {
        Error::Validation("Invalid CIDR prefix".to_string())
    })?;

    if prefix > 32 {
        return Err(Error::Validation(
            "CIDR prefix must be between 0 and 32".to_string()
        ));
    }

    Ok(())
}

/// URL validation
pub fn validate_url(url: &str) -> ValidationResult<()> {
    if url.is_empty() {
        return Err(Error::Validation("URL cannot be empty".to_string()));
    }

    // Basic URL validation
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(Error::Validation(
            "URL must start with http:// or https://".to_string()
        ));
    }

    // Use url crate for proper validation
    url::Url::parse(url).map_err(|e| {
        Error::Validation(format!("Invalid URL: {}", e))
    })?;

    Ok(())
}

/// Sanitizes a string by removing potentially dangerous characters
pub fn sanitize_string(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '_' || *c == '.')
        .collect()
}

/// Sanitizes HTML by escaping special characters
pub fn sanitize_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Validates VM configuration
pub fn validate_vm_config(
    name: &str,
    memory: u64,
    cpus: u32,
    disk_size: u64,
) -> ValidationResult<()> {
    validate_vm_name(name)?;
    validate_memory(memory)?;
    validate_cpus(cpus)?;
    validate_disk_size(disk_size)?;
    Ok(())
}

/// Validates user registration data
pub fn validate_user_registration(
    username: &str,
    password: &str,
    email: Option<&str>,
) -> ValidationResult<()> {
    validate_username(username)?;
    validate_password(password)?;

    if let Some(email_addr) = email {
        validate_email(email_addr)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_vm_name() {
        assert!(validate_vm_name("web-server-01").is_ok());
        assert!(validate_vm_name("db_prod").is_ok());
        assert!(validate_vm_name("").is_err());
        assert!(validate_vm_name("/etc/passwd").is_err());
        assert!(validate_vm_name("../etc/passwd").is_err());
        assert!(validate_vm_name("vm with spaces").is_err());
    }

    #[test]
    fn test_validate_memory() {
        assert!(validate_memory(2048).is_ok());  // 2GB
        assert!(validate_memory(4096).is_ok());  // 4GB
        assert!(validate_memory(100).is_err());  // Too low
        assert!(validate_memory(2000000).is_err());  // Too high
        assert!(validate_memory(1000).is_err());  // Not aligned to 128MB
    }

    #[test]
    fn test_validate_cpus() {
        assert!(validate_cpus(1).is_ok());
        assert!(validate_cpus(4).is_ok());
        assert!(validate_cpus(0).is_err());
        assert!(validate_cpus(300).is_err());
    }

    #[test]
    fn test_validate_username() {
        assert!(validate_username("john_doe").is_ok());
        assert!(validate_username("user123").is_ok());
        assert!(validate_username("ab").is_err());  // Too short
        assert!(validate_username("root").is_err());  // Reserved
        assert!(validate_username("user@domain").is_err());  // Invalid chars
    }

    #[test]
    fn test_validate_password() {
        assert!(validate_password("SecurePass123").is_ok());
        assert!(validate_password("MyP@ssw0rd").is_ok());
        assert!(validate_password("short").is_err());  // Too short
        assert!(validate_password("alllowercase123").is_err());  // No uppercase
        assert!(validate_password("ALLUPPERCASE123").is_err());  // No lowercase
        assert!(validate_password("NoDigitsHere").is_err());  // No digits
        assert!(validate_password("Password123").is_err());  // Common password
    }

    #[test]
    fn test_validate_email() {
        assert!(validate_email("user@example.com").is_ok());
        assert!(validate_email("john.doe@company.co.uk").is_ok());
        assert!(validate_email("invalid-email").is_err());
        assert!(validate_email("@example.com").is_err());
        assert!(validate_email("user@").is_err());
    }

    #[test]
    fn test_validate_ip_address() {
        assert!(validate_ip_address("192.168.1.1").is_ok());
        assert!(validate_ip_address("10.0.0.1").is_ok());
        assert!(validate_ip_address("999.999.999.999").is_err());
        assert!(validate_ip_address("192.168.1").is_err());
        assert!(validate_ip_address("not-an-ip").is_err());
    }

    #[test]
    fn test_validate_hostname() {
        assert!(validate_hostname("example.com").is_ok());
        assert!(validate_hostname("sub.domain.example.com").is_ok());
        assert!(validate_hostname("localhost").is_ok());
        assert!(validate_hostname("-invalid").is_err());
        assert!(validate_hostname("invalid-.com").is_err());
    }

    #[test]
    fn test_validate_mac_address() {
        assert!(validate_mac_address("00:11:22:33:44:55").is_ok());
        assert!(validate_mac_address("00-11-22-33-44-55").is_ok());
        assert!(validate_mac_address("AA:BB:CC:DD:EE:FF").is_ok());
        assert!(validate_mac_address("00:11:22:33:44").is_err());  // Too short
        assert!(validate_mac_address("invalid-mac").is_err());
    }

    #[test]
    fn test_validate_cidr() {
        assert!(validate_cidr("192.168.1.0/24").is_ok());
        assert!(validate_cidr("10.0.0.0/8").is_ok());
        assert!(validate_cidr("192.168.1.1/33").is_err());  // Invalid prefix
        assert!(validate_cidr("192.168.1.1").is_err());  // Missing prefix
        assert!(validate_cidr("invalid/24").is_err());  // Invalid IP
    }

    #[test]
    fn test_validate_url() {
        assert!(validate_url("http://example.com").is_ok());
        assert!(validate_url("https://api.example.com/v1").is_ok());
        assert!(validate_url("ftp://example.com").is_err());  // Not http/https
        assert!(validate_url("not-a-url").is_err());
    }

    #[test]
    fn test_sanitize_string() {
        assert_eq!(sanitize_string("hello world"), "hello world");
        assert_eq!(sanitize_string("test<script>alert()</script>"), "testscriptalertscript");
        assert_eq!(sanitize_string("file-name_v1.0"), "file-name_v1.0");
    }

    #[test]
    fn test_sanitize_html() {
        assert_eq!(sanitize_html("<script>"), "&lt;script&gt;");
        assert_eq!(sanitize_html("a & b"), "a &amp; b");
        assert_eq!(sanitize_html("\"quoted\""), "&quot;quoted&quot;");
    }

    #[test]
    fn test_validate_vm_config() {
        assert!(validate_vm_config("web-server", 2048, 2, 21474836480).is_ok());
        assert!(validate_vm_config("", 2048, 2, 21474836480).is_err());  // Empty name
        assert!(validate_vm_config("web-server", 100, 2, 21474836480).is_err());  // Low memory
        assert!(validate_vm_config("web-server", 2048, 0, 21474836480).is_err());  // Zero CPUs
        assert!(validate_vm_config("web-server", 2048, 2, 100).is_err());  // Small disk
    }

    #[test]
    fn test_validate_user_registration() {
        assert!(validate_user_registration(
            "john_doe",
            "SecurePass123",
            Some("john@example.com")
        ).is_ok());

        assert!(validate_user_registration(
            "ab",  // Too short
            "SecurePass123",
            Some("john@example.com")
        ).is_err());

        assert!(validate_user_registration(
            "john_doe",
            "weak",  // Weak password
            Some("john@example.com")
        ).is_err());

        assert!(validate_user_registration(
            "john_doe",
            "SecurePass123",
            Some("invalid-email")  // Invalid email
        ).is_err());
    }

    #[test]
    fn test_path_traversal_prevention() {
        assert!(validate_path("/var/lib/horcrux").is_ok());
        assert!(validate_path("../etc/passwd").is_err());
        assert!(validate_path("/var/../etc/passwd").is_err());
        assert!(validate_path("/var/lib/\0passwd").is_err());  // Null byte
    }
}
