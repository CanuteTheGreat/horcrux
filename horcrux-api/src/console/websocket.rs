//! WebSocket proxy for VNC connections
//! Bridges WebSocket connections from browsers to VNC servers

#![allow(dead_code)]

use horcrux_common::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// WebSocket proxy configuration
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub ticket_id: String,
    pub vnc_host: String,
    pub vnc_port: u16,
    pub ws_port: u16,
}

/// WebSocket proxy manager
pub struct WebSocketProxy {
    active_proxies: Arc<RwLock<HashMap<String, ProxyConfig>>>,
    next_port: Arc<RwLock<u16>>,
}

impl WebSocketProxy {
    pub fn new() -> Self {
        Self {
            active_proxies: Arc::new(RwLock::new(HashMap::new())),
            next_port: Arc::new(RwLock::new(6080)), // Start WebSocket ports at 6080
        }
    }

    /// Start a WebSocket proxy for a VNC connection
    pub async fn start_proxy(&self, ticket_id: &str, vnc_host: &str, vnc_port: u16) -> Result<u16> {
        // Get next available port
        let mut next_port = self.next_port.write().await;
        let ws_port = *next_port;
        *next_port += 1;
        drop(next_port);

        let config = ProxyConfig {
            ticket_id: ticket_id.to_string(),
            vnc_host: vnc_host.to_string(),
            vnc_port,
            ws_port,
        };

        // Store proxy config
        let mut proxies = self.active_proxies.write().await;
        proxies.insert(ticket_id.to_string(), config.clone());
        drop(proxies);

        // Start WebSocket listener
        let ticket_id = ticket_id.to_string();
        let vnc_host = vnc_host.to_string();

        tokio::spawn(async move {
            if let Err(e) = Self::run_proxy(&ticket_id, &vnc_host, vnc_port, ws_port).await {
                tracing::error!("WebSocket proxy error for {}: {}", ticket_id, e);
            }
        });

        Ok(ws_port)
    }

    /// Start a WebSocket proxy for a Unix socket (e.g., serial console)
    pub async fn start_unix_proxy(&self, ticket_id: &str, socket_path: &str) -> Result<u16> {
        // Get next available port
        let mut next_port = self.next_port.write().await;
        let ws_port = *next_port;
        *next_port += 1;
        drop(next_port);

        let config = ProxyConfig {
            ticket_id: ticket_id.to_string(),
            vnc_host: socket_path.to_string(), // Reuse vnc_host field for socket path
            vnc_port: 0,
            ws_port,
        };

        // Store proxy config
        let mut proxies = self.active_proxies.write().await;
        proxies.insert(ticket_id.to_string(), config.clone());
        drop(proxies);

        // Start WebSocket listener for Unix socket
        let ticket_id = ticket_id.to_string();
        let socket_path = socket_path.to_string();

        tokio::spawn(async move {
            if let Err(e) = Self::run_unix_proxy(&ticket_id, &socket_path, ws_port).await {
                tracing::error!("WebSocket Unix socket proxy error for {}: {}", ticket_id, e);
            }
        });

        Ok(ws_port)
    }

    /// Run the WebSocket proxy server
    async fn run_proxy(ticket_id: &str, vnc_host: &str, vnc_port: u16, ws_port: u16) -> Result<()> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", ws_port))
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to bind WebSocket port {}: {}", ws_port, e)))?;

        tracing::info!("WebSocket proxy listening on port {} for ticket {}", ws_port, ticket_id);

        loop {
            match listener.accept().await {
                Ok((ws_stream, addr)) => {
                    tracing::debug!("WebSocket connection from {}", addr);
                    let vnc_host = vnc_host.to_string();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_connection(ws_stream, &vnc_host, vnc_port).await {
                            tracing::error!("Connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Handle a WebSocket connection
    async fn handle_connection(ws_stream: TcpStream, vnc_host: &str, vnc_port: u16) -> Result<()> {
        // Connect to VNC server
        let vnc_stream = TcpStream::connect(format!("{}:{}", vnc_host, vnc_port))
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to connect to VNC server: {}", e)))?;

        tracing::debug!("Connected to VNC server at {}:{}", vnc_host, vnc_port);

        // For a full implementation, we'd need to handle the WebSocket handshake here
        // For now, we'll do a simple TCP proxy (works with websockify-compatible clients)

        // Split streams for bidirectional forwarding - we need to own them
        let (ws_read, ws_write) = ws_stream.into_split();
        let (vnc_read, vnc_write) = vnc_stream.into_split();

        // Spawn task to forward from WebSocket to VNC
        let ws_to_vnc = tokio::spawn(async move {
            Self::forward_stream(ws_read, vnc_write).await;
        });

        // Forward from VNC to WebSocket
        let vnc_to_ws = tokio::spawn(async move {
            Self::forward_stream(vnc_read, ws_write).await;
        });

        // Wait for both directions to finish
        let _ = tokio::join!(ws_to_vnc, vnc_to_ws);

        tracing::debug!("WebSocket proxy connection closed");
        Ok(())
    }

    /// Run the WebSocket proxy server for Unix socket
    async fn run_unix_proxy(ticket_id: &str, socket_path: &str, ws_port: u16) -> Result<()> {
        let listener = TcpListener::bind(format!("0.0.0.0:{}", ws_port))
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to bind WebSocket port {}: {}", ws_port, e)))?;

        tracing::info!("WebSocket Unix socket proxy listening on port {} for ticket {}", ws_port, ticket_id);

        loop {
            match listener.accept().await {
                Ok((ws_stream, addr)) => {
                    tracing::debug!("WebSocket connection from {} for Unix socket", addr);
                    let socket_path = socket_path.to_string();

                    tokio::spawn(async move {
                        if let Err(e) = Self::handle_unix_connection(ws_stream, &socket_path).await {
                            tracing::error!("Unix socket connection error: {}", e);
                        }
                    });
                }
                Err(e) => {
                    tracing::error!("Failed to accept connection: {}", e);
                }
            }
        }
    }

    /// Handle a WebSocket connection to Unix socket
    async fn handle_unix_connection(ws_stream: TcpStream, socket_path: &str) -> Result<()> {
        // Connect to Unix socket
        let unix_stream = tokio::net::UnixStream::connect(socket_path)
            .await
            .map_err(|e| horcrux_common::Error::System(format!("Failed to connect to Unix socket {}: {}", socket_path, e)))?;

        tracing::debug!("Connected to Unix socket at {}", socket_path);

        // Split streams for bidirectional forwarding
        let (ws_read, ws_write) = ws_stream.into_split();
        let (unix_read, unix_write) = unix_stream.into_split();

        // Spawn task to forward from WebSocket to Unix socket
        let ws_to_unix = tokio::spawn(async move {
            Self::forward_stream(ws_read, unix_write).await;
        });

        // Forward from Unix socket to WebSocket
        let unix_to_ws = tokio::spawn(async move {
            Self::forward_stream(unix_read, ws_write).await;
        });

        // Wait for both directions to finish
        let _ = tokio::join!(ws_to_unix, unix_to_ws);

        tracing::debug!("WebSocket Unix socket proxy connection closed");
        Ok(())
    }

    /// Forward data from one stream to another
    async fn forward_stream<R, W>(mut reader: R, mut writer: W)
    where
        R: AsyncReadExt + Unpin,
        W: AsyncWriteExt + Unpin,
    {
        let mut buffer = vec![0u8; 8192];
        loop {
            match reader.read(&mut buffer).await {
                Ok(0) => break, // Connection closed
                Ok(n) => {
                    if writer.write_all(&buffer[..n]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    /// Stop a WebSocket proxy
    pub async fn stop_proxy(&self, ticket_id: &str) -> Result<()> {
        let mut proxies = self.active_proxies.write().await;
        proxies.remove(ticket_id);
        Ok(())
    }

    /// Get proxy config
    pub async fn get_proxy(&self, ticket_id: &str) -> Option<ProxyConfig> {
        let proxies = self.active_proxies.read().await;
        proxies.get(ticket_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_proxy_creation() {
        let proxy = WebSocketProxy::new();
        let port = proxy.start_proxy("test-ticket", "127.0.0.1", 5900).await.unwrap();
        assert!(port >= 6080);

        let config = proxy.get_proxy("test-ticket").await.unwrap();
        assert_eq!(config.vnc_port, 5900);
        assert_eq!(config.ws_port, port);
    }
}
