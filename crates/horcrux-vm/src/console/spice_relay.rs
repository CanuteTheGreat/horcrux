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
use super::lxd::{Backend, Client};
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
/// Per attempt: opening a daemon channel and reading its SPICE link reply
/// (HORCRUX_SPICE_LINK_TIMEOUT_SECS, default 8).
fn upstream_link_timeout() -> std::time::Duration {
    std::time::Duration::from_secs(super::lxd::env_secs("HORCRUX_SPICE_LINK_TIMEOUT_SECS", 8))
}
/// SPICE link errors (spice-protocol `SpiceLinkErr`).
const LINK_ERR_PERMISSION_DENIED: u32 = 7;

#[derive(Clone, Debug)]
pub struct RelayInfo {
    /// Address the relay listens on.
    pub listen: String,
    pub port: u16,
    pub password: String,
}

/// A value shared by every connection of a relay, created by the first one and
/// dropped when the last one ends (here: the VM's single `vga` operation).
struct Shared<T> {
    inner: Mutex<(Option<Arc<T>>, usize)>,
    /// Held while a value is being torn down on the daemon, and by every join:
    /// a new operation is not opened until the old one is really closed
    /// (Incus refuses a second vga session while the first is still ending).
    gate: Mutex<()>,
}

impl<T> Shared<T> {
    fn new() -> Self {
        Self {
            inner: Mutex::new((None, 0)),
            gate: Mutex::new(()),
        }
    }

    /// Join: the current value, `open`ed first when there is none.
    async fn join<F>(&self, open: F) -> Result<Arc<T>>
    where
        F: std::future::Future<Output = Result<T>>,
    {
        let _gate = self.gate.lock().await;
        let mut g = self.inner.lock().await;
        let v = match &g.0 {
            Some(v) => v.clone(),
            None => {
                let v = Arc::new(open.await?);
                g.0 = Some(v.clone());
                v
            }
        };
        g.1 += 1;
        Ok(v)
    }

    /// Leave. The last one out gets the value back to tear it down. `broken`
    /// drops the value now (a dead daemon operation) so the next join opens a
    /// fresh one; the stale value is returned to the caller that saw it break.
    async fn leave(&self, broken: Option<&Arc<T>>) -> Option<Arc<T>> {
        let mut g = self.inner.lock().await;
        g.1 = g.1.saturating_sub(1);
        if let (Some(b), Some(cur)) = (broken, &g.0) {
            if Arc::ptr_eq(b, cur) {
                return g.0.take();
            }
        }
        if g.1 == 0 {
            g.0.take()
        } else {
            None
        }
    }
}

type VgaShare = Shared<super::lxd::VgaOp>;

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
    let share: Arc<VgaShare> = Arc::new(Shared::new());
    let task = tokio::spawn(run(
        listener,
        backend,
        instance.to_string(),
        info.password.clone(),
        rsa,
        last_active.clone(),
        active.clone(),
        share,
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

#[allow(clippy::too_many_arguments)]
async fn run(
    listener: TcpListener,
    backend: Backend,
    instance: String,
    password: String,
    rsa: Arc<(RsaPrivateKey, Vec<u8>)>,
    last_active: Arc<AtomicI64>,
    active: Arc<AtomicUsize>,
    share: Arc<VgaShare>,
) {
    loop {
        let accepted =
            tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept()).await;
        match accepted {
            Ok(Ok((sock, peer))) => {
                last_active.store(now(), Ordering::Relaxed);
                active.fetch_add(1, Ordering::Relaxed);
                let (backend, instance, password, rsa, last_active, active, share) = (
                    backend,
                    instance.clone(),
                    password.clone(),
                    rsa.clone(),
                    last_active.clone(),
                    active.clone(),
                    share.clone(),
                );
                tokio::spawn(async move {
                    if let Err(e) = handle(sock, backend, &instance, &password, &rsa, &share).await
                    {
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
    client: TcpStream,
    backend: Backend,
    instance: &str,
    password: &str,
    rsa: &(RsaPrivateKey, Vec<u8>),
    share: &VgaShare,
) -> Result<()> {
    let client_api = Client::new(backend);
    let mut op_used: Option<Arc<super::lxd::VgaOp>> = None;
    let r = handle_inner(
        client,
        &client_api,
        instance,
        password,
        rsa,
        share,
        &mut op_used,
    )
    .await;
    // Leave the shared vga operation; the last connection out ends it so the
    // VM's console is free again.
    if let Some(used) = op_used.take() {
        let _gate = share.gate.lock().await;
        let gone = share.leave(None).await;
        drop(used);
        if let Some(g) = gone {
            if let Ok(op) = Arc::try_unwrap(g) {
                client_api.close_vga_op(op).await;
            }
        }
    }
    r
}

async fn handle_inner(
    mut client: TcpStream,
    api: &Client,
    instance: &str,
    password: &str,
    rsa: &(RsaPrivateKey, Vec<u8>),
    share: &VgaShare,
    op_used: &mut Option<Arc<super::lxd::VgaOp>>,
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
    let channel_type = mess.get(4).copied().unwrap_or(0);

    // The daemon can refuse or drop a vga session right after another one of the
    // same VM closed (SPICE opens one per channel and clients disconnect in
    // quick succession; incus-vm-console run 1346 saw the third session of a
    // burst closed before any reply). Retry opening the session and reading the
    // daemon's link reply a few times so the client gets a real answer.
    //
    // All channels of a VM ride ONE daemon `vga` operation (Incus >= 6.0.4
    // refuses a second one: "This console is already connected"); each channel
    // is another websocket on it. A dead operation is replaced on the retry.
    let mut attempt = 0u32;
    let (mut up, rhdr, mut reply) = loop {
        attempt += 1;
        let tried: Result<_> = async {
            let op = share.join(api.open_vga_op(instance)).await?;
            *op_used = Some(op.clone());
            // Bounded: a daemon that stops answering (crashed, restarting)
            // must not leave the client waiting forever; the next attempt
            // opens a fresh operation.
            tokio::time::timeout(upstream_link_timeout(), async {
                let mut up = Upstream {
                    ws: api.vga_channel(&op).await?,
                    buf: Vec::new(),
                };
                up.write(&[hdr.as_slice(), mess.as_slice()].concat())
                    .await?;
                let rhdr = up.read_n(16).await?;
                let rsize = u32_at(&rhdr, 12) as usize;
                let reply = up.read_n(rsize).await?;
                Ok((up, rhdr, reply))
            })
            .await
            .map_err(|_| Error::System("SPICE display did not answer the link".into()))?
        }
        .await;
        match tried {
            Ok(v) => break v,
            Err(e) => {
                // Leave (and drop, as broken) the operation this attempt used.
                if let Some(used) = op_used.take() {
                    let _gate = share.gate.lock().await;
                    if let Some(g) = share.leave(Some(&used)).await {
                        drop(used);
                        if let Ok(op) = Arc::try_unwrap(g) {
                            api.close_vga_op(op).await;
                        }
                    }
                }
                if attempt >= 8 {
                    return Err(e);
                }
                warn!("SPICE relay {instance}: upstream link attempt {attempt} failed ({e}), retrying");
                tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
            }
        }
    };
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
    let mut upstream_seen = leftover.len();
    if !leftover.is_empty() {
        client.write_all(&leftover).await.map_err(io)?;
    }
    let (mut c_rx, mut c_tx) = client.into_split();
    let (mut u_tx, mut u_rx) = up.ws.split();
    let mut client_sent = 0usize;
    let to_vm = async {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match c_rx.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    client_sent += n;
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
            upstream_seen += data.len();
            if c_tx.write_all(&data).await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        _ = to_vm => {}
        _ = to_client => {}
    }
    // A display channel whose client left before sending DISPLAY_INIT: the
    // VM's SPICE server sits in a synchronous wait for that message, and
    // when the channel's socket closes during the wait (the daemon closes it
    // when the vga operation ends) spice-server 0.15 frees the channel
    // client inside its own wait loop and QEMU dies with a NULL dereference
    // in the SPICE worker (RedChannelClient::is_connected from dcc_start).
    // Complete the init on the client's behalf and keep the channel open
    // until the server is past its wait (it then sends more than the one
    // SET_ACK message it sends before waiting), so the channel is a normal,
    // fully started channel when it closes. Init and close in one read of
    // the server still crash it.
    if channel_type == SPICE_CHANNEL_DISPLAY && client_sent == 0 {
        let mini = client_caps & server_caps & COMMON_CAP_MINI_HEADER != 0;
        if u_tx
            .send(WsMessage::Binary(display_init_message(mini)))
            .await
            .is_ok()
        {
            let pre_init = set_ack_len(mini);
            let _ = tokio::time::timeout(upstream_link_timeout(), async {
                while upstream_seen <= pre_init {
                    match u_rx.next().await {
                        Some(Ok(WsMessage::Binary(d))) => upstream_seen += d.len(),
                        Some(Ok(WsMessage::Text(t))) => upstream_seen += t.len(),
                        Some(Ok(WsMessage::Close(_))) | Some(Err(_)) | None => break,
                        Some(Ok(_)) => {}
                    }
                }
            })
            .await;
        }
    }
    // The daemon keeps this channel's socket to the VM's SPICE server open
    // until the whole vga operation ends (it closes per-channel sockets only
    // then). Keep reading it until then so the SPICE server never blocks on
    // a channel nobody reads; the operation ends when the last channel
    // leaves.
    tokio::spawn(drain_until_closed(u_tx, u_rx));
    Ok(())
}

/// SPICE channel type of the display channel.
const SPICE_CHANNEL_DISPLAY: u8 = 2;
/// Common capability bit: mini message header (spice-protocol
/// SPICE_COMMON_CAP_MINI_HEADER).
const COMMON_CAP_MINI_HEADER: u32 = 1 << 3;

/// Size of the SET_ACK message (header + generation u32 + window u32), the
/// only message a display channel sends before the client's init.
fn set_ack_len(mini: bool) -> usize {
    if mini {
        6 + 8
    } else {
        18 + 8
    }
}

/// SPICE_MSGC_DISPLAY_INIT with the smallest caches (the channel is closing):
/// pixmap cache id 1 of size 0, GLZ dictionary id 1 with a 256 Ki-pixel
/// window. Mini header (type u16, size u32) when both sides negotiated it,
/// else the full header (serial u64 = 1, type, size, sub_list u32 = 0).
fn display_init_message(mini: bool) -> Vec<u8> {
    const MSGC_DISPLAY_INIT: u16 = 101;
    let mut body = Vec::with_capacity(14);
    body.push(1u8); // pixmap_cache_id
    body.extend_from_slice(&0i64.to_le_bytes()); // pixmap_cache_size
    body.push(1u8); // glz_dictionary_id
    body.extend_from_slice(&(256 * 1024i32).to_le_bytes()); // glz window
    let mut m = Vec::with_capacity(18 + body.len());
    if !mini {
        m.extend_from_slice(&1u64.to_le_bytes());
    }
    m.extend_from_slice(&MSGC_DISPLAY_INIT.to_le_bytes());
    m.extend_from_slice(&(body.len() as u32).to_le_bytes());
    if !mini {
        m.extend_from_slice(&0u32.to_le_bytes());
    }
    m.extend_from_slice(&body);
    m
}

/// Read and discard a daemon channel until the daemon closes it (bounded).
async fn drain_until_closed(
    tx: futures::stream::SplitSink<super::lxd::Ws, WsMessage>,
    mut rx: futures::stream::SplitStream<super::lxd::Ws>,
) {
    let _keep_open = tx;
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(super::lxd::env_secs("HORCRUX_SPICE_DRAIN_MAX_SECS", 600)),
        async {
            while let Some(m) = rx.next().await {
                if matches!(m, Ok(WsMessage::Close(_)) | Err(_)) {
                    break;
                }
            }
        },
    )
    .await;
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
    async fn shared_vga_op_is_opened_once_and_ended_by_the_last_user() {
        let sh: Shared<u32> = Shared::new();
        let opens = std::sync::atomic::AtomicUsize::new(0);
        let open = || async {
            opens.fetch_add(1, Ordering::Relaxed);
            Ok::<u32, Error>(7)
        };
        let a = sh.join(open()).await.unwrap();
        let b = sh.join(open()).await.unwrap();
        assert_eq!((*a, *b), (7, 7));
        assert_eq!(
            opens.load(Ordering::Relaxed),
            1,
            "second channel reuses the op"
        );
        drop(a);
        assert!(sh.leave(None).await.is_none(), "one channel still open");
        drop(b);
        assert!(
            sh.leave(None).await.is_some(),
            "last channel out ends the op"
        );
        let _c = sh.join(open()).await.unwrap();
        assert_eq!(
            opens.load(Ordering::Relaxed),
            2,
            "a new op after the last ended"
        );
    }

    #[tokio::test]
    async fn join_waits_until_the_old_op_is_closed() {
        let sh: Shared<u32> = Shared::new();
        let gate = sh.gate.lock().await;
        let pending = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            sh.join(async { Ok::<u32, Error>(1) }),
        )
        .await;
        assert!(pending.is_err(), "a join must wait while an op is closing");
        drop(gate);
        let v = sh.join(async { Ok::<u32, Error>(1) }).await.unwrap();
        assert_eq!(*v, 1);
    }

    #[tokio::test]
    async fn broken_vga_op_is_replaced_for_the_next_channel() {
        let sh: Shared<u32> = Shared::new();
        let a = sh.join(async { Ok::<u32, Error>(1) }).await.unwrap();
        let b = sh.join(async { Ok::<u32, Error>(1) }).await.unwrap();
        // channel a sees the op die: it is dropped at once, b's later leave is a no-op
        assert!(sh.leave(Some(&a)).await.is_some());
        let c = sh.join(async { Ok::<u32, Error>(2) }).await.unwrap();
        assert_eq!(*c, 2, "a fresh op after a broken one");
        drop(b);
        assert!(
            sh.leave(None).await.is_none(),
            "stale op's user does not end the new one"
        );
    }

    #[test]
    fn display_init_message_layout() {
        let full = display_init_message(false);
        assert_eq!(full.len(), 18 + 14);
        assert_eq!(
            u64::from_le_bytes(full[0..8].try_into().unwrap()),
            1,
            "serial"
        );
        assert_eq!(
            u16::from_le_bytes(full[8..10].try_into().unwrap()),
            101,
            "type"
        );
        assert_eq!(
            u32::from_le_bytes(full[10..14].try_into().unwrap()),
            14,
            "size"
        );
        assert_eq!(
            u32::from_le_bytes(full[14..18].try_into().unwrap()),
            0,
            "sub_list"
        );
        let mini = display_init_message(true);
        assert_eq!(mini.len(), 6 + 14);
        assert_eq!(u16::from_le_bytes(mini[0..2].try_into().unwrap()), 101);
        assert_eq!(u32::from_le_bytes(mini[2..6].try_into().unwrap()), 14);
        assert_eq!(&mini[6..], &full[18..], "same body");
        let body = &mini[6..];
        assert_eq!(body[0], 1, "pixmap cache id");
        assert_eq!(i64::from_le_bytes(body[1..9].try_into().unwrap()), 0);
        assert_eq!(body[9], 1, "glz dictionary id");
        assert_eq!(
            i32::from_le_bytes(body[10..14].try_into().unwrap()),
            256 * 1024
        );
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
