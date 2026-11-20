//! A UDP socket that answers from the address a request reached.
//!
//! A socket bound to the unspecified address (0.0.0.0 / ::) sends every
//! datagram with the source address of the outgoing route. On a node with
//! several NICs, VM bridges, container bridges or VPN interfaces the route
//! back to a client often leaves through a different address than the one
//! the client sent to, and a NAT or stateful firewall in front of the
//! client drops the answer (RFC 8489 section 6.3.2 expects the response to
//! come from the request's destination).
//!
//! On Linux the fix is IP_PKTINFO (IPv4) / IPV6_RECVPKTINFO (IPv6): the
//! kernel reports the destination address of every received datagram as
//! ancillary data, and `sendmsg` takes the source address to use the same
//! way. A dual-stack `[::]` socket gets both (IPv4 clients arrive as
//! IPv4-mapped peers with an IP_PKTINFO message). When the socket is bound
//! to a specific address, or the platform has no support, the plain path
//! is used: the kernel then already picks the right source.

use std::io;
use std::net::{IpAddr, SocketAddr};
use tokio::net::UdpSocket;

/// The local address a datagram was received on (and is answered from).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub ip: IpAddr,
    /// Interface the datagram arrived on; only used for link-local IPv6.
    pub ifindex: u32,
}

pub struct PktSocket {
    sock: UdpSocket,
    pktinfo: bool,
}

/// Whether this platform can report and set the datagram address.
pub const SUPPORTED: bool = cfg!(target_os = "linux");

impl PktSocket {
    /// Bind `addr`. `reply_from_request` asks for destination-address
    /// tracking; it takes effect only when `addr` is unspecified and the
    /// platform supports it (see `answers_from_request`).
    pub async fn bind(addr: SocketAddr, reply_from_request: bool) -> io::Result<PktSocket> {
        let sock = UdpSocket::bind(addr).await?;
        let mut pktinfo = false;
        if reply_from_request && addr.ip().is_unspecified() && SUPPORTED {
            pktinfo = imp::enable(&sock, addr.is_ipv6()).is_ok();
            if !pktinfo {
                tracing::warn!(
                    "relay: could not enable per-datagram destination addresses on {addr}; \
                     replies use the outgoing route's source address"
                );
            }
        }
        Ok(PktSocket { sock, pktinfo })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sock.local_addr()
    }

    /// True when replies are sent from the address the request reached.
    pub fn answers_from_request(&self) -> bool {
        self.pktinfo
    }

    /// Receive one datagram: length, sender and (when tracked) the local
    /// address it reached.
    pub async fn recv_from(
        &self,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr, Option<Local>)> {
        if !self.pktinfo {
            let (n, a) = self.sock.recv_from(buf).await?;
            return Ok((n, a, None));
        }
        loop {
            self.sock.readable().await?;
            match self
                .sock
                .try_io(tokio::io::Interest::READABLE, || imp::recv(&self.sock, buf))
            {
                Ok(r) => return Ok(r),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// Send without waiting; from `local` when given and tracked.
    pub fn try_send_from(
        &self,
        buf: &[u8],
        dst: SocketAddr,
        local: Option<Local>,
    ) -> io::Result<usize> {
        match local {
            Some(l) if self.pktinfo => self.sock.try_io(tokio::io::Interest::WRITABLE, || {
                imp::send(&self.sock, buf, dst, l)
            }),
            _ => self.sock.try_send_to(buf, dst),
        }
    }
}

/// An IPv4-mapped IPv6 address as IPv4 (so IPv4 clients of a dual-stack
/// socket compare equal to IPv4 addresses).
pub fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{unmap, Local};
    use nix::libc;
    use nix::sys::socket::{
        recvmsg, sendmsg, setsockopt, sockopt, ControlMessage, ControlMessageOwned, MsgFlags,
        SockaddrStorage,
    };
    use std::io::{self, IoSlice, IoSliceMut};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::os::fd::AsRawFd;
    use tokio::net::UdpSocket;

    pub fn enable(sock: &UdpSocket, v6: bool) -> io::Result<()> {
        if v6 {
            setsockopt(sock, sockopt::Ipv6RecvPacketInfo, &true).map_err(io::Error::from)?;
            // IPv4 clients of a dual-stack socket: best effort (an
            // IPV6_V6ONLY socket has no IPv4 side).
            let _ = setsockopt(sock, sockopt::Ipv4PacketInfo, &true);
            Ok(())
        } else {
            setsockopt(sock, sockopt::Ipv4PacketInfo, &true).map_err(io::Error::from)
        }
    }

    pub fn recv(
        sock: &UdpSocket,
        buf: &mut [u8],
    ) -> io::Result<(usize, SocketAddr, Option<Local>)> {
        let mut cmsg = nix::cmsg_space!(libc::in_pktinfo, libc::in6_pktinfo);
        let mut iov = [IoSliceMut::new(buf)];
        let msg = recvmsg::<SockaddrStorage>(
            sock.as_raw_fd(),
            &mut iov,
            Some(&mut cmsg),
            MsgFlags::empty(),
        )
        .map_err(io::Error::from)?;
        let from = msg
            .address
            .and_then(|a| {
                if let Some(v4) = a.as_sockaddr_in() {
                    Some(SocketAddr::from(std::net::SocketAddrV4::from(*v4)))
                } else {
                    a.as_sockaddr_in6()
                        .map(|v6| SocketAddr::from(std::net::SocketAddrV6::from(*v6)))
                }
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "datagram without sender"))?;
        let mut local = None;
        for c in msg.cmsgs().map_err(io::Error::from)? {
            match c {
                ControlMessageOwned::Ipv4PacketInfo(pi) => {
                    local = Some(Local {
                        ip: IpAddr::V4(Ipv4Addr::from(u32::from_be(pi.ipi_addr.s_addr))),
                        ifindex: pi.ipi_ifindex as u32,
                    });
                }
                ControlMessageOwned::Ipv6PacketInfo(pi) => {
                    local = Some(Local {
                        ip: unmap(IpAddr::V6(pi.ipi6_addr.s6_addr.into())),
                        ifindex: pi.ipi6_ifindex,
                    });
                }
                _ => {}
            }
        }
        Ok((msg.bytes, from, local))
    }

    pub fn send(sock: &UdpSocket, buf: &[u8], dst: SocketAddr, local: Local) -> io::Result<usize> {
        let iov = [IoSlice::new(buf)];
        let to = SockaddrStorage::from(dst);
        let dst_is_v6 = dst.is_ipv6();
        let sock_is_v6 = sock.local_addr()?.is_ipv6();
        // Link-local IPv6 needs the interface; for everything else leave
        // the choice of interface to the routing table (an answer pinned to
        // the arrival interface would break asymmetric routing).
        let v6_if = match local.ip {
            IpAddr::V6(a) if (a.segments()[0] & 0xffc0) == 0xfe80 => local.ifindex,
            _ => 0,
        };
        let r = match local.ip {
            IpAddr::V4(a) if !dst_is_v6 || sock_is_v6 => {
                let pi = libc::in_pktinfo {
                    ipi_ifindex: 0,
                    ipi_spec_dst: libc::in_addr {
                        s_addr: u32::from(a).to_be(),
                    },
                    ipi_addr: libc::in_addr { s_addr: 0 },
                };
                sendmsg(
                    sock.as_raw_fd(),
                    &iov,
                    &[ControlMessage::Ipv4PacketInfo(&pi)],
                    MsgFlags::empty(),
                    Some(&to),
                )
            }
            IpAddr::V6(a) if dst_is_v6 && sock_is_v6 => {
                let pi = libc::in6_pktinfo {
                    ipi6_addr: libc::in6_addr {
                        s6_addr: a.octets(),
                    },
                    ipi6_ifindex: v6_if,
                };
                sendmsg(
                    sock.as_raw_fd(),
                    &iov,
                    &[ControlMessage::Ipv6PacketInfo(&pi)],
                    MsgFlags::empty(),
                    Some(&to),
                )
            }
            // Address family of the local address does not fit the peer:
            // plain send.
            _ => sendmsg(sock.as_raw_fd(), &iov, &[], MsgFlags::empty(), Some(&to)),
        };
        r.map_err(io::Error::from)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use super::Local;
    use std::io;
    use std::net::SocketAddr;
    use tokio::net::UdpSocket;

    pub fn enable(_: &UdpSocket, _: bool) -> io::Result<()> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
    pub fn recv(_: &UdpSocket, _: &mut [u8]) -> io::Result<(usize, SocketAddr, Option<Local>)> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
    pub fn send(_: &UdpSocket, _: &[u8], _: SocketAddr, _: Local) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The reply source is the one the sender asks for, even when it is
    /// not the address the request reached (proves the cmsg path; the
    /// loopback route alone would answer from the request's destination).
    #[tokio::test]
    async fn send_from_sets_the_source_address() {
        let srv = PktSocket::bind("0.0.0.0:0".parse().unwrap(), true)
            .await
            .unwrap();
        assert!(srv.answers_from_request());
        let port = srv.local_addr().unwrap().port();
        let cli = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        cli.send_to(b"x", ("127.0.0.2", port)).await.unwrap();
        let mut buf = [0u8; 16];
        let (n, from, local) = srv.recv_from(&mut buf).await.unwrap();
        assert_eq!((&buf[..n], from), (&b"x"[..], cli.local_addr().unwrap()));
        let local = local.expect("destination address");
        assert_eq!(local.ip, "127.0.0.2".parse::<IpAddr>().unwrap());
        // Answer from a different local address than the request used.
        let other = Local {
            ip: "127.0.0.3".parse().unwrap(),
            ifindex: 0,
        };
        srv.try_send_from(b"y", from, Some(other)).unwrap();
        let mut b2 = [0u8; 16];
        let (n, src) = tokio::time::timeout(Duration::from_secs(2), cli.recv_from(&mut b2))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&b2[..n], b"y");
        assert_eq!(src.ip(), other.ip);
        assert_eq!(src.port(), port);
    }

    #[tokio::test]
    async fn specific_bind_keeps_the_plain_path() {
        let srv = PktSocket::bind("127.0.0.1:0".parse().unwrap(), true)
            .await
            .unwrap();
        assert!(!srv.answers_from_request());
        let off = PktSocket::bind("0.0.0.0:0".parse().unwrap(), false)
            .await
            .unwrap();
        assert!(!off.answers_from_request());
    }

    /// Dual-stack `[::]`: an IPv4 client reaching 127.0.0.2 and an IPv6
    /// client reaching ::1 are both answered from the address they used.
    #[tokio::test]
    async fn dual_stack_answers_both_families() {
        let Ok(srv) = PktSocket::bind("[::]:0".parse().unwrap(), true).await else {
            eprintln!("SKIP: no IPv6 in this environment");
            return;
        };
        let port = srv.local_addr().unwrap().port();
        let Ok(c6) = UdpSocket::bind("[::1]:0").await else {
            eprintln!("SKIP: no IPv6 loopback");
            return;
        };
        let c4 = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut buf = [0u8; 16];
        for (c, dst) in [(&c4, "127.0.0.2"), (&c6, "::1")] {
            c.send_to(b"q", (dst, port)).await.unwrap();
            let (_, from, local) = srv.recv_from(&mut buf).await.unwrap();
            let local = local.expect("destination address");
            assert_eq!(local.ip, dst.parse::<IpAddr>().unwrap());
            srv.try_send_from(b"a", from, Some(local)).unwrap();
            let mut b2 = [0u8; 16];
            let (_, src) = tokio::time::timeout(Duration::from_secs(2), c.recv_from(&mut b2))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(unmap(src.ip()), dst.parse::<IpAddr>().unwrap());
        }
    }
}
