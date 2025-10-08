///! Predefined security groups (like AWS/Azure security groups)

use super::{Direction, FirewallAction, FirewallRule, Protocol, SecurityGroup};
use uuid::Uuid;

/// Create common predefined security groups
pub fn create_predefined_groups() -> Vec<SecurityGroup> {
    vec![
        create_web_server_group(),
        create_ssh_group(),
        create_database_group(),
        create_allow_all_group(),
    ]
}

/// Web server security group (HTTP/HTTPS)
fn create_web_server_group() -> SecurityGroup {
    SecurityGroup {
        name: "web-server".to_string(),
        description: "Allow HTTP and HTTPS traffic".to_string(),
        rules: vec![
            FirewallRule {
                id: Uuid::new_v4().to_string(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::In,
                protocol: Some(Protocol::Tcp),
                source: None,
                dest: None,
                sport: None,
                dport: Some("80".to_string()),
                comment: Some("HTTP".to_string()),
                log: false,
                position: 0,
            },
            FirewallRule {
                id: Uuid::new_v4().to_string(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::In,
                protocol: Some(Protocol::Tcp),
                source: None,
                dest: None,
                sport: None,
                dport: Some("443".to_string()),
                comment: Some("HTTPS".to_string()),
                log: false,
                position: 1,
            },
        ],
    }
}

/// SSH security group
fn create_ssh_group() -> SecurityGroup {
    SecurityGroup {
        name: "ssh".to_string(),
        description: "Allow SSH access".to_string(),
        rules: vec![FirewallRule {
            id: Uuid::new_v4().to_string(),
            enabled: true,
            action: FirewallAction::Accept,
            direction: Direction::In,
            protocol: Some(Protocol::Tcp),
            source: None,
            dest: None,
            sport: None,
            dport: Some("22".to_string()),
            comment: Some("SSH".to_string()),
            log: false,
            position: 0,
        }],
    }
}

/// Database security group (MySQL/PostgreSQL)
fn create_database_group() -> SecurityGroup {
    SecurityGroup {
        name: "database".to_string(),
        description: "Allow database access (MySQL/PostgreSQL)".to_string(),
        rules: vec![
            FirewallRule {
                id: Uuid::new_v4().to_string(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::In,
                protocol: Some(Protocol::Tcp),
                source: None,
                dest: None,
                sport: None,
                dport: Some("3306".to_string()),
                comment: Some("MySQL".to_string()),
                log: false,
                position: 0,
            },
            FirewallRule {
                id: Uuid::new_v4().to_string(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::In,
                protocol: Some(Protocol::Tcp),
                source: None,
                dest: None,
                sport: None,
                dport: Some("5432".to_string()),
                comment: Some("PostgreSQL".to_string()),
                log: false,
                position: 1,
            },
        ],
    }
}

/// Allow all traffic (development/testing)
fn create_allow_all_group() -> SecurityGroup {
    SecurityGroup {
        name: "allow-all".to_string(),
        description: "Allow all traffic (use with caution)".to_string(),
        rules: vec![
            FirewallRule {
                id: Uuid::new_v4().to_string(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::In,
                protocol: None,
                source: None,
                dest: None,
                sport: None,
                dport: None,
                comment: Some("Allow all incoming".to_string()),
                log: false,
                position: 0,
            },
            FirewallRule {
                id: Uuid::new_v4().to_string(),
                enabled: true,
                action: FirewallAction::Accept,
                direction: Direction::Out,
                protocol: None,
                source: None,
                dest: None,
                sport: None,
                dport: None,
                comment: Some("Allow all outgoing".to_string()),
                log: false,
                position: 1,
            },
        ],
    }
}
