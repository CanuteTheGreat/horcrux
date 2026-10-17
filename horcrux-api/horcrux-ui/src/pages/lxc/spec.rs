//! Proxmox-style one-line specs for LXC NICs and mount points, used by the
//! create form and the container's Configure page:
//!
//! `name=eth0,bridge=vmbr0,tag=20,ip=10.0.0.5/24,gw=10.0.0.1,ip6=auto,hwaddr=02:..,mtu=1500`
//! `mp=/srv/data,source=/tank/data,ro=1` (bind mount) or `mp=/var/lib/app`
//! (managed volume; `volume=mp0` is kept when editing).
use horcrux_common::{LxcMount, LxcNet};

pub fn parse_net(spec: &str) -> Result<LxcNet, String> {
    LxcNet::parse_spec(spec)
}

pub fn format_net(index: usize, n: &LxcNet) -> String {
    n.to_spec(index)
}

pub fn parse_mount(spec: &str) -> Result<LxcMount, String> {
    LxcMount::parse_spec(spec)
}

pub fn format_mount(m: &LxcMount) -> String {
    m.to_spec()
}

fn non_empty_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty())
}

pub fn parse_nets(text: &str) -> Result<Vec<LxcNet>, String> {
    non_empty_lines(text).map(parse_net).collect()
}

pub fn parse_mounts(text: &str) -> Result<Vec<LxcMount>, String> {
    non_empty_lines(text).map(parse_mount).collect()
}

pub fn format_nets(nets: &[LxcNet]) -> String {
    nets.iter()
        .enumerate()
        .map(|(i, n)| format_net(i, n))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn format_mounts(mounts: &[LxcMount]) -> String {
    mounts
        .iter()
        .map(format_mount)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Optional number field ("" = unset).
pub fn opt_num<T: std::str::FromStr>(field: &str, v: &str) -> Result<Option<T>, String> {
    let v = v.trim();
    if v.is_empty() {
        Ok(None)
    } else {
        v.parse()
            .map(Some)
            .map_err(|_| format!("{field} must be a number, got '{v}'"))
    }
}

pub fn opt_text(v: &str) -> Option<String> {
    let v = v.trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

/// Remove ANSI/VT escape sequences (colors, cursor movement) from PTY
/// output so it reads cleanly in the plain-text terminal view.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\u{1b}' => match it.peek() {
                Some('[') => {
                    it.next();
                    // CSI: parameters until a final byte in @..~
                    for d in it.by_ref() {
                        if ('@'..='~').contains(&d) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    it.next();
                    // OSC: until BEL or ESC \
                    while let Some(d) = it.next() {
                        if d == '\u{7}' {
                            break;
                        }
                        if d == '\u{1b}' {
                            it.next();
                            break;
                        }
                    }
                }
                _ => {
                    it.next();
                }
            },
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nic_specs_round_trip() {
        let n = parse_net(
            "name=eth0, bridge=vmbr0,tag=20,ip=10.0.0.5/24,gw=10.0.0.1,ip6=auto,mtu=1450",
        )
        .unwrap();
        assert_eq!(n.bridge, "vmbr0");
        assert_eq!(n.vlan, Some(20));
        assert_eq!(n.ip.as_deref(), Some("10.0.0.5/24"));
        assert_eq!(n.gateway.as_deref(), Some("10.0.0.1"));
        assert_eq!(n.ip6.as_deref(), Some("auto"));
        assert_eq!(n.mtu, Some(1450));
        assert_eq!(parse_net(&format_net(0, &n)).unwrap(), n);
        // Default name comes from the position.
        assert_eq!(
            format_net(1, &parse_net("bridge=br1,ip=dhcp").unwrap()),
            "name=eth1,bridge=br1,ip=dhcp"
        );
        assert!(parse_net("ip=dhcp").is_err());
        assert!(parse_net("bridge=b,tag=x").is_err());
        assert!(parse_net("bridge=b,firewall=1").is_err());
        assert!(parse_net("bridge").is_err());
        let all = parse_nets("bridge=a,ip=dhcp\n\n  bridge=b\n").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            parse_nets(&format_nets(&all)).unwrap()[1].name.as_deref(),
            Some("eth1")
        );
    }

    #[test]
    fn mount_specs_round_trip() {
        let m = parse_mount("mp=/srv/data,source=/tank/data,ro=1").unwrap();
        assert_eq!(m.path, "/srv/data");
        assert_eq!(m.source.as_deref(), Some("/tank/data"));
        assert!(m.read_only);
        assert_eq!(parse_mount(&format_mount(&m)).unwrap(), m);
        let v = parse_mount("mp=/var/lib/app,volume=mp0").unwrap();
        assert_eq!(v.volume.as_deref(), Some("mp0"));
        assert_eq!(format_mount(&v), "mp=/var/lib/app,volume=mp0");
        assert!(parse_mount("source=/x").is_err());
        assert_eq!(parse_mounts("mp=/a\nmp=/b,ro=1").unwrap().len(), 2);
    }

    #[test]
    fn number_fields() {
        assert_eq!(opt_num::<u64>("Disk", " ").unwrap(), None);
        assert_eq!(opt_num::<u64>("Disk", "8").unwrap(), Some(8));
        assert_eq!(opt_num::<f64>("CPU limit", "0.5").unwrap(), Some(0.5));
        assert!(opt_num::<u64>("Disk", "big").is_err());
    }

    #[test]
    fn strips_terminal_escapes() {
        assert_eq!(
            strip_ansi("\u{1b}[1;32mroot@ct\u{1b}[0m:~# ls\r\n\u{1b}]0;title\u{7}ok"),
            "root@ct:~# ls\nok"
        );
    }
}
