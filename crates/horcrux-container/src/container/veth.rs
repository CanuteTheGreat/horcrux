//! Stable host-side interface names for container NICs (`vc<hash>i<n>`,
//! see `horcrux_common::ct_veth_name`), so a container's firewall rules
//! can match its traffic and nothing else.
//!
//! - LXC: `lxc.net.N.veth.pair` in the generated config (lxc_config.rs).
//! - LXD/Incus: the NIC's `host_name` device option, set before start.
//! - Docker/Podman: the runtime picks a random `vethXXXX`; after the
//!   container starts, its host-side peers are renamed (down, rename, up:
//!   the port stays on its bridge).
use horcrux_common::{ContainerRuntime, Result};
use tokio::process::Command;
use tracing::{debug, warn};

async fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().await.ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// `ip -o link` lines inside a netns: (container ifname, host peer ifindex)
/// for every veth whose peer lives in another namespace (`eth0@if7`).
pub fn parse_peer_links(ip_o_link: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in ip_o_link.lines() {
        // "12: eth0@if13: <BROADCAST,...> mtu 1500 ..."
        let Some(name_part) = line.split(':').nth(1) else {
            continue;
        };
        let name_part = name_part.trim();
        if let Some((name, peer)) = name_part.split_once("@if") {
            if let Ok(idx) = peer.parse::<u32>() {
                out.push((name.to_string(), idx));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn host_ifname_by_index(idx: u32) -> Option<String> {
    for e in std::fs::read_dir("/sys/class/net").ok()?.flatten() {
        let i = std::fs::read_to_string(e.path().join("ifindex")).ok()?;
        if i.trim().parse::<u32>().ok() == Some(idx) {
            return Some(e.file_name().to_string_lossy().to_string());
        }
    }
    None
}

async fn rename_link(from: &str, to: &str) -> Result<()> {
    for args in [
        vec!["link", "set", "dev", from, "down"],
        vec!["link", "set", "dev", from, "name", to],
        vec!["link", "set", "dev", to, "up"],
    ] {
        let out = Command::new("ip")
            .args(&args)
            .output()
            .await
            .map_err(|e| horcrux_common::Error::System(format!("ip: {e}")))?;
        if !out.status.success() {
            return Err(horcrux_common::Error::System(format!(
                "ip {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
    }
    Ok(())
}

/// Rename the host-side veths of a running Docker/Podman container to the
/// container's stable names. Returns the names now in place.
pub async fn name_docker_veths(
    runtime: &ContainerRuntime,
    name: &str,
    id: &str,
) -> Result<Vec<String>> {
    let cli = match runtime {
        ContainerRuntime::Docker => "docker",
        ContainerRuntime::Podman => "podman",
        _ => return Ok(Vec::new()),
    };
    let Some(pid) = output(cli, &["inspect", "-f", "{{.State.Pid}}", name])
        .await
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|p| *p > 0)
    else {
        return Ok(Vec::new());
    };
    let pid_s = pid.to_string();
    let Some(links) = output("nsenter", &["-t", &pid_s, "-n", "ip", "-o", "link"]).await else {
        warn!("container {id}: could not read its interfaces (nsenter)");
        return Ok(Vec::new());
    };
    let mut named = Vec::new();
    for (n, (ifname, peer)) in parse_peer_links(&links).into_iter().enumerate() {
        let want = horcrux_common::ct_veth_name(id, n);
        match host_ifname_by_index(peer) {
            Some(cur) if cur == want => named.push(want),
            Some(cur) => {
                rename_link(&cur, &want).await?;
                debug!("container {id}: {ifname} host side {cur} -> {want}");
                named.push(want);
            }
            None => {}
        }
    }
    Ok(named)
}

/// LXD/Incus: set the NIC's host-side name before the container starts
/// (`host_name` on its eth0, overriding the profile's device if needed).
pub async fn set_instance_host_name(cli: &str, name: &str, id: &str) {
    let want = format!("host_name={}", horcrux_common::ct_veth_name(id, 0));
    if output(cli, &["config", "device", "set", name, "eth0", &want])
        .await
        .is_some()
    {
        return;
    }
    if output(cli, &["config", "device", "override", name, "eth0", &want])
        .await
        .is_none()
    {
        debug!("container {id}: no eth0 NIC to name (no network?)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_links_are_parsed_in_interface_order() {
        let out = "1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN\\    link/loopback 00:00:00:00:00:00\n\
                   14: eth1@if15: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500\\    link/ether 02:42:ac:11:00:03\n\
                   12: eth0@if13: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500\\    link/ether 02:42:ac:11:00:02\n";
        assert_eq!(
            parse_peer_links(out),
            vec![("eth0".to_string(), 13), ("eth1".to_string(), 15)]
        );
    }
}
