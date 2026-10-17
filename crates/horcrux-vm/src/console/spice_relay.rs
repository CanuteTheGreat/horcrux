//! SPICE relay for LXD/Incus VMs.
//!
//! LXD and Incus give their VMs a SPICE display on a local Unix socket with
//! ticketing disabled, reachable only through the instance console API
//! (`type=vga`, one websocket per SPICE channel). Horcrux turns that into a
//! normal password-protected SPICE server, so the same remote-viewer `.vv`
//! file and the same browser WebSocket proxy work as for QEMU VMs:
//!
//! * a TCP listener per VM (HORCRUX_SPICE_LISTEN or 127.0.0.1, ephemeral
//!   port) with a random password;
//! * for every client connection (SPICE opens one per channel) the link
//!   handshake is terminated here: the client gets Horcrux's RSA key, its
//!   ticket is decrypted and checked against the password, and only then
//!   is a `vga` console session opened on the daemon and the link
//!   completed upstream (with the daemon's own key);
//! * after a successful link the two byte streams are spliced.
//!
//! The relay stops after 10 idle minutes without connections, or when the
//! VM is no longer running.
use super::lxd::{Backend, Client, ConsoleKind};
use futures::{SinkExt, StreamExt};
use horcrux_common::{Error, Result};
use rand::Rng;
use rsa::pkcs8::{DecodePublicKey, EncodePublicKey};
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, info, warn};

const IDLE_SECS: i64 = 600;
/// SPICE link errors (spice-protocol `SpiceLinkErr`).
const LINK_ERR_PERMISSION_DENIED: u32 = 7;

#[derive(Clone, Debug)]
pub struct RelayInfo {
    /// Address the relay listens on.
    pub listen: String,
    pub port: u16,
    pub password: String,
}

struct Relay {
    info: RelayInfo,
    last_active: Arc<AtomicI64>,
    active: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

fn relays() -> &'static Mutex<HashMap<String, Relay>> {
    static R: OnceLock<Mutex<HashMap<String, Relay>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// RSA-1024 key whose public half is sent to SPICE clients (the protocol
/// fixes the 162-byte SubjectPublicKeyInfo of a 1024-bit key).
async fn relay_key() -> Result<Arc<(RsaPrivateKey, Vec<u8>)>> {
    static K: OnceLock<Arc<(RsaPrivateKey, Vec<u8>)>> = OnceLock::new();
    if let Some(k) = K.get() {
        return Ok(k.clone());
    }
    let generated = tokio::task::spawn_blocking(|| {
        let key = RsaPrivateKey::new(&mut rand::thread_rng(), 1024)
            .map_err(|e| Error::System(format!("SPICE relay key: {e}")))?;
        let der = RsaPublicKey::from(&key)
            .to_public_key_der()
            .map_err(|e| Error::System(format!("SPICE relay key: {e}")))?
            .as_bytes()
            .to_vec();
        if der.len() != 162 {
            return Err(Error::System(format!(
                "SPICE relay key: unexpected public key size {}",
                der.len()
            )));
        }
        Ok(Arc::new((key, der)))
    })
    .await
    .map_err(|e| Error::System(format!("SPICE relay key: {e}")))??;
    // A concurrent first caller may have won; either key is fine.
    Ok(K.get_or_init(|| generated).clone())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn new_password() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARS[rng.gen_range(0..CHARS.len())] as char)
        .collect()
}

/// The running relay for `instance`, starting one if needed.
pub async fn ensure(backend: Backend, instance: &str) -> Result<RelayInfo> {
    let key = format!("{backend:?}/{instance}");
    let mut map = relays().lock().await;
    if let Some(r) = map.get(&key) {
        if !r.task.is_finished() {
            r.last_active.store(now(), Ordering::Relaxed);
            return Ok(r.info.clone());
        }
        map.remove(&key);
    }
    let rsa = relay_key().await?;
    let listen = std::env::var("HORCRUX_SPICE_LISTEN")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let bind = if listen.contains(':') && !listen.starts_with('[') {
        format!("[{listen}]:0")
    } else {
        format!("{listen}:0")
    };
    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|e| Error::System(format!("SPICE relay: cannot listen on {bind}: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| Error::System(e.to_string()))?
        .port();
    let info = RelayInfo {
        listen: listen.clone(),
        port,
        password: new_password(),
    };
    let last_active = Arc::new(AtomicI64::new(now()));
    let active = Arc::new(AtomicUsize::new(0));
    let task = tokio::spawn(run(
        listener,
        backend,
        instance.to_string(),
        info.password.clone(),
        rsa,
        last_active.clone(),
        active.clone(),
    ));
    info!(
        "SPICE relay for {} VM {} on {}:{}",
        backend.label(),
        instance,
        listen,
        port
    );
    map.insert(
        key,
        Relay {
            info: info.clone(),
            last_active,
            active,
            task,
        },
    );
    Ok(info)
}

async fn run(
    listener: TcpListener,
    backend: Backend,
    instance: String,
    password: String,
    rsa: Arc<(RsaPrivateKey, Vec<u8>)>,
    last_active: Arc<AtomicI64>,
    active: Arc<AtomicUsize>,
) {
    loop {
        let accepted =
            tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept()).await;
        match accepted {
            Ok(Ok((sock, peer))) => {
                last_active.store(now(), Ordering::Relaxed);
                active.fetch_add(1, Ordering::Relaxed);
                let (backend, instance, password, rsa, last_active, active) = (
                    backend,
                    instance.clone(),
                    password.clone(),
                    rsa.clone(),
                    last_active.clone(),
                    active.clone(),
                );
                tokio::spawn(async move {
                    if let Err(e) = handle(sock, backend, &instance, &password, &rsa).await {
                        debug!("SPICE relay {instance} ({peer}): {e}");
                    }
                    last_active.store(now(), Ordering::Relaxed);
                    active.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Ok(Err(e)) => warn!("SPICE relay {instance}: accept failed: {e}"),
            Err(_) => {}
        }
        let idle = active.load(Ordering::Relaxed) == 0
            && now() - last_active.load(Ordering::Relaxed) > IDLE_SECS;
        if idle {
            info!("SPICE relay for {instance} idle, closing");
            return;
        }
        if active.load(Ordering::Relaxed) == 0 {
            match Client::new(backend).instance_state(&instance).await {
                Ok(s) if s.is_running() => {}
                _ => {
                    info!("SPICE relay for {instance}: VM gone or stopped, closing");
                    return;
                }
            }
        }
    }
}

async fn read_n(s: &mut TcpStream, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

/// Byte-stream view of the daemon's vga websocket.
struct Upstream {
    ws: super::lxd::Ws,
    buf: Vec<u8>,
}

impl Upstream {
    async fn read_n(&mut self, n: usize) -> Result<Vec<u8>> {
        while self.buf.len() < n {
            match self.ws.next().await {
                Some(Ok(WsMessage::Binary(d))) => self.buf.extend_from_slice(&d),
                Some(Ok(WsMessage::Text(t))) => self.buf.extend_from_slice(t.as_bytes()),
                Some(Ok(WsMessage::Close(_))) | None => {
                    return Err(Error::System("SPICE display closed by the daemon".into()))
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(Error::System(format!("SPICE display: {e}"))),
            }
        }
        Ok(self.buf.drain(..n).collect())
    }
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.ws
            .send(WsMessage::Binary(data.to_vec()))
            .await
            .map_err(|e| Error::System(format!("SPICE display: {e}")))
    }
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    b.get(off..off + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}

/// First common capability word of a link message/reply (bit 0 =
/// PROTOCOL_AUTH_SELECTION).
fn first_common_cap(body: &[u8], num_caps_off: usize, caps_off_off: usize) -> u32 {
    if u32_at(body, num_caps_off) == 0 {
        return 0;
    }
    u32_at(body, u32_at(body, caps_off_off) as usize)
}

async fn handle(
    mut client: TcpStream,
    backend: Backend,
    instance: &str,
    password: &str,
    rsa: &(RsaPrivateKey, Vec<u8>),
) -> Result<()> {
    let io = |e: std::io::Error| Error::System(e.to_string());
    let _ = client.set_nodelay(true);
    // SpiceLinkHeader: magic, major, minor, size; then SpiceLinkMess.
    let hdr = read_n(&mut client, 16).await.map_err(io)?;
    if &hdr[0..4] != b"REDQ" {
        return Err(Error::System("not a SPICE client".into()));
    }
    let size = u32_at(&hdr, 12) as usize;
    if size > 4096 {
        return Err(Error::System("SPICE link message too large".into()));
    }
    let mess = read_n(&mut client, size).await.map_err(io)?;
    // SpiceLinkMess: connection_id u32, channel_type u8, channel_id u8,
    // num_common_caps u32 @6, num_channel_caps u32 @10, caps_offset u32 @14
    let client_caps = first_common_cap(&mess, 6, 14);

    let session = Client::new(backend)
        .open_console(instance, ConsoleKind::Vga)
        .await?;
    let mut control = session.control;
    let mut up = Upstream {
        ws: session.data,
        buf: Vec::new(),
    };
    up.write(&[hdr.as_slice(), mess.as_slice()].concat())
        .await?;
    let rhdr = up.read_n(16).await?;
    let rsize = u32_at(&rhdr, 12) as usize;
    let mut reply = up.read_n(rsize).await?;
    // SpiceLinkReply: error u32, pub_key [162], num_common_caps u32 @166,
    // num_channel_caps u32 @170, caps_offset u32 @174
    if &rhdr[0..4] != b"REDQ" || reply.len() < 178 || u32_at(&reply, 0) != 0 {
        client.write_all(&rhdr).await.map_err(io)?;
        client.write_all(&reply).await.map_err(io)?;
        return Err(Error::System("SPICE link refused by the VM".into()));
    }
    let server_caps = first_common_cap(&reply, 166, 174);
    let upstream_key = RsaPublicKey::from_public_key_der(&reply[4..166])
        .map_err(|e| Error::System(format!("SPICE display key: {e}")))?;
    reply[4..166].copy_from_slice(&rsa.1);
    client.write_all(&rhdr).await.map_err(io)?;
    client.write_all(&reply).await.map_err(io)?;

    let auth_selection = client_caps & 1 != 0 && server_caps & 1 != 0;
    if auth_selection {
        let mech = read_n(&mut client, 4).await.map_err(io)?;
        if u32_at(&mech, 0) != 1 {
            // Only SPICE ticket auth is offered (no SASL).
            client
                .write_all(&LINK_ERR_PERMISSION_DENIED.to_le_bytes())
                .await
                .map_err(io)?;
            return Err(Error::System("unsupported SPICE auth mechanism".into()));
        }
        up.write(&mech).await?;
    }
    let ticket = read_n(&mut client, 128).await.map_err(io)?;
    let ok = rsa
        .0
        .decrypt(Oaep::new::<sha1::Sha1>(), &ticket)
        .ok()
        .map(|pt| {
            let pt = pt.split(|b| *b == 0).next().unwrap_or(&[]).to_vec();
            use subtle::ConstantTimeEq;
            pt.len() == password.len() && bool::from(pt.ct_eq(password.as_bytes()))
        })
        .unwrap_or(false);
    if !ok {
        client
            .write_all(&LINK_ERR_PERMISSION_DENIED.to_le_bytes())
            .await
            .map_err(io)?;
        let _ = control.close(None).await;
        return Err(Error::System("wrong SPICE password".into()));
    }
    // The daemon's SPICE has ticketing disabled; complete its link with an
    // empty ticket under its own key.
    let enc = upstream_key
        .encrypt(&mut rand::thread_rng(), Oaep::new::<sha1::Sha1>(), b"\0")
        .map_err(|e| Error::System(format!("SPICE link: {e}")))?;
    up.write(&enc).await?;
    let result = up.read_n(4).await?;
    client.write_all(&result).await.map_err(io)?;
    if u32_at(&result, 0) != 0 {
        return Err(Error::System(format!(
            "SPICE link failed upstream ({})",
            u32_at(&result, 0)
        )));
    }

    // Splice.
    let leftover = std::mem::take(&mut up.buf);
    if !leftover.is_empty() {
        client.write_all(&leftover).await.map_err(io)?;
    }
    let (mut c_rx, mut c_tx) = client.into_split();
    let (mut u_tx, mut u_rx) = up.ws.split();
    let to_vm = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match c_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if u_tx
                        .send(WsMessage::Binary(buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    };
    let to_client = async {
        while let Some(m) = u_rx.next().await {
            let data = match m {
                Ok(WsMessage::Binary(d)) => d,
                Ok(WsMessage::Text(t)) => t.into_bytes(),
                Ok(WsMessage::Close(_)) | Err(_) => break,
                Ok(_) => continue,
            };
            if c_tx.write_all(&data).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = to_vm => {}
        _ = to_client => {}
    }
    let _ = control.close(None).await;
    Ok(())
}

/// `.vv` connection file for remote-viewer.
pub fn virt_viewer_file(title: &str, host: &str, info: &RelayInfo) -> String {
    format!(
        "[virt-viewer]\ntype=spice\nhost={host}\nport={}\npassword={}\ntitle={title} (Horcrux)\n\
         delete-this-file=1\nfullscreen=0\nenable-usbredir=1\nenable-smartcard=0\n\
         secure-attention=ctrl+alt+end\ntoggle-fullscreen=shift+f11\nrelease-cursor=shift+f12\n",
        info.port, info.password
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_caps_are_read_from_their_offset() {
        // num_common_caps=1 @6, caps_offset=18 @14, cap word 0b11 @18
        let mut mess = vec![0u8; 22];
        mess[6] = 1;
        mess[14] = 18;
        mess[18] = 0b11;
        assert_eq!(first_common_cap(&mess, 6, 14), 0b11);
        mess[6] = 0;
        assert_eq!(first_common_cap(&mess, 6, 14), 0);
    }

    #[tokio::test]
    async fn relay_key_is_a_spice_sized_rsa_key() {
        let k = relay_key().await.unwrap();
        assert_eq!(k.1.len(), 162);
        let pw = new_password();
        assert_eq!(pw.len(), 16);
        let vv = virt_viewer_file(
            "vm1",
            "10.0.0.1",
            &RelayInfo {
                listen: "0.0.0.0".into(),
                port: 5999,
                password: pw.clone(),
            },
        );
        assert!(vv.contains("type=spice\nhost=10.0.0.1\nport=5999\n"));
        assert!(vv.contains(&format!("password={pw}\n")));
    }
}
