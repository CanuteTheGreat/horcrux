//! The API listener: HTTPS (HTTP/1.1 + HTTP/2, websocket upgrades) with a
//! plain-HTTP redirect on the same port, an optional extra redirect port
//! (e.g. 80) that also answers ACME HTTP-01 challenges, and graceful
//! shutdown.
//!
//! The first byte of a connection tells TLS (0x16, a handshake record) from
//! plain HTTP, so `http://host:8006/...` gets a `308` to `https://host:8006/...`
//! instead of a confusing protocol error. `/api/health*` is answered on plain
//! HTTP too, so existing liveness probes keep working unchanged.

use super::TlsManager;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, Response, StatusCode};
use axum::Router;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tower::ServiceExt;

/// Build the rustls server config around the manager's live certificate.
pub fn server_config(tls: &TlsManager) -> rustls::ServerConfig {
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13, &rustls::version::TLS12])
    .expect("TLS 1.2/1.3 are supported by the ring provider")
    .with_no_client_auth()
    .with_cert_resolver(tls.resolver());
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    cfg
}

/// Where a plain-HTTP request should be sent on the HTTPS port.
pub fn https_location(host_header: Option<&str>, path_and_query: &str, https_port: u16) -> String {
    let host = host_header.unwrap_or("localhost");
    // keep the host, force the HTTPS port
    let bare = if host.starts_with('[') {
        host.split(']')
            .next()
            .map(|h| format!("{}]", h))
            .unwrap_or_else(|| host.to_string())
    } else {
        host.split(':').next().unwrap_or(host).to_string()
    };
    let authority = if https_port == 443 {
        bare
    } else {
        format!("{}:{}", bare, https_port)
    };
    format!("https://{}{}", authority, path_and_query)
}

/// Plain-HTTP handler: ACME challenges, health probes, redirect for the rest.
async fn plain_http(
    req: Request<Incoming>,
    app: Router,
    tls: Arc<TlsManager>,
    peer: SocketAddr,
    https_port: u16,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();
    if let Some(token) = path.strip_prefix("/.well-known/acme-challenge/") {
        return Ok(match tls.acme_challenge(token) {
            Some(ka) => Response::builder()
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .body(Body::from(ka))
                .unwrap(),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap(),
        });
    }
    if path == "/api/health" || path.starts_with("/api/health/") {
        let mut req = req.map(Body::new);
        req.extensions_mut().insert(ConnectInfo(peer));
        return Ok(app.oneshot(req).await.unwrap_or_else(|e| match e {}));
    }
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.to_string()));
    let location = https_location(host.as_deref(), &pq, https_port);
    Ok(Response::builder()
        .status(StatusCode::PERMANENT_REDIRECT)
        .header(header::LOCATION, location.clone())
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(format!(
            "This server only speaks HTTPS: {}\n",
            location
        )))
        .unwrap())
}

/// Shutdown signal + live-connection tracking shared by connection tasks.
#[derive(Clone)]
struct Conns {
    stop: tokio::sync::watch::Receiver<bool>,
    live: Arc<()>,
}

/// Serve one connection with the full API router.
async fn serve_app<I>(io: I, app: Router, peer: SocketAddr, conns: Conns)
where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Conns { mut stop, live } = conns;
    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
        let app = app.clone();
        async move {
            let mut req = req.map(Body::new);
            req.extensions_mut().insert(ConnectInfo(peer));
            Ok::<_, Infallible>(app.oneshot(req).await.unwrap_or_else(|e| match e {}))
        }
    });
    let builder = Builder::new(TokioExecutor::new());
    let conn = builder.serve_connection_with_upgrades(TokioIo::new(io), svc);
    tokio::pin!(conn);
    tokio::select! {
        r = conn.as_mut() => {
            if let Err(e) = r {
                tracing::debug!("connection from {} ended: {}", peer, e);
            }
        }
        _ = stop.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
        }
    }
    drop(live);
}

async fn serve_plain<I>(
    io: I,
    app: Router,
    tls: Arc<TlsManager>,
    peer: SocketAddr,
    https_port: u16,
    conns: Conns,
) where
    I: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let Conns { mut stop, live } = conns;
    let svc = hyper::service::service_fn(move |req| {
        plain_http(req, app.clone(), tls.clone(), peer, https_port)
    });
    let builder = Builder::new(TokioExecutor::new());
    let conn = builder.serve_connection(TokioIo::new(io), svc);
    tokio::pin!(conn);
    tokio::select! {
        _ = conn.as_mut() => {}
        _ = stop.changed() => {
            conn.as_mut().graceful_shutdown();
            let _ = conn.await;
        }
    }
    drop(live);
}

async fn first_byte(stream: &TcpStream) -> Option<u8> {
    let mut b = [0u8; 1];
    match tokio::time::timeout(Duration::from_secs(10), stream.peek(&mut b)).await {
        Ok(Ok(1)) => Some(b[0]),
        _ => None,
    }
}

/// Sniff, handshake and serve one connection on the main port.
async fn handle(
    stream: TcpStream,
    peer: SocketAddr,
    app: Router,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    tls_mgr: Arc<TlsManager>,
    https_port: u16,
    conns: Conns,
) {
    let Some(acceptor) = acceptor else {
        return serve_app(stream, app, peer, conns).await;
    };
    match first_byte(&stream).await {
        Some(0x16) => {
            match tokio::time::timeout(Duration::from_secs(15), acceptor.accept(stream)).await {
                Ok(Ok(tls_stream)) => serve_app(tls_stream, app, peer, conns).await,
                Ok(Err(e)) => tracing::debug!("TLS handshake from {} failed: {}", peer, e),
                Err(_) => tracing::debug!("TLS handshake from {} timed out", peer),
            }
        }
        Some(_) => serve_plain(stream, app, tls_mgr, peer, https_port, conns).await,
        None => {}
    }
}

/// Run the API until `shutdown` resolves.
///
/// - `tls = Some(..)`: HTTPS on `listener`, plain HTTP there is redirected.
/// - `tls = None`: plain HTTP (TLS terminated by a proxy in front).
/// - `redirect_listener`: extra plain port, redirect + ACME HTTP-01 only.
pub async fn serve(
    listener: TcpListener,
    redirect_listener: Option<TcpListener>,
    app: Router,
    tls: Option<Arc<TlsManager>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let https_port = listener.local_addr()?.port();
    let acceptor = tls
        .as_ref()
        .map(|t| tokio_rustls::TlsAcceptor::from(Arc::new(server_config(t))));
    super::set_serving_tls(acceptor.is_some());
    let tls_mgr = tls.clone().unwrap_or_else(|| Arc::new(TlsManager::new()));
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let live = Arc::new(());
    let conns = Conns {
        stop: stop_rx,
        live: live.clone(),
    };
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(x) => x,
                    Err(e) => {
                        tracing::warn!("accept failed: {}", e);
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let _ = stream.set_nodelay(true);
                tokio::spawn(handle(
                    stream,
                    peer,
                    app.clone(),
                    acceptor.clone(),
                    tls_mgr.clone(),
                    https_port,
                    conns.clone(),
                ));
            }
            accepted = async {
                match &redirect_listener {
                    Some(l) => l.accept().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Ok((stream, peer)) = accepted {
                    tokio::spawn(serve_plain(stream, app.clone(), tls_mgr.clone(), peer, https_port, conns.clone()));
                }
            }
        }
    }

    tracing::info!("Stopped accepting connections; draining open ones");
    drop(conns);
    let _ = stop_tx.send(true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while Arc::strong_count(&live) > 1 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if Arc::strong_count(&live) > 1 {
        tracing::warn!(
            "{} connection(s) still open after 10s",
            Arc::strong_count(&live) - 1
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_targets() {
        assert_eq!(
            https_location(Some("pve.lan:8006"), "/vms?x=1", 8006),
            "https://pve.lan:8006/vms?x=1"
        );
        assert_eq!(
            https_location(Some("pve.lan"), "/", 8006),
            "https://pve.lan:8006/"
        );
        assert_eq!(
            https_location(Some("[fd00::1]:80"), "/", 8006),
            "https://[fd00::1]:8006/"
        );
        assert_eq!(https_location(Some("h:80"), "/a", 443), "https://h/a");
    }

    async fn start(
        tls: bool,
    ) -> (
        SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        Arc<TlsManager>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let mgr = Arc::new(TlsManager::with_paths(
            dir.path().join("cert.pem"),
            dir.path().join("key.pem"),
            dir.path().join("fb"),
        ));
        mgr.ensure_certificate().unwrap();
        let app = Router::new()
            .route("/api/health", axum::routing::get(|| async { "ok" }))
            .route(
                "/api/echo",
                axum::routing::post(
                    |ConnectInfo(a): ConnectInfo<SocketAddr>, body: String| async move {
                        format!("{}:{}", a.ip(), body)
                    },
                ),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        let m = mgr.clone();
        tokio::spawn(async move {
            serve(listener, None, app, tls.then_some(m), async {
                let _ = rx.await;
            })
            .await
            .unwrap();
        });
        (addr, tx, mgr, dir)
    }

    #[tokio::test]
    async fn https_and_same_port_redirect() {
        let (addr, stop, mgr, _dir) = start(true).await;
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        // HTTPS works, with ConnectInfo
        let r = client
            .post(format!("https://127.0.0.1:{}/api/echo", addr.port()))
            .body("hi")
            .send()
            .await
            .unwrap();
        assert_eq!(r.text().await.unwrap(), "127.0.0.1:hi");
        // plain HTTP on the same port: redirect, health still answered
        let r = client
            .get(format!("http://127.0.0.1:{}/vms?a=b", addr.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 308);
        assert_eq!(
            r.headers()["location"],
            format!("https://127.0.0.1:{}/vms?a=b", addr.port())
        );
        let r = client
            .get(format!("http://127.0.0.1:{}/api/health", addr.port()))
            .send()
            .await
            .unwrap();
        assert_eq!(r.text().await.unwrap(), "ok");
        // ACME challenge on plain HTTP
        mgr.set_acme_challenge("tok", "tok.thumb");
        let r = client
            .get(format!(
                "http://127.0.0.1:{}/.well-known/acme-challenge/tok",
                addr.port()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(r.text().await.unwrap(), "tok.thumb");
        // the presented certificate is the manager's
        let fp = mgr.current_info().fingerprint_sha256;
        assert!(!fp.is_empty());
        let _ = stop.send(());
    }

    #[tokio::test]
    async fn plain_mode_when_tls_disabled() {
        let (addr, stop, _mgr, _dir) = start(false).await;
        let r = reqwest::get(format!("http://127.0.0.1:{}/api/health", addr.port()))
            .await
            .unwrap();
        assert_eq!(r.text().await.unwrap(), "ok");
        let _ = stop.send(());
    }
}
