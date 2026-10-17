//! Helpers shared by the backup pages.
use crate::api;

/// A VM or container that can be backed up.
#[derive(Debug, Clone, PartialEq)]
pub struct Guest {
    pub id: String,
    /// "vm" or "container".
    pub kind: String,
    pub label: String,
}

/// Every VM and container, VMs first.
pub async fn load_guests() -> Vec<Guest> {
    let mut out = Vec::new();
    if let Ok(vms) = api::get_vms().await {
        for v in vms {
            out.push(Guest {
                label: format!("VM {} ({})", v.id, v.name),
                id: v.id,
                kind: "vm".into(),
            });
        }
    }
    if let Ok(cts) = api::get_containers().await {
        for c in cts {
            out.push(Guest {
                label: format!("CT {} ({}, {})", c.id, c.name, c.runtime),
                id: c.id,
                kind: "container".into(),
            });
        }
    }
    out
}

/// `Some(trimmed)` unless blank.
pub fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Ask the user to confirm (true outside a browser, e.g. in tests).
pub fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

/// First 8 characters of an id.
pub fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(non_empty("  "), None);
        assert_eq!(non_empty(" a "), Some("a".to_string()));
        assert_eq!(short("0123456789"), "01234567");
    }
}
