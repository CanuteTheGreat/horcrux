//! Talking to the node: TLS (the node's CA or a pinned certificate),
//! login, the console status and ticket API, and the API WebSocket that
//! carries the offer/answer (the web client's protocol).
use crate::config::Settings;
use futures::{SinkExt, StreamExt};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use serde_json::{json, Value};
use sha2::Digest;
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Accepts exactly one certificate (by SHA-256 of its DER), and records
/// what it saw so a refusal can name the fingerprint.
#[derive(Debug)]
struct Pinned {
    want: Option<Vec<u8>>,
    seen: Arc<Mutex<Option<String>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = sha2::Sha256::digest(end_entity.as_ref()).to_vec();
        if let Ok(mut s) = self.seen.lock() {
            *s = Some(hex::encode(&fp));
        }
        match &self.want {
            Some(w) if *w == fp => Ok(ServerCertVerified::assertion()),
            _ => Err(rustls::Error::General(
                "certificate fingerprint does not match".into(),
            )),
        }
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub struct Tls {
    pub config: Arc<rustls::ClientConfig>,
    seen: Arc<Mutex<Option<String>>>,
}

impl Tls {
    /// The fingerprint the node presented (after a failed handshake).
    pub fn seen(&self) -> Option<String> {
        self.seen.lock().ok().and_then(|s| s.clone())
    }
}

pub fn tls(s: &Settings) -> Result<Tls, String> {
    let p = provider();
    let builder = rustls::ClientConfig::builder_with_provider(p.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let seen = Arc::new(Mutex::new(None));
    let config = if !s.ca_file.is_empty() {
        let pem = std::fs::read(&s.ca_file).map_err(|e| format!("{}: {e}", s.ca_file))?;
        let mut roots = rustls::RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut pem.as_slice()) {
            roots
                .add(c.map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        if roots.is_empty() {
            return Err(format!("{}: no certificate", s.ca_file));
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        let want = (!s.cert_sha256.is_empty())
            .then(|| hex::decode(s.cert_sha256.replace(':', "").to_ascii_lowercase()))
            .transpose()
            .map_err(|e| e.to_string())?;
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(Pinned {
                want,
                seen: seen.clone(),
                provider: p,
            }))
            .with_no_client_auth()
    };
    Ok(Tls {
        config: Arc::new(config),
        seen,
    })
}

pub struct Api {
    pub base: String,
    pub http: reqwest::Client,
    pub token: String,
    tls: Arc<rustls::ClientConfig>,
}

fn host_port(base: &str) -> Result<(String, u16, bool), String> {
    let u = reqwest::Url::parse(base).map_err(|e| format!("node URL: {e}"))?;
    let tls = u.scheme() == "https";
    let host = u.host_str().ok_or("node URL has no host")?.to_string();
    let port = u.port_or_known_default().ok_or("node URL has no port")?;
    Ok((host, port, tls))
}

impl Api {
    pub async fn login(s: &Settings, t: &Tls, password: &str) -> Result<Api, String> {
        let http = reqwest::Client::builder()
            .use_preconfigured_tls((*t.config).clone())
            .user_agent(concat!("horcrux-console/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| e.to_string())?;
        let base = s.node.trim_end_matches('/').to_string();
        let r = http
            .post(format!("{base}/api/auth/login"))
            .json(&json!({"username": s.user, "password": password}))
            .send()
            .await
            .map_err(|e| match t.seen() {
                Some(fp) if s.ca_file.is_empty() => format!(
                    "the node's certificate is not trusted (SHA-256 {fp}); check it on the node and set cert_sha256, or set ca_file ({e})"
                ),
                _ => format!("login: {e}"),
            })?;
        let status = r.status();
        let v: Value = r.json().await.map_err(|e| format!("login: {e}"))?;
        if !status.is_success() {
            return Err(format!("login refused: {status} {v}"));
        }
        let token = v
            .get("ticket")
            .or_else(|| v.get("token"))
            .and_then(Value::as_str)
            .ok_or("login answer has no ticket")?
            .to_string();
        Ok(Api {
            base,
            http,
            token,
            tls: t.config.clone(),
        })
    }

    pub async fn get(&self, path: &str) -> Result<Value, String> {
        let r = self
            .http
            .get(format!("{}/api{path}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let st = r.status();
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        if !st.is_success() {
            return Err(format!("GET {path}: {st} {v}"));
        }
        Ok(v)
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let r = self
            .http
            .post(format!("{}/api{path}", self.base))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let st = r.status();
        let v: Value = r.json().await.map_err(|e| e.to_string())?;
        if !st.is_success() {
            return Err(format!("POST {path}: {st} {v}"));
        }
        Ok(v)
    }

    /// The API WebSocket (console signalling), authenticated like the API.
    pub async fn signalling(&self) -> Result<Signalling, String> {
        let (host, port, tls) = host_port(&self.base)?;
        let scheme = if tls { "wss" } else { "ws" };
        let url = format!("{scheme}://{host}:{port}/api/ws");
        let req = http::Request::builder()
            .uri(&url)
            .header("Host", format!("{host}:{port}"))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            )
            .body(())
            .map_err(|e| e.to_string())?;
        let tcp = TcpStream::connect((host.as_str(), port))
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        let _ = tcp.set_nodelay(true);
        let stream: Box<dyn AsyncStream> = if tls {
            let name = ServerName::try_from(host.clone()).map_err(|e| e.to_string())?;
            let s = tokio_rustls::TlsConnector::from(self.tls.clone())
                .connect(name, tcp)
                .await
                .map_err(|e| format!("{url}: {e}"))?;
            Box::new(s)
        } else {
            Box::new(tcp)
        };
        let (mut ws, _) = tokio_tungstenite::client_async(req, stream)
            .await
            .map_err(|e| format!("{url}: {e}"))?;
        ws.send(Message::Text(json!({"type": "console.hello"}).to_string()))
            .await
            .map_err(|e| e.to_string())?;
        loop {
            let m = ws
                .next()
                .await
                .ok_or("signalling closed")?
                .map_err(|e| e.to_string())?;
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                if v["type"] == "ConsoleHello" {
                    let conn_id = v["data"]["conn_id"]
                        .as_str()
                        .ok_or("hello without conn_id")?
                        .to_string();
                    return Ok(Signalling { ws, conn_id });
                }
            }
        }
    }
}

pub trait AsyncStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> AsyncStream for T {}

pub struct Signalling {
    pub ws: tokio_tungstenite::WebSocketStream<Box<dyn AsyncStream>>,
    pub conn_id: String,
}

impl Signalling {
    /// Send the offer with the ticket; the node's answer (session id, SDP).
    pub async fn offer(&mut self, ticket: &str, sdp: &str) -> Result<(String, String), String> {
        self.ws
            .send(Message::Text(
                json!({"type": "console.offer", "ticket": ticket, "sdp": sdp}).to_string(),
            ))
            .await
            .map_err(|e| e.to_string())?;
        loop {
            let m = self
                .ws
                .next()
                .await
                .ok_or("signalling closed")?
                .map_err(|e| e.to_string())?;
            if let Message::Text(t) = m {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                match v["type"].as_str() {
                    Some("ConsoleAnswer") => {
                        return Ok((
                            v["data"]["session"]
                                .as_str()
                                .unwrap_or_default()
                                .to_string(),
                            v["data"]["sdp"]
                                .as_str()
                                .ok_or("answer without SDP")?
                                .to_string(),
                        ))
                    }
                    Some("ConsoleError") => {
                        return Err(v["data"]["message"]
                            .as_str()
                            .unwrap_or("refused")
                            .to_string())
                    }
                    _ => {}
                }
            }
        }
    }
}

/// `turn:host:port[?transport=udp]` from a ticket's ICE servers (UDP only).
pub fn turn_server(ice_servers: &Value) -> Option<(String, String, String)> {
    for s in ice_servers.as_array()? {
        let user = s["username"].as_str()?.to_string();
        let cred = s["credential"].as_str()?.to_string();
        let urls: Vec<&str> = match &s["urls"] {
            Value::String(u) => vec![u.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        for u in urls {
            let Some(rest) = u.strip_prefix("turn:") else {
                continue;
            };
            let (hp, q) = rest.split_once('?').unwrap_or((rest, ""));
            if q.is_empty() || q.contains("transport=udp") {
                return Some((hp.to_string(), user, cred));
            }
        }
    }
    None
}

/// Host candidates (`ip:port`) in an SDP.
pub fn candidates(sdp: &str) -> Vec<std::net::SocketAddr> {
    sdp.lines()
        .filter_map(|l| l.trim().strip_prefix("a=candidate:"))
        .filter_map(|c| {
            let f: Vec<&str> = c.split_whitespace().collect();
            (f.len() >= 6 && f[2].eq_ignore_ascii_case("udp"))
                .then(|| format!("{}:{}", f[4], f[5]).parse().ok())?
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ticket_relay_and_answer_candidates() {
        let v = json!([{"urls": ["turn:10.0.0.1:3478?transport=tcp", "turn:10.0.0.1:3478?transport=udp"], "username": "u", "credential": "p"}]);
        assert_eq!(
            turn_server(&v),
            Some(("10.0.0.1:3478".into(), "u".into(), "p".into()))
        );
        assert_eq!(turn_server(&json!([])), None);
        let sdp = "v=0\r\na=candidate:1 1 udp 2130706431 10.77.0.1 61200 typ host\r\na=candidate:2 1 tcp 1 10.77.0.1 9 typ host\r\n";
        assert_eq!(candidates(sdp), vec!["10.77.0.1:61200".parse().unwrap()]);
        assert_eq!(
            host_port("https://n.example:8006/").unwrap(),
            ("n.example".into(), 8006, true)
        );
    }

    #[test]
    fn tls_needs_a_trust_anchor_or_reports_the_fingerprint() {
        let s = Settings {
            node: "https://x".into(),
            vm: "v".into(),
            ..Default::default()
        };
        let t = tls(&s).unwrap();
        assert!(t.seen().is_none());
        let bad = Settings {
            ca_file: "/nonexistent.pem".into(),
            ..s
        };
        assert!(tls(&bad).is_err());
    }
}
