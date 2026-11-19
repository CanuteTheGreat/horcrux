//! The browser client of the Horcrux Console, served by the node at
//! `GET /api/console/:vm_id/hxc` (same origin as the API, so the session
//! cookie authenticates the signalling WebSocket and the ticket request).
//! Plain HTML + JavaScript using the browser's own WebRTC stack: no
//! third-party viewer or library.

const CLIENT: &str = include_str!("client.html");

/// The client page for one VM.
pub fn html(vm_id: &str) -> String {
    // JSON string literal: safe inside the <script> (no "</" sequences).
    let id = serde_json::to_string(vm_id)
        .unwrap_or_else(|_| "\"\"".into())
        .replace("</", "<\\/");
    CLIENT.replace("__VM_ID__", &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_id_is_escaped() {
        let h = html("vm-1");
        assert!(h.contains("const VM = \"vm-1\";"));
        let evil = html("</script><script>alert(1)</script>");
        assert!(!evil.contains("</script><script>alert(1)"));
        assert!(evil.contains("<\\/script>"));
    }

    #[test]
    fn page_uses_the_design_channels() {
        let h = html("x");
        for c in ["\"ctl\"", "\"keys\"", "\"input\"", "\"cursor\""] {
            assert!(h.contains(c), "{c}");
        }
        assert!(h.contains("maxRetransmits: 0"));
        assert!(h.contains("console.offer"));
    }
}
