//! Graceful shutdown handling for Horcrux
//!
//! Provides coordinated shutdown of all services with:
//! - Signal handling (SIGTERM, SIGINT)
//! - Graceful connection draining
//! - Background task cancellation
//! - State persistence before exit

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::timeout;
use tracing::{error, info, warn};

/// Shutdown coordinator for graceful termination
pub struct ShutdownCoordinator {
    /// Watch channel for shutdown signal
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    /// Flag indicating shutdown has started
    is_shutting_down: Arc<AtomicBool>,
    /// Timeout for graceful shutdown
    timeout: Duration,
}

impl ShutdownCoordinator {
    /// Create a new shutdown coordinator with default 30s timeout
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(30))
    }

    /// Create with custom timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        Self {
            shutdown_tx,
            shutdown_rx,
            is_shutting_down: Arc::new(AtomicBool::new(false)),
            timeout,
        }
    }

    /// Get a receiver for shutdown signals
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }

    /// Check if shutdown is in progress
    pub fn is_shutting_down(&self) -> bool {
        self.is_shutting_down.load(Ordering::SeqCst)
    }

    /// Initiate graceful shutdown
    pub fn shutdown(&self) {
        if self.is_shutting_down.swap(true, Ordering::SeqCst) {
            // Already shutting down
            return;
        }

        info!("Initiating graceful shutdown...");
        let _ = self.shutdown_tx.send(true);
    }

    /// Wait for shutdown signal from OS
    pub async fn wait_for_signal(&self) {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};

            let mut sigterm = signal(SignalKind::terminate())
                .expect("Failed to register SIGTERM handler");
            let mut sigint = signal(SignalKind::interrupt())
                .expect("Failed to register SIGINT handler");
            let mut sigquit = signal(SignalKind::quit())
                .expect("Failed to register SIGQUIT handler");

            tokio::select! {
                _ = sigterm.recv() => {
                    info!("Received SIGTERM");
                }
                _ = sigint.recv() => {
                    info!("Received SIGINT");
                }
                _ = sigquit.recv() => {
                    info!("Received SIGQUIT");
                }
            }

            self.shutdown();
        }

        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to register ctrl-c handler");
            info!("Received Ctrl+C");
            self.shutdown();
        }
    }

    /// Get the configured timeout
    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Default for ShutdownCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for ShutdownCoordinator {
    fn clone(&self) -> Self {
        Self {
            shutdown_tx: self.shutdown_tx.clone(),
            shutdown_rx: self.shutdown_rx.clone(),
            is_shutting_down: self.is_shutting_down.clone(),
            timeout: self.timeout,
        }
    }
}

/// Manages graceful shutdown of background tasks
pub struct TaskShutdown {
    tasks: Vec<(&'static str, tokio::task::JoinHandle<()>)>,
}

impl TaskShutdown {
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Register a background task for shutdown
    pub fn register(&mut self, name: &'static str, handle: tokio::task::JoinHandle<()>) {
        self.tasks.push((name, handle));
    }

    /// Abort all registered tasks
    pub async fn abort_all(&mut self, timeout: Duration) {
        info!("Stopping {} background tasks...", self.tasks.len());

        for (name, handle) in self.tasks.drain(..) {
            handle.abort();

            match tokio::time::timeout(timeout, handle).await {
                Ok(Ok(())) => {
                    info!("Task '{}' stopped gracefully", name);
                }
                Ok(Err(e)) if e.is_cancelled() => {
                    info!("Task '{}' cancelled", name);
                }
                Ok(Err(e)) => {
                    warn!("Task '{}' failed: {}", name, e);
                }
                Err(_) => {
                    warn!("Task '{}' did not stop within timeout", name);
                }
            }
        }
    }
}

impl Default for TaskShutdown {
    fn default() -> Self {
        Self::new()
    }
}

/// Graceful server shutdown handler
pub struct GracefulShutdown {
    coordinator: ShutdownCoordinator,
}

impl GracefulShutdown {
    pub fn new(coordinator: ShutdownCoordinator) -> Self {
        Self { coordinator }
    }

    /// Run shutdown sequence for the API server
    pub async fn run<F, Fut>(&self, cleanup: F)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        // Wait for shutdown signal
        self.coordinator.wait_for_signal().await;

        info!("Starting graceful shutdown sequence...");
        let shutdown_timeout = self.coordinator.timeout();

        // Run cleanup with timeout
        match timeout(shutdown_timeout, cleanup()).await {
            Ok(()) => {
                info!("Cleanup completed successfully");
            }
            Err(_) => {
                warn!(
                    "Cleanup did not complete within {:?}, forcing exit",
                    shutdown_timeout
                );
            }
        }

        info!("Shutdown complete");
    }

    /// Create a shutdown signal for axum
    pub fn signal(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut rx = self.coordinator.subscribe();

        async move {
            // Wait for the shutdown signal
            while !*rx.borrow() {
                if rx.changed().await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Cleanup operations to run during shutdown
pub struct CleanupRunner {
    operations: Vec<Box<dyn CleanupOperation + Send + Sync>>,
}

impl CleanupRunner {
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
        }
    }

    /// Add a cleanup operation
    pub fn add<T: CleanupOperation + Send + Sync + 'static>(&mut self, op: T) {
        self.operations.push(Box::new(op));
    }

    /// Run all cleanup operations
    pub async fn run_all(&self) {
        for (i, op) in self.operations.iter().enumerate() {
            info!("Running cleanup operation {}/{}: {}", i + 1, self.operations.len(), op.name());

            if let Err(e) = op.cleanup().await {
                error!("Cleanup operation '{}' failed: {}", op.name(), e);
            }
        }
    }
}

impl Default for CleanupRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for cleanup operations during shutdown
#[async_trait::async_trait]
pub trait CleanupOperation {
    /// Name of the cleanup operation
    fn name(&self) -> &'static str;

    /// Perform cleanup
    async fn cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// Database cleanup operation
pub struct DatabaseCleanup {
    db: Arc<crate::db::Database>,
}

impl DatabaseCleanup {
    pub fn new(db: Arc<crate::db::Database>) -> Self {
        Self { db }
    }
}

#[async_trait::async_trait]
impl CleanupOperation for DatabaseCleanup {
    fn name(&self) -> &'static str {
        "database"
    }

    async fn cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Closing database connections...");
        self.db.close().await;
        Ok(())
    }
}

/// WebSocket cleanup operation
pub struct WebSocketCleanup {
    ws_state: Arc<crate::websocket::WsState>,
}

impl WebSocketCleanup {
    pub fn new(ws_state: Arc<crate::websocket::WsState>) -> Self {
        Self { ws_state }
    }
}

#[async_trait::async_trait]
impl CleanupOperation for WebSocketCleanup {
    fn name(&self) -> &'static str {
        "websocket"
    }

    async fn cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Closing WebSocket connections...");
        self.ws_state.close_all().await;
        Ok(())
    }
}

/// Monitoring cleanup operation
pub struct MonitoringCleanup {
    monitoring: Arc<crate::monitoring::MonitoringManager>,
}

impl MonitoringCleanup {
    pub fn new(monitoring: Arc<crate::monitoring::MonitoringManager>) -> Self {
        Self { monitoring }
    }
}

#[async_trait::async_trait]
impl CleanupOperation for MonitoringCleanup {
    fn name(&self) -> &'static str {
        "monitoring"
    }

    async fn cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Stopping monitoring collection...");
        self.monitoring.stop_collection().await;
        Ok(())
    }
}

/// Scheduler cleanup operation
pub struct SchedulerCleanup {
    scheduler: Arc<crate::vm::snapshot_scheduler::SnapshotScheduler>,
}

impl SchedulerCleanup {
    pub fn new(scheduler: Arc<crate::vm::snapshot_scheduler::SnapshotScheduler>) -> Self {
        Self { scheduler }
    }
}

#[async_trait::async_trait]
impl CleanupOperation for SchedulerCleanup {
    fn name(&self) -> &'static str {
        "snapshot_scheduler"
    }

    async fn cleanup(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Stopping snapshot scheduler...");
        self.scheduler.stop().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_shutdown_coordinator() {
        let coordinator = ShutdownCoordinator::new();
        let mut rx = coordinator.subscribe();

        assert!(!coordinator.is_shutting_down());

        coordinator.shutdown();

        assert!(coordinator.is_shutting_down());
        rx.changed().await.unwrap();
        assert!(*rx.borrow());
    }

    #[tokio::test]
    async fn test_task_shutdown() {
        let mut tasks = TaskShutdown::new();

        let handle = tokio::spawn(async {
            tokio::time::sleep(Duration::from_secs(100)).await;
        });

        tasks.register("test_task", handle);
        tasks.abort_all(Duration::from_millis(100)).await;

        // All tasks should be cancelled
        assert!(tasks.tasks.is_empty());
    }
}
