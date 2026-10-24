//! The API server: startup, router assembly and the HTTP handlers that
//! still live in one place (moved here from main.rs so the library and the
//! binary no longer compile every module twice).

// Every module, by the name main.rs used for it.
#[cfg(feature = "backup")]
#[allow(unused_imports)]
use crate::backup;
#[cfg(feature = "cluster")]
#[allow(unused_imports)]
use crate::ha;
#[cfg(feature = "kubernetes")]
#[allow(unused_imports)]
use crate::kubernetes;
#[cfg(feature = "lxc")]
#[allow(unused_imports)]
use crate::lxc_api;
#[cfg(feature = "nas")]
#[allow(unused_imports)]
use crate::nas;
#[allow(unused_imports)]
use crate::{
    alerts, api, audit, auth, cloudinit, cluster, config, console, container, db, encryption,
    error, features, firewall, gpu, health, logging, metrics, metrics_collector, metrics_explorer,
    middleware, migration, monitoring, notification_channels, observability, openapi, persistence,
    prometheus, sdn, secrets, shutdown, static_ui, storage, template, tls, validation, vm,
    vm_console_api, webhooks, websocket, AppState,
};

use crate::alerts::{Alert, AlertRule, NotificationChannel};
use crate::audit::AuditLogger;
use crate::cloudinit::{CloudInitConfig, CloudInitManager};
use crate::cluster::{ArchitectureSummary, Node};
use crate::console::{ConsoleInfo, ConsoleManager, ConsoleType};
use crate::firewall::{FirewallRule, FirewallScope, SecurityGroup};
use crate::monitoring::{MonitoringManager, NodeMetrics, ResourceMetrics, StorageMetrics};
use crate::observability::{OtelConfig, OtelManager};
use crate::secrets::VaultManager;
use crate::template::{CloneRequest, OsType, StorageType, Template};
use crate::tls::TlsManager;
use crate::vm::VmManager;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    middleware as axum_middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Json, Router,
};
use horcrux_common::VmConfig;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing::{error, info};

/// Start the Horcrux API server (the `horcrux-api` binary just calls this).
pub async fn run() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Load configuration
    let horcrux_config = config::HorcruxConfig::load();
    if let Err(e) = horcrux_config.validate() {
        error!("Configuration validation failed: {}", e);
        return Err(anyhow::anyhow!("Invalid configuration: {}", e));
    }
    info!("Configuration loaded successfully");
    let horcrux_config = Arc::new(horcrux_config);

    // QEMU/KVM host checks (USE=qemu only).
    #[cfg(feature = "qemu")]
    {
        // Check if KVM is available
        if vm::qemu::QemuManager::check_kvm_available() {
            info!("KVM acceleration is available");
        } else {
            error!("WARNING: KVM is not available. VMs will run without hardware acceleration!");
        }

        // Get QEMU version
        match vm::qemu::QemuManager::get_qemu_version().await {
            Ok(version) => info!("QEMU version: {}", version),
            Err(e) => error!("Failed to get QEMU version: {}", e),
        }
    }

    // Initialize database
    let database = Arc::new(
        db::Database::new(&horcrux_config.database.url)
            .await
            .expect("Failed to connect to database"),
    );

    // Run migrations
    database.migrate().await.expect("Failed to run migrations");
    info!("Database initialized");

    // Access control: built-in roles (and the one-time conversion of
    // pre-RBAC admins), token signing secret, first-run administrator.
    auth::store::ensure_builtin_roles(database.pool())
        .await
        .expect("Failed to set up roles");
    middleware::auth::init_jwt_secret(&horcrux_config.paths.data_dir)
        .expect("Failed to set up the token signing secret");
    api::auth::bootstrap_admin(&database, &horcrux_config).await;

    let monitoring_manager = Arc::new(MonitoringManager::new());

    // Start background metrics collection
    monitoring_manager.start_collection().await;
    info!("Monitoring system started");

    // Start session cleanup task
    let db_clone = database.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Every hour
        loop {
            interval.tick().await;
            match db::users::cleanup_expired_sessions(db_clone.pool()).await {
                Ok(_) => tracing::debug!("Session cleanup completed"),
                Err(e) => tracing::error!("Session cleanup failed: {}", e),
            }
        }
    });
    info!("Session cleanup task started");

    // Initialize rate limiter with custom config
    //
    // max_requests is overridable via RATE_LIMIT_MAX_REQUESTS: the default of
    // 100/user/60s is right for production, but CI's integration test suite
    // authenticates all ~22 parallel tests as the same shared admin user
    // (ADMIN_PASSWORD env var above), so per-user limiting gives that one
    // bucket a 100 req/min budget shared across the whole suite - easily
    // exhausted by legitimate concurrent test traffic, producing an
    // intermittent 429 where a test expects 200 (not a real rate-limit bug,
    // just production limits applied to a test fixture they weren't sized
    // for). CI sets this much higher; production is unaffected since the
    // var is unset there.
    let rate_limit_max_requests: u32 = std::env::var("RATE_LIMIT_MAX_REQUESTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);
    let rate_limit_config = middleware::rate_limit::RateLimitConfig {
        max_requests: rate_limit_max_requests,
        window: std::time::Duration::from_secs(60),
        per_user: true,
    };
    let rate_limiter = middleware::rate_limit::create_limiter(rate_limit_config.clone());
    middleware::rate_limit::start_cleanup_task(rate_limiter.clone());
    info!(
        "Rate limiter initialized: {} requests per {} seconds",
        rate_limit_config.max_requests,
        rate_limit_config.window.as_secs()
    );

    // Initialize strict rate limiter for auth endpoints
    // Overridable via AUTH_RATE_LIMIT_MAX_REQUESTS for the same reason as
    // RATE_LIMIT_MAX_REQUESTS above: CI's shared test server is logged into
    // by every API test binary (integration, contract, backup, console...)
    // from one IP, which exhausts a 5/min budget and fails unrelated tests
    // with 429. Production keeps 5/min (the var is unset there).
    let auth_rate_limit_max_requests: u32 = std::env::var("AUTH_RATE_LIMIT_MAX_REQUESTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let auth_rate_limit_config = middleware::rate_limit::RateLimitConfig {
        max_requests: auth_rate_limit_max_requests, // 5 login attempts per minute by default
        window: std::time::Duration::from_secs(60),
        per_user: false, // Per IP address
    };
    let auth_rate_limiter = middleware::rate_limit::create_limiter(auth_rate_limit_config.clone());
    middleware::rate_limit::start_cleanup_task(auth_rate_limiter.clone());
    info!(
        "Auth rate limiter initialized: {} requests per {} seconds",
        auth_rate_limit_config.max_requests,
        auth_rate_limit_config.window.as_secs()
    );

    // Initialize CORS config
    let cors_config = middleware::cors::CorsConfig::default();

    // Initialize GPU manager and scan for devices
    let gpu_manager = Arc::new(gpu::GpuManager::new());
    match gpu_manager.scan_devices().await {
        Ok(devices) => info!("Found {} GPU device(s)", devices.len()),
        Err(e) => tracing::warn!("Failed to scan GPU devices: {}", e),
    }

    // Initialize Prometheus metrics
    let prometheus_manager = Arc::new(prometheus::PrometheusManager::new());
    prometheus_manager.init_default_metrics().await;
    info!("Prometheus metrics initialized");

    // Initialize webhook manager
    let webhook_manager = Arc::new(
        webhooks::WebhookManager::with_database(&database)
            .await
            .expect("Failed to load webhooks"),
    );
    info!("Webhook manager initialized");

    // Initialize CNI manager: networks and attachments come back from the
    // database (their conflists are rewritten).
    let cni_manager = Arc::new(tokio::sync::RwLock::new(
        sdn::cni::CniManager::with_database(
            horcrux_config.cni.bin_dir.clone(),
            horcrux_config.cni.conf_dir.clone(),
            database.clone(),
        )
        .await
        .expect("Failed to load CNI networks"),
    ));

    // Create default CNI network if CNI directories exist
    if horcrux_config.cni.enabled && horcrux_config.cni.bin_dir.exists() {
        match cni_manager.write().await.create_default_network().await {
            Ok(_) => info!("CNI default network ready"),
            Err(e) => tracing::warn!("Failed to create default CNI network: {}", e),
        }
    } else if !horcrux_config.cni.bin_dir.exists() {
        tracing::warn!(
            "CNI binary directory not found at {:?} - CNI features disabled",
            horcrux_config.cni.bin_dir
        );
    }

    // Initialize Network Policy manager (persisted) and install the stored
    // policies in nftables (the kernel ruleset doesn't survive a reboot).
    let network_policy_manager = Arc::new(tokio::sync::RwLock::new(
        sdn::policy::NetworkPolicyManager::with_database(database.clone())
            .await
            .expect("Failed to load network policies"),
    ));
    api::sdn_cni::enforce(&network_policy_manager, &cni_manager).await;
    info!("Network policy manager initialized");

    // Initialize VM Snapshot manager
    let mut snapshot_manager = vm::snapshot::VmSnapshotManager::with_qmp_socket_pattern(
        horcrux_config.paths.snapshots.to_string_lossy().to_string(),
        horcrux_config.qemu.qmp_socket_pattern.clone(),
    );
    snapshot_manager.load_snapshots().await?;
    let snapshot_manager = Arc::new(tokio::sync::RwLock::new(snapshot_manager));
    info!("VM Snapshot manager initialized");

    // Initialize Snapshot Scheduler (schedules persisted in the database)
    let snapshot_scheduler = Arc::new(
        vm::snapshot_scheduler::SnapshotScheduler::with_database(
            snapshot_manager.clone(),
            database.clone(),
        )
        .await
        .expect("Failed to load snapshot schedules"),
    );
    info!("Snapshot scheduler initialized");

    // Initialize Snapshot Quota manager
    let snapshot_quota_manager = Arc::new(vm::snapshot_quota::SnapshotQuotaManager::new());
    info!("Snapshot quota manager initialized");

    // Initialize VM Clone manager
    let clone_manager = Arc::new(vm::clone::VmCloneManager::new(
        horcrux_config
            .paths
            .vm_storage
            .to_string_lossy()
            .to_string(),
    ));
    info!("VM Clone manager initialized");

    // Initialize Clone Job manager
    let clone_job_manager = Arc::new(vm::clone_progress::CloneJobManager::new());
    info!("Clone Job manager initialized");

    // Initialize Replication manager (jobs persisted in the database)
    let replication_manager = Arc::new(
        vm::replication::ReplicationManager::with_database(database.clone())
            .await
            .expect("Failed to load replication jobs"),
    );
    info!("Replication manager initialized");

    // Configuration managers backed by the database (config_objects table):
    // each loads its persisted objects here and writes every change through,
    // so storage pools, firewall rules, HA, templates, alert rules, cluster
    // nodes and backup jobs/catalog survive a restart.
    let persisted = persistence::load_persisted_managers(&database, &horcrux_config).await;

    // Initialize Kubernetes manager (if feature enabled)
    #[cfg(feature = "kubernetes")]
    let kubernetes_manager = {
        let mgr = Arc::new(
            kubernetes::KubernetesManager::with_encrypted_store(
                database.clone(),
                Arc::new(VaultManager::new()),
                &horcrux_config.paths.data_dir.join("secrets/kubeconfig.key"),
            )
            .await?,
        );
        if let Err(e) = mgr.initialize().await {
            tracing::warn!("Failed to initialize Kubernetes manager: {}", e);
        } else {
            info!("Kubernetes manager initialized");
            mgr.reconnect_all_in_background();
        }
        mgr
    };

    // HTTPS certificate: generated on first start, replaceable at runtime.
    let tls_manager = Arc::new(TlsManager::with_paths(
        horcrux_config.tls.cert_path.clone(),
        horcrux_config.tls.key_path.clone(),
        horcrux_config.paths.data_dir.join("ssl"),
    ));
    tls_manager.set_http_redirect_port(horcrux_config.server.http_redirect_port);
    if horcrux_config.server.tls_enabled {
        let info = tls_manager
            .ensure_certificate()
            .expect("Failed to load or create the TLS certificate");
        info!(
            "TLS certificate: {} ({}), valid until {}, SHA-256 {}",
            info.subject, info.source, info.valid_until, info.fingerprint_sha256
        );
        tls::acme::start_renewal_task(tls_manager.clone(), database.pool().clone());
    }

    let database_for_stacks = database.clone();
    let shared_container_manager = Arc::new(container::ContainerManager::with_database(
        database_for_stacks.clone(),
    ));
    let state = Arc::new(AppState {
        config: horcrux_config.clone(),
        vm_manager: Arc::new(VmManager::with_database_and_storage_path(
            database.clone(),
            horcrux_config.paths.vm_storage.clone(),
        )),
        container_manager: shared_container_manager.clone(),
        #[cfg(feature = "backup")]
        backup_manager: persisted.backup,
        cloudinit_manager: Arc::new(CloudInitManager::new(
            horcrux_config.paths.cloudinit.clone(),
        )),
        template_manager: persisted.template,
        firewall_manager: persisted.firewall,
        monitoring_manager,
        console_manager: Arc::new(ConsoleManager::new()),
        cluster_manager: persisted.cluster,
        alert_manager: persisted.alert,
        otel_manager: Arc::new(OtelManager::new()),
        tls_manager: tls_manager.clone(),
        vault_manager: Arc::new(VaultManager::new()),
        audit_logger: Arc::new(AuditLogger::new(Some(
            horcrux_config.logging.audit_log.clone(),
        ))),
        database,
        _rate_limiter: rate_limiter.clone(),
        storage_manager: persisted.storage,
        #[cfg(feature = "cluster")]
        ha_manager: persisted.ha,
        migration_manager: Arc::new(migration::MigrationManager::new()),
        gpu_manager,
        prometheus_manager,
        webhook_manager,
        cni_manager,
        network_policy_manager,
        snapshot_manager: snapshot_manager.clone(),
        snapshot_scheduler: snapshot_scheduler.clone(),
        snapshot_quota_manager: snapshot_quota_manager.clone(),
        clone_manager,
        clone_job_manager,
        replication_manager,
        ws_state: Arc::new(websocket::WsState::new()),
        #[cfg(any(feature = "docker", feature = "podman"))]
        image_manager: Arc::new(container::image::ImageManager::new()),
        #[cfg(any(feature = "docker", feature = "podman"))]
        volume_manager: Arc::new(container::volume::VolumeManager::new()),
        #[cfg(any(feature = "docker", feature = "podman"))]
        container_network_manager: Arc::new(container::cnetwork::ContainerNetworkManager::new()),
        #[cfg(any(feature = "docker", feature = "podman"))]
        stack_manager: Arc::new(container::stack::StackManager::new(
            shared_container_manager,
            database_for_stacks,
        )),
        #[cfg(any(
            feature = "docker",
            feature = "podman",
            feature = "lxc",
            feature = "lxd",
            feature = "incus"
        ))]
        container_ws_manager: Arc::new(container::ws::ContainerWsManager::new()),
        #[cfg(feature = "kubernetes")]
        kubernetes_manager,
    });

    // Start snapshot scheduler background task
    let state_for_scheduler = state.clone();
    let vm_getter = Arc::new(move |vm_id: &str| {
        let state = state_for_scheduler.clone();
        let vm_id = vm_id.to_string();
        Box::pin(async move { state.database.get_vm(&vm_id).await.ok() })
            as futures::future::BoxFuture<'static, Option<VmConfig>>
    });
    snapshot_scheduler.start_scheduler(vm_getter);
    info!("Snapshot scheduler background task started");

    // Initialize libvirt manager for VM metrics collection (optional)
    #[cfg(feature = "qemu")]
    let libvirt_manager = {
        let mgr = Arc::new(metrics::LibvirtManager::new());
        match mgr.connect(None).await {
            Ok(_) => {
                info!("Connected to libvirt (qemu:///system) for VM metrics");
                Some(mgr)
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to connect to libvirt: {} - VM metrics will use fallback",
                    e
                );
                None
            }
        }
    };
    #[cfg(not(feature = "qemu"))]
    let libvirt_manager = None;

    // Cluster node heartbeat: keeps every registered node's live CPU/memory
    // usage and online/offline status current.
    // USE=cluster only: without it there are no cluster routes to
    // register nodes with.
    #[cfg(feature = "cluster")]
    state
        .cluster_manager
        .start_usage_monitor(horcrux_config.server.port);
    // HA failure detection, fencing and recovery.
    #[cfg(feature = "cluster")]
    api::ha_loop::spawn(state.clone());
    // Rebalance when a node joins or returns from maintenance (leader only).
    #[cfg(feature = "cluster")]
    api::drain::spawn(state.clone());

    // Save/resume of guests across host shutdown: token, boot pass, running snapshot.
    api::guest_state::engine::spawn(state.clone());

    // OpenTelemetry export (saved settings + periodic export).
    start_otel_exporter(state.clone());

    // VMs half-received from a migration whose source went quiet.
    api::migration_peer::spawn_reaper(state.clone());

    // Running Docker/Podman containers with firewall rules: give their
    // host-side veths the stable names the rules match (the runtime gave
    // them random ones when it started them, e.g. at boot).
    {
        let st = state.clone();
        tokio::spawn(async move {
            // Let the container reconcilers load the runtimes' containers.
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let _ = st.container_manager.list_containers().await;
            for id in st.firewall_manager.containers_with_rules().await {
                if let Err(e) = st.container_manager.name_host_veths(&id).await {
                    tracing::warn!("container {id}: host-side interfaces not renamed: {e}");
                }
            }
        });
    }

    // Start metrics collection background task
    metrics_collector::start_metrics_collector(
        state.ws_state.clone(),
        state.monitoring_manager.clone(),
        state.vm_manager.clone(),
        libvirt_manager,
    );

    // Static files serving for the frontend
    // (see static_ui.rs for why this is no longer a fixed relative path)
    let (ui_dir, ui_found) = static_ui::ui_dir();
    if ui_found {
        info!("Serving web UI from {}", ui_dir.display());
    } else {
        tracing::warn!(
            "Web UI not found (no index.html in {} or any of {:?}); the API still works but UI requests will 404. Set {} to the trunk dist/ directory.",
            ui_dir.display(),
            static_ui::UI_DIR_CANDIDATES,
            static_ui::UI_DIR_ENV
        );
    }
    // Real files, the SPA shell with 200 for client-side routes (deep links
    // like /vms used to come back 404), real 404s for missing assets.
    let serve_dir = static_ui::ui_service(&ui_dir);

    // Public auth routes: the strict per-IP limit applies only to the
    // credential-submitting POSTs (login, tfa, register); the login page's
    // reads and the OpenID redirect/callback get the general limit only.
    let auth_router = api::auth::credential_routes()
        .with_state(state.clone())
        .layer(axum_middleware::from_fn(move |conn_info, req, next| {
            middleware::rate_limit::rate_limit_middleware(
                auth_rate_limiter.clone(),
                conn_info,
                req,
                next,
            )
        }))
        .merge(api::auth::public_read_routes().with_state(state.clone()));

    // Build protected routes using modular route builders
    let protected_routes = Router::new()
        .merge(vm_routes())
        .merge(vm_console_api::routes())
        .merge(api::vm_import::routes(&state))
        .merge(container_routes())
        .merge(template_routes())
        .merge(auth_protected_routes())
        .merge(api::access_routes())
        .merge(storage_routes())
        .merge(monitoring_routes())
        .merge(network_routes())
        .merge(api::lane_m::routes(&state).await?)
        .merge(api::migration_jobs::routes())
        .merge(api::migration_peer::routes())
        .merge(api::node_summary::routes())
        .merge(api::guest_state::routes())
        .merge(api::node_disks::routes())
        .merge(api::system_admin::routes(&state))
        .merge(api::tasks::routes())
        .merge(api::ceph_admin::routes())
        .merge(api::audit_view::routes())
        .merge(api::node_shell::routes())
        .merge(api::sdn_zones::routes(&state))
        .merge(api::vm_hw::routes())
        .merge(api::vm_devices::routes())
        .merge(api::storage_volumes::routes())
        .merge(api::storage_content::routes())
        .merge(api::resource_pools::routes())
        .merge(api::inventory::routes())
        .merge(api::firewall_objects::routes())
        .merge(api::sdn_cni::routes())
        .merge(api::host_net::routes(&state))
        .merge(api::dashboards::routes(&state));
    #[cfg(feature = "s3-gateway")]
    let protected_routes = protected_routes.merge(api::s3_gateway::routes(&state));

    // USE-flag gated route groups (see src/features.rs): a feature that was
    // not compiled in has no routes, so its endpoints answer 404.
    #[cfg(any(feature = "docker", feature = "podman"))]
    let protected_routes = protected_routes.merge(oci_routes());
    #[cfg(any(
        feature = "docker",
        feature = "podman",
        feature = "lxc",
        feature = "lxd",
        feature = "incus"
    ))]
    let protected_routes = protected_routes.merge(container_ws_routes());
    #[cfg(feature = "backup")]
    let protected_routes = protected_routes.merge(api::backup::routes(&state));
    #[cfg(feature = "cluster")]
    let protected_routes = protected_routes
        .merge(cluster_routes())
        .merge(api::ha_loop::routes())
        .merge(api::drain::routes())
        .merge(api::ct_move::routes())
        .merge(api::cluster_join::routes());

    // LXC parity routes (templates, shutdown/reboot, pct set, resize, snapshots)
    #[cfg(feature = "lxc")]
    let protected_routes = protected_routes.merge(lxc_api::lxc_routes());

    // Add Kubernetes routes if feature is enabled
    #[cfg(feature = "kubernetes")]
    let protected_routes = protected_routes.merge(kubernetes_routes());

    // Add NAS routes if feature is enabled
    #[cfg(feature = "nas")]
    let protected_routes = protected_routes.merge(nas_routes());

    let protected_routes = protected_routes
        // WebSocket endpoint for real-time updates
        .route("/api/ws", get(websocket::ws_handler::<AppState>))
        .with_state(state.clone())
        // Every protected route: permission check (inner) after
        // authentication (outer layer, runs first).
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::rbac::rbac_middleware::<AppState>,
        ))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::auth::auth_middleware::<AppState>,
        ));

    // Build main app with public and protected routes
    let app = Router::new()
        .route("/api/health", get(health_check))
        .route("/api/health/detailed", get(health_detailed))
        .route("/api/health/live", get(liveness_probe))
        .route("/api/health/ready", get(readiness_probe))
        .with_state(state.clone())
        // Compiled-in features (USE flags), read by the UI to hide the rest
        .merge(features::routes())
        // Host shutdown/boot hook: token-authenticated, loopback tooling.
        .merge(api::guest_state::agent::routes().with_state(state.clone()))
        // Merge OpenAPI / Swagger UI routes (public, for documentation)
        .merge(openapi::openapi_routes())
        // Merge protected routes (with auth middleware)
        .merge(protected_routes)
        // Merge auth router with strict rate limiting (public routes)
        .merge(auth_router)
        // Add middleware layers (in reverse order of execution)
        .layer(axum_middleware::from_fn(move |req, next| {
            middleware::cors::cors_middleware(cors_config.clone(), req, next)
        }))
        .layer(axum_middleware::from_fn(move |conn_info, req, next| {
            middleware::rate_limit::rate_limit_middleware(
                rate_limiter.clone(),
                conn_info,
                req,
                next,
            )
        }))
        // Prometheus: count every request by route pattern and status
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            api::prometheus_export::count_http_requests,
        ))
        // Note: Auth middleware will be added per-route or per-group later
        // Serve static files (frontend) - must be last to act as fallback
        .fallback_service(serve_dir);

    // Set up graceful shutdown
    let shutdown_coordinator = shutdown::ShutdownCoordinator::new();
    let graceful = shutdown::GracefulShutdown::new(shutdown_coordinator.clone());

    // Clone managers for cleanup from state
    let db_for_cleanup = state.database.clone();
    let ws_for_cleanup = state.ws_state.clone();
    let monitoring_for_cleanup = state.monitoring_manager.clone();
    let scheduler_for_cleanup = state.snapshot_scheduler.clone();

    // Start server: HTTPS by default (plain HTTP on the same port is
    // redirected), plain HTTP only when TLS is turned off.
    let addr = format!(
        "{}:{}",
        horcrux_config.server.host, horcrux_config.server.port
    );
    let listener = TcpListener::bind(&addr).await?;
    let redirect_listener = match horcrux_config.server.http_redirect_port {
        Some(port) if horcrux_config.server.tls_enabled => {
            let raddr = format!("{}:{}", horcrux_config.server.host, port);
            info!("HTTP redirect / ACME challenge listener on {}", raddr);
            Some(TcpListener::bind(&raddr).await?)
        }
        _ => None,
    };
    info!(
        "Horcrux API listening on {}://{}",
        if horcrux_config.server.tls_enabled {
            "https"
        } else {
            "http"
        },
        addr
    );

    let shutdown_signal = graceful.signal();
    tls::server::serve(
        listener,
        redirect_listener,
        app,
        horcrux_config
            .server
            .tls_enabled
            .then(|| state.tls_manager.clone()),
        shutdown_signal,
    )
    .await?;

    // Run cleanup after server stops
    info!("Server stopped, running cleanup...");

    // Stop scheduler
    scheduler_for_cleanup.stop().await;

    // Stop monitoring
    monitoring_for_cleanup.stop_collection().await;

    // Close WebSocket connections
    ws_for_cleanup.close_all().await;

    // Close database
    db_for_cleanup.close().await;

    info!("Cleanup complete, exiting");

    Ok(())
}

// =============================================================================
// Route Builder Functions
// =============================================================================

/// Build VM-related routes (VMs, snapshots, clones, replication)
fn vm_routes() -> Router<Arc<AppState>> {
    Router::new()
        // VM CRUD endpoints
        .route("/api/vms", get(list_vms))
        .route("/api/vms", post(create_vm))
        .route("/api/vms/:id", get(get_vm))
        .route("/api/vms/:id/start", post(start_vm))
        .route("/api/vms/:id/stop", post(stop_vm))
        .route("/api/vms/:id", delete(delete_vm))
        // VM Snapshot endpoints
        .route("/api/vms/:id/snapshots", get(list_vm_snapshots))
        .route("/api/vms/:id/snapshots", post(create_vm_snapshot))
        .route("/api/vms/:id/snapshots/:snapshot_id", get(get_vm_snapshot))
        .route(
            "/api/vms/:id/snapshots/:snapshot_id",
            delete(delete_vm_snapshot),
        )
        .route(
            "/api/vms/:id/snapshots/:snapshot_id/restore",
            post(restore_vm_snapshot),
        )
        .route("/api/vms/:id/snapshots/tree", get(get_vm_snapshot_tree))
        // Snapshot Schedule endpoints
        .route("/api/snapshot-schedules", get(list_snapshot_schedules))
        .route("/api/snapshot-schedules", post(create_snapshot_schedule))
        .route("/api/snapshot-schedules/:id", get(get_snapshot_schedule))
        .route("/api/snapshot-schedules/:id", put(update_snapshot_schedule))
        .route(
            "/api/snapshot-schedules/:id",
            delete(delete_snapshot_schedule),
        )
        // VM Clone endpoints
        .route("/api/vms/:id/clone", post(clone_vm))
        .route("/api/vms/:id/clone-cross-node", post(clone_vm_cross_node))
        // Clone Job Progress endpoints
        .route("/api/clone-jobs", get(list_clone_jobs))
        .route("/api/clone-jobs/:id", get(get_clone_job))
        .route("/api/clone-jobs/:id/cancel", post(cancel_clone_job))
        .route("/api/clone-jobs/:id", delete(delete_clone_job))
        // Snapshot Quota endpoints
        .route("/api/snapshot-quotas", get(list_snapshot_quotas))
        .route("/api/snapshot-quotas", post(create_snapshot_quota))
        .route("/api/snapshot-quotas/:id", get(get_snapshot_quota))
        .route("/api/snapshot-quotas/:id", put(update_snapshot_quota))
        .route("/api/snapshot-quotas/:id", delete(delete_snapshot_quota))
        .route(
            "/api/snapshot-quotas/:id/usage",
            get(get_snapshot_quota_usage),
        )
        .route(
            "/api/snapshot-quotas/summary",
            get(get_snapshot_quota_summary),
        )
        .route(
            "/api/snapshot-quotas/:id/enforce",
            post(enforce_snapshot_quota),
        )
        // Replication endpoints
        .route("/api/replication/jobs", get(list_replication_jobs))
        .route("/api/replication/jobs", post(create_replication_job))
        .route("/api/replication/jobs/:id", get(get_replication_job))
        .route("/api/replication/jobs/:id", delete(delete_replication_job))
        .route(
            "/api/replication/jobs/:id/execute",
            post(execute_replication_job),
        )
        .route(
            "/api/replication/jobs/:id/status",
            get(get_replication_status),
        )
        // Console access endpoints
        .route("/api/console/:vm_id/vnc", post(create_vnc_console))
        .route("/api/console/:vm_id/websocket", get(get_vnc_websocket))
        .route("/api/console/:vm_id/novnc", get(get_novnc_page))
        .route("/api/console/:vm_id/serial", post(create_serial_console))
        .route("/api/console/ticket/:ticket_id", get(verify_console_ticket))
        .route("/api/console/ws/:ticket_id", get(vnc_websocket_handler))
        .route(
            "/api/console/ws/serial/:ticket_id",
            get(serial_websocket_handler),
        )
        // Migration endpoints
        .route("/api/migrate/:vm_id", post(migrate_vm))
        .route("/api/migrate/:vm_id/status", get(get_migration_status))
}

/// Build container-related routes
fn container_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/containers", get(list_containers))
        .route("/api/containers", post(create_container))
        .route("/api/containers/:id", get(get_container))
        .route("/api/containers/:id", delete(delete_container))
        .route("/api/containers/:id/start", post(start_container))
        .route("/api/containers/:id/stop", post(stop_container))
        .route("/api/containers/:id/pause", post(pause_container))
        .route("/api/containers/:id/resume", post(resume_container))
        .route("/api/containers/:id/status", get(get_container_status))
        .route("/api/containers/:id/exec", post(exec_container_command))
        .route("/api/containers/:id/clone", post(clone_container))
}

/// Image management routes (Docker + Podman) - see container/image.rs.
/// Runtime is selected via `?runtime=docker|podman` query param, defaulting
/// to docker, consistent with Portainer's per-environment image view.
#[cfg(any(feature = "docker", feature = "podman"))]
fn image_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/images", get(list_images))
        .route("/api/images/pull", post(pull_image))
        .route("/api/images/:id", get(inspect_image))
        .route("/api/images/:id", delete(remove_image))
        .route("/api/images/:id/tag", post(tag_image))
}

/// Volume management routes - see container/volume.rs.
#[cfg(any(feature = "docker", feature = "podman"))]
fn volume_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/volumes", get(list_volumes))
        .route("/api/volumes", post(create_container_volume))
        .route("/api/volumes/:name", get(inspect_volume))
        .route("/api/volumes/:name", delete(remove_volume))
}

/// Container-runtime network routes (docker/podman bridge networks - not
/// Horcrux's own SDN/CNI, see container/cnetwork.rs).
#[cfg(any(feature = "docker", feature = "podman"))]
fn container_network_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/container-networks", get(list_container_networks))
        .route("/api/container-networks", post(create_container_network))
        .route(
            "/api/container-networks/:id",
            get(inspect_container_network),
        )
        .route(
            "/api/container-networks/:id",
            delete(remove_container_network),
        )
        .route(
            "/api/container-networks/:id/connect",
            post(connect_container_network),
        )
        .route(
            "/api/container-networks/:id/disconnect",
            post(disconnect_container_network),
        )
}

/// Compose/stack deployment routes - see container/stack.rs.
#[cfg(any(feature = "docker", feature = "podman"))]
fn stack_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/stacks", get(list_stacks))
        .route("/api/stacks", post(deploy_stack))
        .route("/api/stacks/:id", get(get_stack))
        .route("/api/stacks/:id", delete(remove_stack))
        .route("/api/stacks/:id/start", post(start_stack))
        .route("/api/stacks/:id/stop", post(stop_stack))
}

/// Ticket-issuing routes for the container WebSocket endpoints (logs,
/// interactive exec, events feed). The actual WebSocket upgrade endpoints
/// are registered directly on the app router (public, ticket-authenticated
/// - see console's `/api/console/*/ws/:ticket` for the exact same pattern).
#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
fn container_ws_ticket_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/containers/:id/logs-ticket",
            post(create_container_logs_ticket),
        )
        .route(
            "/api/containers/:id/exec-ticket",
            post(create_container_exec_ticket),
        )
        .route(
            "/api/containers/events-ticket",
            post(create_container_events_ticket),
        )
        .route(
            "/api/containers/:id/console-ticket",
            post(create_container_console_ticket),
        )
}

/// Docker/Podman-only route groups (images, volumes, runtime networks,
/// stacks), behind the gate that refuses a `runtime` this build was
/// compiled without.
#[cfg(any(feature = "docker", feature = "podman"))]
fn oci_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(image_routes())
        .merge(volume_routes())
        .merge(container_network_routes())
        .merge(stack_routes())
        .layer(axum_middleware::from_fn(features::oci_runtime_gate))
}

/// Container log/exec/console WebSockets and their tickets: Docker/Podman
/// and (lane J) LXC consoles, so they must exist in an lxc-only build too.
#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
fn container_ws_routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(container_ws_ticket_routes())
        // Container logs/exec/events WebSocket endpoints (ticket-authenticated,
        // same pattern as the VNC/serial console WebSockets).
        .route(
            "/api/containers/ws/logs/:ticket_id",
            get(container_logs_websocket_handler),
        )
        .route(
            "/api/containers/ws/exec/:ticket_id",
            get(container_exec_websocket_handler),
        )
        .route(
            "/api/containers/ws/events/:ticket_id",
            get(container_events_websocket_handler),
        )
        .route(
            "/api/containers/ws/console/:ticket_id",
            get(container_console_websocket_handler),
        )
        .layer(axum_middleware::from_fn(features::oci_runtime_gate))
}

/// Build template and cloud-init routes (backup routes live in
/// src/api/backup.rs, USE=backup).
fn template_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Cloud-init endpoints
        .route("/api/cloudinit/:vm_id", post(generate_cloudinit))
        .route("/api/cloudinit/:vm_id", delete(delete_cloudinit))
        // Template endpoints
        .route("/api/templates", get(list_templates))
        .route("/api/templates", post(create_template))
        .route("/api/templates/:id", get(get_template))
        .route("/api/templates/:id", delete(delete_template))
        .route("/api/templates/:id/clone", post(clone_template))
}

/// Audit log routes (users, roles, sessions and the rest of access control
/// live in `api::auth`).
fn auth_protected_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Audit log endpoints
        .route("/api/audit/events", get(query_audit_events))
        .route("/api/audit/stats", get(get_audit_stats))
        .route("/api/audit/security-events", get(get_security_events))
        .route("/api/audit/failed-logins", get(get_failed_logins))
        .route("/api/audit/brute-force", get(detect_brute_force_attempts))
}

/// Build storage-related routes (pools, GPU, TLS, Vault)
fn storage_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Storage pool endpoints
        .route("/api/storage/pools", get(list_storage_pools))
        .route("/api/storage/pools/:id", get(get_storage_pool))
        .route("/api/storage/pools", post(add_storage_pool))
        .route("/api/storage/pools/:id", delete(remove_storage_pool))
        // GPU passthrough endpoints
        .route("/api/gpu/devices", get(list_gpu_devices))
        .route("/api/gpu/devices/scan", post(scan_gpu_devices))
        .route("/api/gpu/devices/:pci_address", get(get_gpu_device))
        .route(
            "/api/gpu/devices/:pci_address/bind-vfio",
            post(bind_gpu_to_vfio),
        )
        .route(
            "/api/gpu/devices/:pci_address/unbind-vfio",
            post(unbind_gpu_from_vfio),
        )
        .route(
            "/api/gpu/devices/:pci_address/iommu-group",
            get(get_gpu_iommu_group),
        )
        .route("/api/gpu/iommu-status", get(check_iommu_status))
        // Vault endpoints
        .route("/api/vault/config", get(get_vault_config))
        .route("/api/vault/config", post(update_vault_config))
        .route("/api/vault/secret/:path", get(read_vault_secret))
        .route("/api/vault/secret/:path", post(write_vault_secret))
        .route("/api/vault/secret/:path", delete(delete_vault_secret))
        .route("/api/vault/secrets/:path", get(list_vault_secrets))
}

/// Build monitoring-related routes (metrics, alerts, webhooks, observability)
fn monitoring_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Monitoring endpoints
        .route("/api/monitoring/node", get(get_node_stats))
        .route("/api/monitoring/vms", get(get_all_vm_stats))
        .route("/api/monitoring/vms/:id", get(get_vm_stats))
        .route("/api/monitoring/containers", get(get_all_container_stats))
        .route("/api/monitoring/containers/:id", get(get_container_stats))
        .route("/api/monitoring/storage", get(get_all_storage_stats))
        .route("/api/monitoring/storage/:name", get(get_storage_stats))
        .route("/api/monitoring/history/:metric", get(get_metric_history))
        // Alert system endpoints
        .route("/api/alerts/rules", get(list_alert_rules))
        .route("/api/alerts/rules", post(create_alert_rule))
        .route("/api/alerts/rules/:rule_id", delete(delete_alert_rule))
        .route("/api/alerts/active", get(list_active_alerts))
        .route("/api/alerts/history", get(get_alert_history))
        .route(
            "/api/alerts/:rule_id/:target/acknowledge",
            post(acknowledge_alert),
        )
        .route("/api/alerts/notifications", get(list_notification_channels))
        .route("/api/alerts/notifications", post(add_notification_channel))
        // OpenTelemetry endpoints
        .route("/api/observability/config", get(get_otel_config))
        .route("/api/observability/config", post(update_otel_config))
        .route("/api/observability/status", get(get_otel_status))
        .route(
            "/api/observability/export/metrics",
            post(export_metrics_now),
        )
        // Prometheus metrics endpoint
        .route("/metrics", get(prometheus_metrics))
        // Webhook endpoints
        .route("/api/webhooks", get(list_webhooks))
        .route("/api/webhooks", post(create_webhook))
        .route("/api/webhooks/:id", get(get_webhook))
        .route("/api/webhooks/:id", post(update_webhook))
        .route("/api/webhooks/:id", delete(delete_webhook))
        .route("/api/webhooks/:id/test", post(test_webhook))
        .route("/api/webhooks/:id/deliveries", get(get_webhook_deliveries))
}

/// Build cluster-related routes (nodes, HA) (USE=cluster)
#[cfg(feature = "cluster")]
fn cluster_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Cluster management
        .route("/api/cluster/status", get(get_cluster_status))
        .route("/api/cluster/local-usage", get(get_local_node_usage))
        .route("/api/cluster/nodes", get(list_cluster_nodes))
        .route(
            "/api/cluster/nodes/:name",
            post(add_cluster_node).delete(remove_cluster_node),
        )
        .route(
            "/api/cluster/nodes/:name/priority",
            axum::routing::patch(set_node_priority),
        )
        .route("/api/cluster/architecture", get(get_cluster_architecture))
        .route("/api/cluster/find-node", post(find_best_node_for_vm))
        // HA (High Availability) endpoints
        .route("/api/ha/resources", get(list_ha_resources))
        .route("/api/ha/resources", post(add_ha_resource))
        .route("/api/ha/resources/:vm_id", delete(remove_ha_resource))
        .route("/api/ha/status", get(get_ha_status))
        .route("/api/ha/groups", post(create_ha_group))
        .route("/api/ha/groups", get(list_ha_groups))
        .route("/api/ha/groups/:name", put(update_ha_group))
        .route("/api/ha/groups/:name", delete(delete_ha_group))
        .route(
            "/api/ha/groups/:name/resources",
            post(assign_ha_group_resource),
        )
}

/// Build firewall routes (CNI networks and network policies: api/sdn_cni.rs)
fn network_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Firewall endpoints
        .route("/api/firewall/rules", get(list_firewall_rules))
        .route("/api/firewall/rules", post(add_firewall_rule))
        .route("/api/firewall/rules/:id", delete(delete_firewall_rule))
        .route("/api/firewall/security-groups", get(list_security_groups))
        .route(
            "/api/firewall/security-groups/:name",
            get(get_security_group),
        )
        .route("/api/firewall/:scope/apply", post(apply_firewall_rules))
}

/// Build Kubernetes-related routes (conditionally compiled)
#[cfg(feature = "kubernetes")]
fn kubernetes_routes() -> Router<Arc<AppState>> {
    Router::new()
        // Kubernetes cluster management
        .route("/api/k8s/clusters", get(k8s_list_clusters))
        .route("/api/k8s/clusters", post(k8s_connect_cluster))
        .route("/api/k8s/clusters/:cluster_id", get(k8s_get_cluster))
        .route("/api/k8s/clusters/:cluster_id", delete(k8s_delete_cluster))
        .route(
            "/api/k8s/clusters/:cluster_id/reconnect",
            post(k8s_reconnect_cluster),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/health",
            get(k8s_get_cluster_health),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/version",
            get(k8s_get_cluster_version),
        )
        // Kubernetes namespaces
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces",
            get(k8s_list_namespaces),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces",
            post(k8s_create_namespace),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace",
            get(k8s_get_namespace),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace",
            delete(k8s_delete_namespace),
        )
        // Kubernetes nodes
        .route("/api/k8s/clusters/:cluster_id/nodes", get(k8s_list_nodes))
        .route(
            "/api/k8s/clusters/:cluster_id/nodes/:node",
            get(k8s_get_node),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/nodes/:node/cordon",
            post(k8s_cordon_node),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/nodes/:node/uncordon",
            post(k8s_uncordon_node),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/nodes/:node/drain",
            post(k8s_drain_node),
        )
        // Kubernetes pods
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods",
            get(k8s_list_pods),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:pod",
            get(k8s_get_pod),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:pod",
            delete(k8s_delete_pod),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pods/:pod/logs",
            get(k8s_get_pod_logs),
        )
        // Kubernetes deployments
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments",
            get(k8s_list_deployments),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:deployment",
            get(k8s_get_deployment),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:deployment/scale",
            post(k8s_scale_deployment),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/deployments/:deployment/restart",
            post(k8s_restart_deployment),
        )
        // Kubernetes events
        .route("/api/k8s/clusters/:cluster_id/events", get(k8s_list_events))
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/events",
            get(k8s_list_namespace_events),
        )
        // Kubernetes services
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/services",
            get(k8s_list_services),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/services",
            post(k8s_create_service),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/services/:service",
            get(k8s_get_service),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/services/:service",
            delete(k8s_delete_service),
        )
        // Kubernetes ingresses
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses",
            get(k8s_list_ingresses),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses",
            post(k8s_create_ingress),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses/:ingress",
            get(k8s_get_ingress),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/ingresses/:ingress",
            delete(k8s_delete_ingress),
        )
        // Kubernetes configmaps
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps",
            get(k8s_list_configmaps),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps",
            post(k8s_create_configmap),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps/:name",
            get(k8s_get_configmap),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/configmaps/:name",
            delete(k8s_delete_configmap),
        )
        // Kubernetes secrets
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets",
            get(k8s_list_secrets),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets",
            post(k8s_create_secret),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets/:name",
            get(k8s_get_secret),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/secrets/:name",
            delete(k8s_delete_secret),
        )
        // Kubernetes PVCs
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs",
            get(k8s_list_pvcs),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs",
            post(k8s_create_pvc),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs/:name",
            get(k8s_get_pvc),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/pvcs/:name",
            delete(k8s_delete_pvc),
        )
        // Kubernetes statefulsets
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets",
            get(k8s_list_statefulsets),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name",
            get(k8s_get_statefulset),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name/scale",
            post(k8s_scale_statefulset),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/statefulsets/:name",
            delete(k8s_delete_statefulset),
        )
        // Kubernetes daemonsets
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets",
            get(k8s_list_daemonsets),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets/:name",
            get(k8s_get_daemonset),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/daemonsets/:name",
            delete(k8s_delete_daemonset),
        )
        // Kubernetes jobs
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs",
            get(k8s_list_jobs),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs/:name",
            get(k8s_get_job),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/jobs/:name",
            delete(k8s_delete_job),
        )
        // Kubernetes cronjobs
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs",
            get(k8s_list_cronjobs),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs/:name",
            get(k8s_get_cronjob),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/cronjobs/:name",
            delete(k8s_delete_cronjob),
        )
        // Kubernetes metrics
        .route(
            "/api/k8s/clusters/:cluster_id/metrics/nodes",
            get(k8s_get_node_metrics),
        )
        .route(
            "/api/k8s/clusters/:cluster_id/namespaces/:namespace/metrics/pods",
            get(k8s_get_pod_metrics),
        )
    // Helm repositories/charts/releases: api::helm (merged in main()).
}

/// Build NAS-related routes (conditionally compiled)
#[cfg(feature = "nas")]
fn nas_routes() -> Router<Arc<AppState>> {
    Router::new()
        // NAS Health and Status
        .route("/api/nas/health", get(nas_health))
        .route("/api/nas/metrics", get(nas_metrics))
        // NAS Shares
        .route("/api/nas/shares", get(nas_list_shares))
        .route("/api/nas/shares", post(nas_create_share))
        .route("/api/nas/shares/:share_id", get(nas_get_share))
        .route("/api/nas/shares/:share_id", put(nas_update_share))
        .route("/api/nas/shares/:share_id", delete(nas_delete_share))
        .route("/api/nas/shares/:share_id/enable", post(nas_enable_share))
        .route("/api/nas/shares/:share_id/disable", post(nas_disable_share))
        // NAS Users
        .route("/api/nas/users", get(nas_list_users))
        .route("/api/nas/users", post(nas_create_user))
        .route("/api/nas/users/:user_id", get(nas_get_user))
        .route("/api/nas/users/:user_id", put(nas_update_user))
        .route("/api/nas/users/:user_id", delete(nas_delete_user))
        .route(
            "/api/nas/users/:user_id/password",
            post(nas_set_user_password),
        )
        .route("/api/nas/users/:user_id/enable", post(nas_enable_user))
        .route("/api/nas/users/:user_id/disable", post(nas_disable_user))
        // NAS Groups
        .route("/api/nas/groups", get(nas_list_groups))
        .route("/api/nas/groups", post(nas_create_group))
        .route("/api/nas/groups/:group_id", get(nas_get_group))
        .route("/api/nas/groups/:group_id", put(nas_update_group))
        .route("/api/nas/groups/:group_id", delete(nas_delete_group))
        .route(
            "/api/nas/groups/:group_id/members",
            get(nas_get_group_members),
        )
        .route(
            "/api/nas/groups/:group_id/members",
            post(nas_add_group_member),
        )
        .route(
            "/api/nas/groups/:group_id/members/:user_id",
            delete(nas_remove_group_member),
        )
        // NAS Pools
        .route("/api/nas/pools", get(nas_list_pools))
        .route("/api/nas/pools", post(nas_create_pool))
        .route("/api/nas/pools/:pool_id", get(nas_get_pool))
        .route("/api/nas/pools/:pool_id", delete(nas_delete_pool))
        .route("/api/nas/pools/:pool_id/scrub", post(nas_scrub_pool))
        .route("/api/nas/pools/:pool_id/status", get(nas_get_pool_status))
        // NAS Datasets
        .route("/api/nas/pools/:pool_id/datasets", get(nas_list_datasets))
        .route("/api/nas/pools/:pool_id/datasets", post(nas_create_dataset))
        .route("/api/nas/datasets/:dataset_id", get(nas_get_dataset))
        .route("/api/nas/datasets/:dataset_id", put(nas_update_dataset))
        .route("/api/nas/datasets/:dataset_id", delete(nas_delete_dataset))
        .route(
            "/api/nas/datasets/:dataset_id/quota",
            post(nas_set_dataset_quota),
        )
        // NAS Snapshots
        .route(
            "/api/nas/datasets/:dataset_id/snapshots",
            get(nas_list_snapshots),
        )
        .route(
            "/api/nas/datasets/:dataset_id/snapshots",
            post(nas_create_snapshot),
        )
        .route("/api/nas/snapshots/:snapshot_id", get(nas_get_snapshot))
        .route(
            "/api/nas/snapshots/:snapshot_id",
            delete(nas_delete_snapshot),
        )
        .route(
            "/api/nas/snapshots/:snapshot_id/rollback",
            post(nas_rollback_snapshot),
        )
        .route(
            "/api/nas/snapshots/:snapshot_id/clone",
            post(nas_clone_snapshot),
        )
        // Snapshot Policies
        .route(
            "/api/nas/snapshot-policies",
            get(nas_list_snapshot_policies),
        )
        .route(
            "/api/nas/snapshot-policies",
            post(nas_create_snapshot_policy),
        )
        .route(
            "/api/nas/snapshot-policies/:policy_id",
            get(nas_get_snapshot_policy),
        )
        .route(
            "/api/nas/snapshot-policies/:policy_id",
            put(nas_update_snapshot_policy),
        )
        .route(
            "/api/nas/snapshot-policies/:policy_id",
            delete(nas_delete_snapshot_policy),
        )
        // NAS Replication
        .route(
            "/api/nas/replication/tasks",
            get(nas_list_replication_tasks),
        )
        .route(
            "/api/nas/replication/tasks",
            post(nas_create_replication_task),
        )
        .route(
            "/api/nas/replication/tasks/:task_id",
            get(nas_get_replication_task),
        )
        .route(
            "/api/nas/replication/tasks/:task_id",
            put(nas_update_replication_task),
        )
        .route(
            "/api/nas/replication/tasks/:task_id",
            delete(nas_delete_replication_task),
        )
        .route(
            "/api/nas/replication/tasks/:task_id/run",
            post(nas_run_replication_task),
        )
        .route(
            "/api/nas/replication/tasks/:task_id/history",
            get(nas_get_replication_history),
        )
        // NAS Services
        .route("/api/nas/services", get(nas_list_services))
        .route("/api/nas/services/:service", get(nas_get_service_status))
        .route("/api/nas/services/:service/start", post(nas_start_service))
        .route("/api/nas/services/:service/stop", post(nas_stop_service))
        .route(
            "/api/nas/services/:service/restart",
            post(nas_restart_service),
        )
        // SMB-specific endpoints
        .route("/api/nas/smb/config", get(nas_get_smb_config))
        .route("/api/nas/smb/config", put(nas_update_smb_config))
        .route("/api/nas/smb/connections", get(nas_list_smb_connections))
        .route(
            "/api/nas/smb/connections/:conn_id/disconnect",
            post(nas_disconnect_smb),
        )
        // NFS-specific endpoints
        .route("/api/nas/nfs/config", get(nas_get_nfs_config))
        .route("/api/nas/nfs/config", put(nas_update_nfs_config))
        .route("/api/nas/nfs/clients", get(nas_list_nfs_clients))
        // iSCSI endpoints
        .route("/api/nas/iscsi/targets", get(nas_list_iscsi_targets))
        .route("/api/nas/iscsi/targets", post(nas_create_iscsi_target))
        .route(
            "/api/nas/iscsi/targets/:target_id",
            get(nas_get_iscsi_target),
        )
        .route(
            "/api/nas/iscsi/targets/:target_id",
            put(nas_update_iscsi_target),
        )
        .route(
            "/api/nas/iscsi/targets/:target_id",
            delete(nas_delete_iscsi_target),
        )
        .route(
            "/api/nas/iscsi/targets/:target_id/luns",
            get(nas_list_iscsi_luns),
        )
        .route(
            "/api/nas/iscsi/targets/:target_id/luns",
            post(nas_create_iscsi_lun),
        )
        .route("/api/nas/iscsi/luns/:lun_id", delete(nas_delete_iscsi_lun))
        .route(
            "/api/nas/iscsi/targets/:target_id/acls",
            get(nas_list_iscsi_acls),
        )
        .route(
            "/api/nas/iscsi/targets/:target_id/acls",
            post(nas_create_iscsi_acl),
        )
        .route("/api/nas/iscsi/acls/:acl_id", delete(nas_delete_iscsi_acl))
        // Directory/Auth configuration
        .route("/api/nas/directory/config", get(nas_get_directory_config))
        .route(
            "/api/nas/directory/config",
            put(nas_update_directory_config),
        )
        // LDAP endpoints
        .route("/api/nas/directory/ldap/status", get(nas_get_ldap_status))
        .route(
            "/api/nas/directory/ldap/configure",
            post(nas_configure_ldap),
        )
        .route(
            "/api/nas/directory/ldap/test",
            post(nas_test_ldap_connection),
        )
        .route("/api/nas/directory/ldap/sync", post(nas_sync_ldap))
        .route("/api/nas/directory/ldap/users", get(nas_search_ldap_users))
        .route(
            "/api/nas/directory/ldap/groups",
            get(nas_search_ldap_groups),
        )
        // Kerberos endpoints
        .route(
            "/api/nas/directory/kerberos/status",
            get(nas_get_kerberos_status),
        )
        .route(
            "/api/nas/directory/kerberos/configure",
            post(nas_configure_kerberos),
        )
        .route(
            "/api/nas/directory/kerberos/kinit",
            post(nas_kerberos_kinit),
        )
        .route("/api/nas/directory/kerberos/klist", get(nas_kerberos_klist))
        .route(
            "/api/nas/directory/kerberos/kdestroy",
            post(nas_kerberos_kdestroy),
        )
        .route(
            "/api/nas/directory/kerberos/keytabs",
            get(nas_list_kerberos_keytabs),
        )
        .route(
            "/api/nas/directory/kerberos/keytab",
            post(nas_create_kerberos_keytab),
        )
        // Active Directory endpoints
        .route("/api/nas/directory/ad/status", get(nas_get_ad_status))
        .route(
            "/api/nas/directory/ad/join",
            post(nas_join_active_directory),
        )
        .route(
            "/api/nas/directory/ad/leave",
            post(nas_leave_active_directory),
        )
        .route("/api/nas/directory/ad/users", get(nas_list_ad_users))
        .route("/api/nas/directory/ad/groups", get(nas_list_ad_groups))
        .route(
            "/api/nas/directory/ad/user/:username/groups",
            get(nas_get_user_ad_groups),
        )
        .route("/api/nas/directory/ad/test-trust", post(nas_test_ad_trust))
        .route("/api/nas/directory/ad/ping-dc", post(nas_ping_ad_dc))
        .route(
            "/api/nas/directory/ad/verify-prereqs",
            post(nas_verify_ad_prereqs),
        )
        // Scheduler endpoints
        .route("/api/nas/scheduler/jobs", get(nas_list_scheduled_jobs))
        .route("/api/nas/scheduler/jobs", post(nas_create_scheduled_job))
        .route(
            "/api/nas/scheduler/jobs/:job_id",
            get(nas_get_scheduled_job),
        )
        .route(
            "/api/nas/scheduler/jobs/:job_id",
            put(nas_update_scheduled_job),
        )
        .route(
            "/api/nas/scheduler/jobs/:job_id",
            delete(nas_delete_scheduled_job),
        )
        .route(
            "/api/nas/scheduler/jobs/:job_id/run",
            post(nas_run_scheduled_job),
        )
        .route(
            "/api/nas/scheduler/jobs/:job_id/pause",
            post(nas_pause_scheduled_job),
        )
        .route(
            "/api/nas/scheduler/jobs/:job_id/resume",
            post(nas_resume_scheduled_job),
        )
        .route(
            "/api/nas/scheduler/jobs/:job_id/history",
            get(nas_get_job_history),
        )
        .route("/api/nas/scheduler/status", get(nas_get_scheduler_status))
        // FTP-specific endpoints
        .route("/api/nas/ftp/config", get(nas_get_ftp_config))
        .route("/api/nas/ftp/config", put(nas_update_ftp_config))
        .route("/api/nas/ftp/connections", get(nas_list_ftp_connections))
        .route(
            "/api/nas/ftp/connections/:conn_id/disconnect",
            post(nas_disconnect_ftp),
        )
        // WebDAV-specific endpoints
        .route("/api/nas/webdav/config", get(nas_get_webdav_config))
        .route("/api/nas/webdav/config", put(nas_update_webdav_config))
        .route(
            "/api/nas/webdav/connections",
            get(nas_list_webdav_connections),
        )
        // AFP-specific endpoints
        .route("/api/nas/afp/config", get(nas_get_afp_config))
        .route("/api/nas/afp/config", put(nas_update_afp_config))
        .route("/api/nas/afp/connections", get(nas_list_afp_connections))
        .route(
            "/api/nas/afp/connections/:conn_id/disconnect",
            post(nas_disconnect_afp),
        )
        .route(
            "/api/nas/afp/timemachine",
            get(nas_list_timemachine_targets),
        )
}

// =============================================================================
// NAS Handler Functions (stubs - to be implemented)
// =============================================================================

#[cfg(feature = "nas")]
mod nas_handlers;

#[cfg(feature = "nas")]
use nas_handlers::*;

// =============================================================================
// Re-export standardized error types
// =============================================================================
use crate::error::ApiError;

// API handlers

/// Simple liveness check
async fn health_check() -> &'static str {
    "OK"
}

/// Detailed health check with component status
async fn health_detailed(State(state): State<Arc<AppState>>) -> Json<health::HealthResponse> {
    let checker = health::HealthChecker::new(env!("CARGO_PKG_VERSION"));

    let mut components = Vec::new();

    // Check database
    components.push(checker.check_database(&state.database).await);

    // Check monitoring
    components.push(checker.check_monitoring(&state.monitoring_manager).await);

    // Check storage
    components.push(checker.check_storage(&state.storage_manager).await);

    // Check VM manager
    components.push(checker.check_vm_manager(&state.vm_manager).await);

    // Check cluster
    components.push(checker.check_cluster(&state.cluster_manager).await);

    // Check WebSocket
    components.push(checker.check_websocket(&state.ws_state));

    Json(checker.build_response(components))
}

/// Liveness probe for container orchestration
async fn liveness_probe() -> Json<health::LivenessResponse> {
    let checker = health::HealthChecker::new(env!("CARGO_PKG_VERSION"));
    Json(checker.liveness())
}

/// Readiness probe for container orchestration
async fn readiness_probe(
    State(state): State<Arc<AppState>>,
) -> Result<Json<health::ReadinessResponse>, StatusCode> {
    let checker = health::HealthChecker::new(env!("CARGO_PKG_VERSION"));

    // Check critical components
    let db_health = checker.check_database(&state.database).await;

    let components = vec![db_health];
    let response = checker.readiness(&components);

    if response.ready {
        Ok(Json(response))
    } else {
        Err(StatusCode::SERVICE_UNAVAILABLE)
    }
}

async fn list_vms(State(state): State<Arc<AppState>>) -> Json<Vec<VmConfig>> {
    let vms = state.vm_manager.list_vms().await;
    Json(vms)
}

async fn get_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let vm = state.vm_manager.get_vm(&id).await?;
    Ok(Json(vm))
}

/// `POST /api/vms`: a VmConfig plus, for LXD/Incus VMs, the instance image.
#[derive(serde::Deserialize)]
struct CreateVmRequest {
    #[serde(flatten)]
    config: VmConfig,
    #[serde(default)]
    image: Option<String>,
}

async fn create_vm(
    State(state): State<Arc<AppState>>,
    auth_user: Option<axum::Extension<middleware::auth::AuthUser>>,
    Json(mut payload): Json<CreateVmRequest>,
) -> Result<(StatusCode, Json<VmConfig>), ApiError> {
    let vm_name = payload.config.name.clone();
    // Disks placed on a storage pool: resolve the pool to its directory.
    api::storage_volumes::place_vm_disks(&state, &mut payload.config).await?;
    // Bridges exist on this host, the ISO is in the ISO library.
    api::vm_hw::check_host_hardware(&state, &payload.config.hardware).await?;
    let vm = state
        .vm_manager
        .create_vm_from_image(payload.config, payload.image)
        .await?;

    // Broadcast VM created event
    let username = auth_user
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "system".to_string());
    state
        .ws_state
        .broadcast_vm_created(vm.id.clone(), vm_name, username);
    let _ = state
        .webhook_manager
        .trigger_event(
            webhooks::WebhookEventType::VmCreated,
            serde_json::json!({"vm_id": vm.id, "name": vm.name}),
        )
        .await;

    Ok((StatusCode::CREATED, Json(vm)))
}

/// An operator started/stopped an HA-managed VM: that is the state HA must
/// keep it in (a stopped one is not "failed" and is not restarted).
#[cfg(feature = "cluster")]
async fn ha_user_intent(state: &Arc<AppState>, vm_id: &str, want: ha::HaState) {
    if state.ha_manager.get_resource(vm_id).await.is_some() {
        let _ = state
            .ha_manager
            .update_resource_state(vm_id, want, None)
            .await;
    }
}

/// Tell event webhooks a VM changed state (`vm.state_changed`, plus the
/// specific `vm.started` / `vm.stopped`).
async fn emit_vm_state(state: &Arc<AppState>, vm: &VmConfig, old: &str) {
    let new = format!("{:?}", vm.status);
    if old == new {
        return;
    }
    let data = serde_json::json!({
        "vm_id": vm.id, "name": vm.name, "old_state": old, "new_state": new,
        "node": state.cluster_manager.get_local_node_name().await.unwrap_or_default(),
    });
    let _ = state
        .webhook_manager
        .trigger_event(webhooks::WebhookEventType::VmStateChanged, data.clone())
        .await;
    let specific = match vm.status {
        horcrux_common::VmStatus::Running if old != "Paused" => {
            Some(webhooks::WebhookEventType::VmStarted)
        }
        horcrux_common::VmStatus::Stopped => Some(webhooks::WebhookEventType::VmStopped),
        _ => None,
    };
    if let Some(ev) = specific {
        let _ = state.webhook_manager.trigger_event(ev, data).await;
    }
}

async fn start_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let old_status = state
        .vm_manager
        .get_vm(&id)
        .await
        .map(|vm| format!("{:?}", vm.status))
        .unwrap_or_else(|_| "unknown".to_string());

    // A guest with saved state resumes from it (starting it cold would make
    // the saved state stale).
    let vm = if api::guest_state::engine::has_saved(&state, api::guest_state::GuestKind::Vm, &id) {
        api::guest_state::engine::resume_guest(&state, api::guest_state::GuestKind::Vm, &id)
            .await?;
        state.vm_manager.get_vm(&id).await?
    } else {
        state.vm_manager.start_vm(&id).await?
    };
    #[cfg(feature = "cluster")]
    ha_user_intent(&state, &id, ha::HaState::Started).await;
    emit_vm_state(&state, &vm, &old_status).await;

    // Broadcast VM status change
    state
        .ws_state
        .broadcast_vm_status(id.clone(), old_status, format!("{:?}", vm.status));

    Ok(Json(vm))
}

async fn stop_vm(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<VmConfig>, ApiError> {
    let old_status = state
        .vm_manager
        .get_vm(&id)
        .await
        .map(|vm| format!("{:?}", vm.status))
        .unwrap_or_else(|_| "unknown".to_string());

    let vm = state.vm_manager.stop_vm(&id).await?;
    #[cfg(feature = "cluster")]
    ha_user_intent(&state, &id, ha::HaState::Stopped).await;
    emit_vm_state(&state, &vm, &old_status).await;

    // Broadcast VM status change
    state
        .ws_state
        .broadcast_vm_status(id.clone(), old_status, format!("{:?}", vm.status));

    Ok(Json(vm))
}

async fn delete_vm(
    State(state): State<Arc<AppState>>,
    auth_user: Option<axum::Extension<middleware::auth::AuthUser>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Get VM name before deletion
    let vm_name = state
        .vm_manager
        .get_vm(&id)
        .await
        .map(|vm| vm.name)
        .unwrap_or_else(|_| id.clone());

    state.vm_manager.delete_vm(&id).await?;
    // Its firewall goes with it: the rules used to stay stored, be
    // re-installed at every start, and apply to a new VM with the same id.
    if let Err(e) = state
        .firewall_manager
        .remove_guest_rules(FirewallScope::Vm(id.clone()))
        .await
    {
        tracing::warn!("VM {id} deleted, but its firewall rules stayed: {e}");
    }

    // Broadcast VM deleted event
    let username = auth_user
        .map(|u| u.username.clone())
        .unwrap_or_else(|| "system".to_string());
    state
        .ws_state
        .broadcast_vm_deleted(id.clone(), vm_name.clone(), username);
    let _ = state
        .webhook_manager
        .trigger_event(
            webhooks::WebhookEventType::VmDeleted,
            serde_json::json!({"vm_id": id, "name": vm_name}),
        )
        .await;

    Ok(StatusCode::NO_CONTENT)
}

// VM Snapshot handlers

#[derive(serde::Deserialize)]
struct CreateSnapshotRequest {
    name: String,
    description: Option<String>,
    include_memory: Option<bool>,
}

async fn list_vm_snapshots(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<Vec<vm::snapshot::VmSnapshot>>, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;
    let snapshots = snapshot_manager.list_snapshots(&vm_id);
    Ok(Json(snapshots))
}

async fn create_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<CreateSnapshotRequest>,
) -> Result<Json<vm::snapshot::VmSnapshot>, ApiError> {
    // Get VM config from database
    let vm_config = state
        .database
        .get_vm(&vm_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("VM {} not found", vm_id)))?;

    let mut snapshot_manager = state.snapshot_manager.write().await;

    let snapshot = snapshot_manager
        .create_snapshot(
            &vm_config,
            req.name,
            req.description,
            req.include_memory.unwrap_or(false),
        )
        .await
        .map_err(ApiError::from)?;

    Ok(Json(snapshot))
}

async fn get_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path((vm_id, snapshot_id)): Path<(String, String)>,
) -> Result<Json<vm::snapshot::VmSnapshot>, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;

    let snapshot = snapshot_manager
        .get_snapshot(&snapshot_id)
        .ok_or_else(|| ApiError::NotFound(format!("Snapshot {} not found", snapshot_id)))?;

    // Verify snapshot belongs to this VM
    if snapshot.vm_id != vm_id {
        return Err(ApiError::NotFound(format!(
            "Snapshot not found for VM {}",
            vm_id
        )));
    }

    Ok(Json(snapshot.clone()))
}

async fn delete_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path((vm_id, snapshot_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let mut snapshot_manager = state.snapshot_manager.write().await;
    snapshot_of_vm(&snapshot_manager, &vm_id, &snapshot_id)?;

    snapshot_manager
        .delete_snapshot(&snapshot_id)
        .await
        .map_err(ApiError::from)?;

    Ok(StatusCode::OK)
}

/// A snapshot addressed as /vms/:id/snapshots/:snapshot_id must belong to
/// that VM (a snapshot id of another VM is "not found" here).
fn snapshot_of_vm(
    m: &vm::snapshot::VmSnapshotManager,
    vm_id: &str,
    snapshot_id: &str,
) -> Result<(), ApiError> {
    match m.get_snapshot(snapshot_id) {
        Some(s) if s.vm_id == vm_id => Ok(()),
        _ => Err(ApiError::NotFound(format!(
            "Snapshot {snapshot_id} not found for VM {vm_id}"
        ))),
    }
}

#[derive(serde::Deserialize)]
struct RestoreSnapshotRequest {
    restore_memory: Option<bool>,
}

async fn restore_vm_snapshot(
    State(state): State<Arc<AppState>>,
    Path((vm_id, snapshot_id)): Path<(String, String)>,
    Json(req): Json<RestoreSnapshotRequest>,
) -> Result<StatusCode, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;
    snapshot_of_vm(&snapshot_manager, &vm_id, &snapshot_id)?;

    snapshot_manager
        .restore_snapshot(&snapshot_id, req.restore_memory.unwrap_or(false))
        .await
        .map_err(ApiError::from)?;

    Ok(StatusCode::OK)
}

async fn get_vm_snapshot_tree(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<Vec<vm::snapshot::SnapshotTreeNode>>, ApiError> {
    let snapshot_manager = state.snapshot_manager.read().await;
    let tree = snapshot_manager.build_snapshot_tree(&vm_id);
    Ok(Json(tree))
}

// Snapshot Schedule handlers
async fn list_snapshot_schedules(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<vm::snapshot_scheduler::SnapshotSchedule>> {
    let schedules = state.snapshot_scheduler.list_schedules().await;
    Json(schedules)
}

async fn create_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Json(mut schedule): Json<vm::snapshot_scheduler::SnapshotSchedule>,
) -> Result<Json<vm::snapshot_scheduler::SnapshotSchedule>, ApiError> {
    // Generate ID if not provided
    if schedule.id.is_empty() {
        schedule.id = uuid::Uuid::new_v4().to_string();
    }

    // Set created_at
    schedule.created_at = chrono::Utc::now().timestamp();

    // Calculate next_run
    schedule.next_run = schedule
        .frequency
        .next_run_after(chrono::Utc::now().timestamp());

    state
        .snapshot_scheduler
        .add_schedule(schedule.clone())
        .await?;
    info!("Created snapshot schedule: {}", schedule.id);
    Ok(Json(schedule))
}

async fn get_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::snapshot_scheduler::SnapshotSchedule>, ApiError> {
    let schedule = state
        .snapshot_scheduler
        .get_schedule(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Snapshot schedule '{}' not found", id)))?;
    Ok(Json(schedule))
}

async fn update_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut schedule): Json<vm::snapshot_scheduler::SnapshotSchedule>,
) -> Result<Json<vm::snapshot_scheduler::SnapshotSchedule>, ApiError> {
    // Ensure ID matches
    schedule.id = id.clone();

    // Recalculate next_run if frequency changed
    schedule.next_run = schedule
        .frequency
        .next_run_after(chrono::Utc::now().timestamp());

    state
        .snapshot_scheduler
        .update_schedule(schedule.clone())
        .await?;
    info!("Updated snapshot schedule: {}", id);
    Ok(Json(schedule))
}

async fn delete_snapshot_schedule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.snapshot_scheduler.remove_schedule(&id).await?;
    info!("Deleted snapshot schedule: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct CloneVmRequest {
    name: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default = "default_clone_mode")]
    mode: String, // "full" or "linked"
    #[serde(default)]
    start: bool,
    #[serde(default)]
    mac_addresses: Option<Vec<String>>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    network_config: Option<vm::clone::NetworkConfig>,
}

fn default_clone_mode() -> String {
    "full".to_string()
}

async fn clone_vm(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<CloneVmRequest>,
) -> Result<Json<VmConfig>, ApiError> {
    // Get source VM configuration
    let source_vm = state
        .database
        .get_vm(&vm_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("VM {} not found", vm_id)))?;

    // Parse clone mode
    let clone_mode = match req.mode.as_str() {
        "full" => vm::clone::CloneMode::Full,
        "linked" => vm::clone::CloneMode::Linked,
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Invalid clone mode: {}",
                req.mode
            )))
        }
    };

    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(ApiError::BadRequest("The clone needs a name".to_string()));
    }

    // Same id scheme as Create VM (`vm-<unix time>`) unless the caller
    // picks one; an id that's taken is refused before any disk is written
    // (the clone's disk is `<id>.qcow2`, so a reused id would overwrite
    // that VM's disk).
    let new_id = match req
        .id
        .map(|i| i.trim().to_string())
        .filter(|i| !i.is_empty())
    {
        Some(id) => {
            if state.vm_manager.get_vm(&id).await.is_ok() {
                return Err(ApiError::Conflict(format!(
                    "VM with ID {id} already exists"
                )));
            }
            id
        }
        None => next_vm_id(&state).await,
    };

    // Recorded as a clone job, so it shows on the Clones page and in
    // `horcrux clone list/status`.
    let jobs = &state.clone_job_manager;
    let job_id = jobs
        .create_job(vm_id.clone(), new_id.clone(), name.clone())
        .await;
    let _ = jobs.start_job(&job_id).await;
    let _ = jobs
        .update_progress(&job_id, vm::clone_progress::CloneStage::CloningDisk, 10)
        .await;

    let clone_options = vm::clone::CloneOptions {
        name,
        id: Some(new_id),
        mode: clone_mode,
        start: req.start,
        mac_addresses: req.mac_addresses,
        description: req.description,
        network_config: req.network_config,
    };

    let result: Result<VmConfig, ApiError> = async {
        let cloned_vm = state
            .clone_manager
            .clone_vm(&source_vm, clone_options)
            .await
            .map_err(|e| match e {
                horcrux_common::Error::Conflict(_) | horcrux_common::Error::Validation(_) => {
                    ApiError::from(e)
                }
                e => ApiError::Internal(format!("Failed to clone VM: {}", e)),
            })?;
        let _ = jobs
            .update_progress(&job_id, vm::clone_progress::CloneStage::Finalizing, 90)
            .await;
        state.database.create_vm(&cloned_vm).await.map_err(|e| {
            ApiError::Internal(format!("Failed to save cloned VM to database: {}", e))
        })?;
        Ok(cloned_vm)
    }
    .await;

    let cloned_vm = match result {
        Ok(vm) => {
            let _ = jobs.complete_job(&job_id).await;
            vm
        }
        Err(e) => {
            let r = e.to_error_response();
            let _ = jobs.fail_job(&job_id, r.details.unwrap_or(r.message)).await;
            return Err(e);
        }
    };

    info!(
        "VM {} cloned successfully to {} (job {})",
        vm_id, cloned_vm.id, job_id
    );

    if req.start {
        return Ok(Json(state.vm_manager.start_vm(&cloned_vm.id).await?));
    }
    Ok(Json(cloned_vm))
}

/// A free VM id in Create VM's scheme, `vm-<unix time>` (the next second
/// if that one is taken).
async fn next_vm_id(state: &AppState) -> String {
    let mut ts = chrono::Utc::now().timestamp();
    loop {
        let id = format!("vm-{ts}");
        let disk = state.config.paths.vm_storage.join(format!("{id}.qcow2"));
        if state.vm_manager.get_vm(&id).await.is_err() && !disk.exists() {
            return id;
        }
        ts += 1;
    }
}

#[derive(Debug, Deserialize)]
struct CrossNodeCloneRequest {
    target_node: String,
    source_node: String,
    name: String,
    id: Option<String>,
    ssh_port: Option<u16>,
    ssh_user: Option<String>,
    compression_enabled: Option<bool>,
    bandwidth_limit_mbps: Option<u32>,
    target_volume_group: Option<String>,
}

async fn clone_vm_cross_node(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<CrossNodeCloneRequest>,
) -> Result<Json<VmConfig>, ApiError> {
    use vm::clone::CloneOptions;
    use vm::cross_node_clone::{CrossNodeCloneConfig, CrossNodeCloneManager};

    // Get source VM configuration
    let source_vm = state
        .database
        .get_vm(&vm_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("VM {} not found", vm_id)))?;

    // Create clone options
    let new_id = match req.id.clone().filter(|i| !i.trim().is_empty()) {
        Some(id) => id,
        None => next_vm_id(&state).await,
    };
    let clone_options = CloneOptions {
        name: req.name.clone(),
        id: Some(new_id),
        mode: vm::clone::CloneMode::Full, // Cross-node clones are always full clones
        start: false,
        mac_addresses: None, // Auto-generate on target
        description: Some(format!("Cross-node clone from {}", req.source_node)),
        network_config: None,
    };

    // Create cross-node clone config
    let cross_node_config = CrossNodeCloneConfig {
        source_node: req.source_node.clone(),
        target_node: req.target_node.clone(),
        source_vm_id: vm_id.clone(),
        clone_options,
        ssh_port: req.ssh_port,
        ssh_user: req.ssh_user,
        compression_enabled: req.compression_enabled.unwrap_or(true),
        bandwidth_limit_mbps: req.bandwidth_limit_mbps,
        target_volume_group: req.target_volume_group,
    };

    // Perform cross-node clone
    let manager =
        CrossNodeCloneManager::new(state.config.paths.vm_storage.to_string_lossy().to_string());
    let cloned_vm = manager
        .clone_cross_node(&source_vm, cross_node_config)
        .await
        .map_err(|e| match e {
            horcrux_common::Error::Validation(_) => ApiError::from(e),
            e => ApiError::Internal(format!("Cross-node clone failed: {}", e)),
        })?;

    // Save to database
    state
        .database
        .create_vm(&cloned_vm)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to save cloned VM to database: {}", e)))?;

    info!(
        "VM {} cloned successfully from {} to {} as {}",
        vm_id, req.source_node, req.target_node, cloned_vm.id
    );

    Ok(Json(cloned_vm))
}

// Clone Job Progress API handlers
async fn list_clone_jobs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<vm::clone_progress::CloneJob>>, ApiError> {
    let jobs = state.clone_job_manager.list_jobs().await;
    Ok(Json(jobs))
}

async fn get_clone_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<vm::clone_progress::CloneJob>, ApiError> {
    let job = state
        .clone_job_manager
        .get_job(&job_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Clone job {} not found", job_id)))?;
    Ok(Json(job))
}

async fn cancel_clone_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .clone_job_manager
        .request_cancellation(&job_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to cancel clone job: {}", e)))?;

    Ok(Json(serde_json::json!({
        "status": "cancellation_requested",
        "job_id": job_id,
        "message": "Clone job cancellation has been requested"
    })))
}

async fn delete_clone_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Only allow deletion of completed or failed jobs
    let job = state
        .clone_job_manager
        .get_job(&job_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Clone job {} not found", job_id)))?;

    match job.state {
        vm::clone_progress::CloneJobState::Running => {
            return Err(ApiError::BadRequest(
                "Cannot delete running clone job. Cancel it first.".to_string(),
            ));
        }
        vm::clone_progress::CloneJobState::Queued => {
            return Err(ApiError::BadRequest(
                "Cannot delete queued clone job. Cancel it first.".to_string(),
            ));
        }
        _ => {}
    }

    state
        .clone_job_manager
        .delete_job(&job_id)
        .await
        .map_err(|e| ApiError::Internal(format!("Failed to delete clone job: {}", e)))?;

    Ok(StatusCode::NO_CONTENT)
}

// Replication API handlers
#[derive(Debug, Deserialize)]
struct CreateReplicationJobRequest {
    source_vm_id: String,
    source_snapshot: String,
    target_node: String,
    target_pool: String,
    schedule: String, // "hourly", "daily", "weekly", or "manual"
    #[serde(default)]
    bandwidth_limit_mbps: Option<u32>,
    #[serde(default = "default_retention_count")]
    retention_count: u32,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

fn default_retention_count() -> u32 {
    7
}

fn default_enabled() -> bool {
    true
}

async fn list_replication_jobs(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<vm::replication::ReplicationJob>> {
    let jobs = state.replication_manager.list_jobs().await;
    Json(jobs)
}

async fn create_replication_job(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateReplicationJobRequest>,
) -> Result<Json<vm::replication::ReplicationJob>, ApiError> {
    use vm::replication::{ReplicationJob, ReplicationSchedule};

    // Parse schedule
    let schedule = match req.schedule.as_str() {
        "hourly" => ReplicationSchedule::Hourly,
        "daily" => ReplicationSchedule::Daily { hour: 2 }, // Default to 2 AM
        "weekly" => ReplicationSchedule::Weekly { day: 0, hour: 2 }, // Default to Sunday 2 AM
        "manual" => ReplicationSchedule::Manual,
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Invalid schedule: {}",
                req.schedule
            )))
        }
    };

    // Create replication job
    let job_id = uuid::Uuid::new_v4().to_string();
    let job_name = format!("Replication: {} -> {}", req.source_vm_id, req.target_node);

    let job = ReplicationJob {
        id: job_id,
        name: job_name,
        source_vm_id: req.source_vm_id,
        source_snapshot: req.source_snapshot,
        target_node: req.target_node,
        target_pool: req.target_pool,
        schedule,
        bandwidth_limit_mbps: req.bandwidth_limit_mbps,
        retention_count: req.retention_count,
        enabled: req.enabled,
        created_at: chrono::Utc::now().timestamp(),
        last_run: None,
        next_run: 0, // Will be set by create_job
    };

    // Validate source VM exists
    state
        .database
        .get_vm(&job.source_vm_id)
        .await
        .map_err(|_| ApiError::NotFound(format!("Source VM {} not found", job.source_vm_id)))?;

    let job = state.replication_manager.create_job(job).await?;
    info!("Created replication job: {}", job.id);
    Ok(Json(job))
}

async fn get_replication_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::replication::ReplicationJob>, ApiError> {
    let job = state
        .replication_manager
        .get_job(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Replication job {} not found", id)))?;
    Ok(Json(job))
}

async fn delete_replication_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.replication_manager.delete_job(&id).await?;
    info!("Deleted replication job: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn execute_replication_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Execute replication
    state.replication_manager.execute_replication(&id).await?;

    info!("Started replication job: {}", id);
    Ok(StatusCode::ACCEPTED)
}

async fn get_replication_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::replication::ReplicationState>, ApiError> {
    let status = state
        .replication_manager
        .get_state(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("No active replication for job {}", id)))?;
    Ok(Json(status))
}

// Snapshot Quota handlers
#[derive(Debug, Deserialize)]
struct CreateSnapshotQuotaRequest {
    name: String,
    quota_type: String, // "per_vm", "per_pool", "global"
    target_id: String,
    max_size_bytes: u64,
    max_count: Option<u32>,
    warning_threshold_percent: Option<u8>,
    auto_cleanup_enabled: Option<bool>,
    cleanup_policy: Option<String>, // "oldest_first", "largest_first", "least_used_first", "manual"
}

async fn list_snapshot_quotas(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<vm::snapshot_quota::SnapshotQuota>> {
    let quotas = state.snapshot_quota_manager.list_quotas().await;
    Json(quotas)
}

async fn create_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateSnapshotQuotaRequest>,
) -> Result<Json<vm::snapshot_quota::SnapshotQuota>, ApiError> {
    use vm::snapshot_quota::{CleanupPolicy, QuotaType};

    let quota_type = match req.quota_type.as_str() {
        "per_vm" => QuotaType::PerVm,
        "per_pool" => QuotaType::PerPool,
        "global" => QuotaType::Global,
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Invalid quota type: {}",
                req.quota_type
            )))
        }
    };

    let cleanup_policy = match req.cleanup_policy.as_deref().unwrap_or("oldest_first") {
        "oldest_first" => CleanupPolicy::OldestFirst,
        "largest_first" => CleanupPolicy::LargestFirst,
        "least_used_first" => CleanupPolicy::LeastUsedFirst,
        "manual" => CleanupPolicy::Manual,
        other => {
            return Err(ApiError::BadRequest(format!(
                "Invalid cleanup policy: {}",
                other
            )))
        }
    };

    let quota = state
        .snapshot_quota_manager
        .create_quota(
            req.name,
            quota_type,
            req.target_id,
            req.max_size_bytes,
            req.max_count,
            req.warning_threshold_percent.unwrap_or(80),
            req.auto_cleanup_enabled.unwrap_or(false),
            cleanup_policy,
        )
        .await?;

    info!("Created snapshot quota: {} ({})", quota.id, quota.name);
    Ok(Json(quota))
}

async fn get_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::snapshot_quota::SnapshotQuota>, ApiError> {
    let quota = state
        .snapshot_quota_manager
        .get_quota(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Quota not found: {}", id)))?;
    Ok(Json(quota))
}

#[derive(Debug, Deserialize)]
struct UpdateSnapshotQuotaRequest {
    max_size_bytes: Option<u64>,
    max_count: Option<Option<u32>>,
    warning_threshold_percent: Option<u8>,
    auto_cleanup_enabled: Option<bool>,
    cleanup_policy: Option<String>,
}

async fn update_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSnapshotQuotaRequest>,
) -> Result<Json<vm::snapshot_quota::SnapshotQuota>, ApiError> {
    use vm::snapshot_quota::CleanupPolicy;

    let cleanup_policy = if let Some(policy_str) = req.cleanup_policy {
        Some(match policy_str.as_str() {
            "oldest_first" => CleanupPolicy::OldestFirst,
            "largest_first" => CleanupPolicy::LargestFirst,
            "least_used_first" => CleanupPolicy::LeastUsedFirst,
            "manual" => CleanupPolicy::Manual,
            other => {
                return Err(ApiError::BadRequest(format!(
                    "Invalid cleanup policy: {}",
                    other
                )))
            }
        })
    } else {
        None
    };

    let quota = state
        .snapshot_quota_manager
        .update_quota(
            &id,
            req.max_size_bytes,
            req.max_count,
            req.warning_threshold_percent,
            req.auto_cleanup_enabled,
            cleanup_policy,
        )
        .await?;

    info!("Updated snapshot quota: {}", id);
    Ok(Json(quota))
}

async fn delete_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.snapshot_quota_manager.delete_quota(&id).await?;
    info!("Deleted snapshot quota: {}", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn get_snapshot_quota_usage(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<vm::snapshot_quota::QuotaUsage>, ApiError> {
    let usage = state.snapshot_quota_manager.get_usage(&id).await?;
    Ok(Json(usage))
}

async fn get_snapshot_quota_summary(
    State(state): State<Arc<AppState>>,
) -> Json<vm::snapshot_quota::QuotaSummary> {
    let summary = state.snapshot_quota_manager.get_quota_summary().await;
    Json(summary)
}

#[derive(Debug, Deserialize)]
struct EnforceQuotaRequest {
    snapshot_ids: Vec<String>,
}

async fn enforce_snapshot_quota(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<EnforceQuotaRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let freed_bytes = state
        .snapshot_quota_manager
        .enforce_quota(&id, req.snapshot_ids)
        .await?;

    info!("Enforced quota {}: freed {} bytes", id, freed_bytes);
    Ok(Json(serde_json::json!({
        "quota_id": id,
        "freed_bytes": freed_bytes
    })))
}

// Audit log handlers
#[derive(Debug, Deserialize)]
struct QueryAuditEventsParams {
    event_type: Option<String>,
    user: Option<String>,
    severity: Option<String>,
    start_time: Option<i64>,
    end_time: Option<i64>,
    limit: Option<usize>,
}

async fn query_audit_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<QueryAuditEventsParams>,
) -> Result<Json<Vec<audit::AuditEvent>>, ApiError> {
    let event_type = params.event_type.and_then(|s| match s.as_str() {
        "Login" => Some(audit::AuditEventType::Login),
        "Logout" => Some(audit::AuditEventType::Logout),
        "LoginFailed" => Some(audit::AuditEventType::LoginFailed),
        "PermissionGranted" => Some(audit::AuditEventType::PermissionGranted),
        "PermissionDenied" => Some(audit::AuditEventType::PermissionDenied),
        "VmCreated" => Some(audit::AuditEventType::VmCreated),
        "VmDeleted" => Some(audit::AuditEventType::VmDeleted),
        "VmStarted" => Some(audit::AuditEventType::VmStarted),
        "VmStopped" => Some(audit::AuditEventType::VmStopped),
        _ => None,
    });

    let severity = params.severity.and_then(|s| match s.as_str() {
        "Info" => Some(audit::AuditSeverity::Info),
        "Warning" => Some(audit::AuditSeverity::Warning),
        "Error" => Some(audit::AuditSeverity::Error),
        "Critical" => Some(audit::AuditSeverity::Critical),
        _ => None,
    });

    let start_time = params
        .start_time
        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));

    let end_time = params
        .end_time
        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0));

    let events = state
        .audit_logger
        .query(
            event_type,
            params.user,
            severity,
            start_time,
            end_time,
            params.limit,
        )
        .await;

    Ok(Json(events))
}

async fn get_audit_stats(
    State(state): State<Arc<AppState>>,
) -> Result<Json<std::collections::HashMap<String, usize>>, ApiError> {
    let stats = state.audit_logger.get_event_counts().await;
    Ok(Json(stats))
}

async fn get_security_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LimitParam>,
) -> Result<Json<Vec<audit::AuditEvent>>, ApiError> {
    let limit = params.limit.unwrap_or(100);
    let events = state.audit_logger.get_security_events(limit).await;
    Ok(Json(events))
}

#[derive(Debug, Deserialize)]
struct FailedLoginsQuery {
    user: Option<String>,
    limit: Option<usize>,
}

async fn get_failed_logins(
    State(state): State<Arc<AppState>>,
    Query(params): Query<FailedLoginsQuery>,
) -> Result<Json<Vec<audit::AuditEvent>>, ApiError> {
    let limit = params.limit.unwrap_or(100);
    let events = state
        .audit_logger
        .get_failed_logins(params.user, limit)
        .await;
    Ok(Json(events))
}

#[derive(Debug, Deserialize)]
struct BruteForceQuery {
    threshold: Option<usize>,
    window_minutes: Option<i64>,
}

async fn detect_brute_force_attempts(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BruteForceQuery>,
) -> Result<Json<Vec<String>>, ApiError> {
    let threshold = params.threshold.unwrap_or(5);
    let window_minutes = params.window_minutes.unwrap_or(10);
    let suspects = state
        .audit_logger
        .detect_brute_force(threshold, window_minutes)
        .await;
    Ok(Json(suspects))
}

#[derive(Debug, Deserialize)]
struct LimitParam {
    limit: Option<usize>,
}

// Container lifecycle handlers
async fn list_containers(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<horcrux_common::ContainerConfig>> {
    let containers = state.container_manager.list_containers().await;
    Json(containers)
}

async fn create_container(
    State(state): State<Arc<AppState>>,
    Json(config): Json<horcrux_common::ContainerConfig>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    // Run on its own task: if the client disconnects/times out mid-request,
    // dropping this handler must not abandon a create that the runtime has
    // already performed (that left containers in docker that horcrux never
    // registered - untracked orphans).
    let manager = state.container_manager.clone();
    let container = tokio::spawn(async move { manager.create_container(config).await })
        .await
        .map_err(|e| ApiError::Internal(format!("container create task failed: {e}")))??;

    // Broadcast container created event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerStatusChanged {
            container_id: container.id.clone(),
            old_status: "none".to_string(),
            new_status: container.status.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!("Container {} created successfully", container.id);
    Ok(Json(container))
}

async fn get_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let container = state.container_manager.get_container(&id).await?;
    Ok(Json(container))
}

async fn delete_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Own task for the same reason as create_container.
    let manager = state.container_manager.clone();
    let del_id = id.clone();
    tokio::spawn(async move { manager.delete_container(&del_id).await })
        .await
        .map_err(|e| ApiError::Internal(format!("container delete task failed: {e}")))??;
    // Its firewall goes with it (a new container with the same id must
    // not inherit the rules).
    if let Err(e) = state
        .firewall_manager
        .remove_guest_rules(FirewallScope::Container(id.clone()))
        .await
    {
        tracing::warn!("container {id} deleted, but its firewall rules stayed: {e}");
    }

    // Broadcast container deleted event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerDeleted {
            container_id: id.clone(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!("Container {} deleted successfully", id);
    Ok(StatusCode::NO_CONTENT)
}

async fn start_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container =
        if api::guest_state::engine::has_saved(&state, api::guest_state::GuestKind::Container, &id)
        {
            api::guest_state::engine::resume_guest(
                &state,
                api::guest_state::GuestKind::Container,
                &id,
            )
            .await?;
            state.container_manager.get_container(&id).await?
        } else {
            state.container_manager.start_container(&id).await?
        };

    // Broadcast status change event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerStatusChanged {
            container_id: id.clone(),
            old_status: old_status.to_string(),
            new_status: "running".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!("Container {} started successfully", id);
    Ok(Json(container))
}

async fn stop_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.stop_container(&id).await?;

    // Broadcast status change event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerStatusChanged {
            container_id: id.clone(),
            old_status: old_status.to_string(),
            new_status: "stopped".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!("Container {} stopped successfully", id);
    Ok(Json(container))
}

async fn pause_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.pause_container(&id).await?;

    // Broadcast status change event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerStatusChanged {
            container_id: id.clone(),
            old_status: old_status.to_string(),
            new_status: "paused".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!("Container {} paused successfully", id);
    Ok(Json(container))
}

async fn resume_container(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let old_status = state.container_manager.get_container(&id).await?.status;
    let container = state.container_manager.resume_container(&id).await?;

    // Broadcast status change event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerStatusChanged {
            container_id: id.clone(),
            old_status: old_status.to_string(),
            new_status: "running".to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!("Container {} resumed successfully", id);
    Ok(Json(container))
}

#[derive(serde::Serialize)]
struct ContainerStatusResponse {
    status: String,
}

async fn get_container_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ContainerStatusResponse>, ApiError> {
    let status = state.container_manager.get_container_status(&id).await?;
    Ok(Json(ContainerStatusResponse {
        status: status.to_string(),
    }))
}

#[derive(serde::Deserialize)]
struct ExecCommandRequest {
    command: Vec<String>,
}

#[derive(serde::Serialize)]
struct ExecCommandResponse {
    output: String,
}

async fn exec_container_command(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ExecCommandRequest>,
) -> Result<Json<ExecCommandResponse>, ApiError> {
    let output = state
        .container_manager
        .exec_command(&id, req.command)
        .await?;
    Ok(Json(ExecCommandResponse { output }))
}

#[derive(serde::Deserialize)]
struct CloneContainerRequest {
    target_id: String,
    target_name: String,
    #[serde(default)]
    snapshot: bool,
}

async fn clone_container(
    State(state): State<Arc<AppState>>,
    Path(source_id): Path<String>,
    Json(req): Json<CloneContainerRequest>,
) -> Result<Json<horcrux_common::ContainerConfig>, ApiError> {
    let cloned_container = state
        .container_manager
        .clone_container(&source_id, &req.target_id, &req.target_name, req.snapshot)
        .await?;

    // Broadcast container created event
    state
        .ws_state
        .broadcast(websocket::WsEvent::ContainerStatusChanged {
            container_id: cloned_container.id.clone(),
            old_status: "none".to_string(),
            new_status: cloned_container.status.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        });

    info!(
        "Container {} cloned to {} successfully",
        source_id, req.target_id
    );
    Ok(Json(cloned_container))
}

// Cloud-init API handlers

async fn generate_cloudinit(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(config): Json<CloudInitConfig>,
) -> Result<Json<CloudInitResponse>, ApiError> {
    // The seed is attached to the VM (as a CD-ROM at its next start), so
    // the VM must exist; it used to be generated and never attached.
    let vm = state.vm_manager.get_vm(&vm_id).await?;
    let iso_path = state
        .cloudinit_manager
        .generate_iso(&vm_id, &config)
        .await?;
    let attached = match vm.disks.first() {
        Some(d) => {
            let seed = vm::qemu::cloudinit_seed(std::path::Path::new(&d.path));
            tokio::fs::copy(&iso_path, &seed).await?;
            Some(seed.to_string_lossy().to_string())
        }
        None => None,
    };
    Ok(Json(CloudInitResponse {
        vm_id,
        iso_path: iso_path.to_string_lossy().to_string(),
        attached_as: attached,
    }))
}

async fn delete_cloudinit(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.cloudinit_manager.delete_iso(&vm_id).await?;
    if let Ok(vm) = state.vm_manager.get_vm(&vm_id).await {
        if let Some(d) = vm.disks.first() {
            let _ = tokio::fs::remove_file(vm::qemu::cloudinit_seed(std::path::Path::new(&d.path)))
                .await;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Serialize)]
struct CloudInitResponse {
    vm_id: String,
    iso_path: String,
    /// The seed ISO the VM boots with (next start), if the VM has a disk.
    attached_as: Option<String>,
}

// Template API handlers

async fn list_templates(State(state): State<Arc<AppState>>) -> Json<Vec<Template>> {
    let templates = state.template_manager.list_templates().await;
    Json(templates)
}

async fn get_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Template>, ApiError> {
    let template = state.template_manager.get_template(&id).await?;
    Ok(Json(template))
}

#[derive(serde::Deserialize)]
struct CreateTemplateRequest {
    vm_id: String,
    name: String,
    description: Option<String>,
    /// Optional: must be one of the VM's own disks. Defaults to its first
    /// disk. (Accepting any path let a client copy arbitrary host files
    /// into a template.)
    #[serde(default)]
    disk_path: Option<String>,
    #[serde(default = "default_template_storage")]
    storage_type: StorageType,
    #[serde(default)]
    memory: Option<u64>,
    #[serde(default)]
    cpus: Option<u32>,
    #[serde(default = "default_template_os")]
    os_type: OsType,
}

fn default_template_storage() -> StorageType {
    StorageType::Directory
}

fn default_template_os() -> OsType {
    OsType::Linux
}

async fn create_template(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<CreateTemplateRequest>,
) -> Result<(StatusCode, Json<Template>), ApiError> {
    let vm = state.vm_manager.get_vm(&payload.vm_id).await?;
    let vm_disks: Vec<String> = vm.disks.iter().map(|d| d.path.clone()).collect();
    let disk_path = match payload.disk_path {
        Some(p) if vm_disks.contains(&p) => p,
        Some(p) => {
            return Err(ApiError::ValidationError(format!(
                "{} is not a disk of VM {}",
                p, payload.vm_id
            )))
        }
        None => vm_disks.first().cloned().ok_or_else(|| {
            ApiError::ValidationError(format!("VM {} has no disk", payload.vm_id))
        })?,
    };
    let template = state
        .template_manager
        .create_template(
            &payload.vm_id,
            payload.name,
            payload.description,
            std::path::PathBuf::from(disk_path),
            payload.storage_type,
            payload.memory.unwrap_or(vm.memory),
            payload.cpus.unwrap_or(vm.cpus),
            payload.os_type,
        )
        .await?;
    // Deployed VMs get the source VM's chipset/firmware/CPU/NIC layout
    // and the disk's bus, or a UEFI/Q35/VirtIO guest wouldn't boot.
    let bus = vm
        .disks
        .iter()
        .find(|d| d.path == template.disk_path.to_string_lossy())
        .map(|d| d.disk_type.clone())
        .unwrap_or_else(|| "default".to_string());
    let template = state
        .template_manager
        .set_hardware(&template.id, vm.hardware.clone(), bus)
        .await?;
    Ok((StatusCode::CREATED, Json(template)))
}

async fn delete_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    // A template's disk is the base image of the VMs linked-cloned from
    // it: deleting it would leave them unbootable.
    if let Ok(t) = state.template_manager.get_template(&id).await {
        let dependents = state.vm_manager.vms_backed_by(&t.disk_path).await;
        if !dependents.is_empty() {
            return Err(ApiError::Conflict(format!(
                "Template {id} can't be deleted: its disk is the base image of the linked \
                 clone(s) {}. Delete those VMs first.",
                dependents.join(", ")
            )));
        }
    }
    state.template_manager.delete_template(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn clone_template(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(request): Json<CloneRequest>,
) -> Result<Json<CloneResponse>, ApiError> {
    // The new VM's ID names its disk (`<vm storage>/<id>.qcow2`): same rule
    // as Create VM / Clone VM (422), checked before anything is written.
    crate::validation::validate_vm_id(&request.new_vm_id)?;
    let template = state.template_manager.get_template(&id).await?;
    if state.vm_manager.get_vm(&request.new_vm_id).await.is_ok() {
        return Err(ApiError::Conflict(format!(
            "VM {} already exists",
            request.new_vm_id
        )));
    }
    let new_vm_name = request.new_vm_name.clone();
    let new_vm_id = state.template_manager.clone_template(&id, request).await?;

    // Register the clone as a real VM (it used to only write a disk file,
    // so the "deployed" VM never appeared anywhere and couldn't be started).
    let disk = state.template_manager.clone_disk_path(&new_vm_id);
    // The VM's disk is as big as the template image (GB, rounded up).
    let (_, virtual_bytes) = crate::template::disk_image_sizes(&disk).await;
    let size_gb = virtual_bytes.div_ceil(1024 * 1024 * 1024);
    state
        .vm_manager
        .import_vm(VmConfig {
            id: new_vm_id.clone(),
            name: new_vm_name,
            hypervisor: horcrux_common::VmHypervisor::Qemu,
            memory: template.memory,
            cpus: template.cpus,
            disk_size: size_gb,
            status: horcrux_common::VmStatus::Stopped,
            architecture: Default::default(),
            disks: vec![horcrux_common::VmDisk {
                path: disk.to_string_lossy().to_string(),
                size_gb,
                // Templates made before buses were recorded came from VMs
                // whose disk was on QEMU's default bus.
                disk_type: template
                    .disk_bus
                    .clone()
                    .unwrap_or_else(|| "default".to_string()),
                ..Default::default()
            }],
            display: Default::default(),
            hardware: template.hardware.for_clone(),
        })
        .await?;
    Ok(Json(CloneResponse { new_vm_id }))
}

#[derive(serde::Serialize)]
struct CloneResponse {
    new_vm_id: String,
}

// Firewall API handlers

async fn list_firewall_rules(State(state): State<Arc<AppState>>) -> Json<Vec<FirewallRule>> {
    let rules = state
        .firewall_manager
        .list_rules(FirewallScope::Datacenter)
        .await;
    Json(rules)
}

async fn add_firewall_rule(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<FirewallRule>,
) -> Result<StatusCode, ApiError> {
    state
        .firewall_manager
        .add_rule(FirewallScope::Datacenter, rule)
        .await?;
    Ok(StatusCode::CREATED)
}

async fn delete_firewall_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .firewall_manager
        .delete_rule(FirewallScope::Datacenter, &id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_security_groups(State(state): State<Arc<AppState>>) -> Json<Vec<SecurityGroup>> {
    let groups = state.firewall_manager.list_security_groups().await;
    Json(groups)
}

async fn get_security_group(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<SecurityGroup>, ApiError> {
    let group = state.firewall_manager.get_security_group(&name).await?;
    Ok(Json(group))
}

async fn apply_firewall_rules(
    State(state): State<Arc<AppState>>,
    Path(scope): Path<String>,
) -> Result<StatusCode, ApiError> {
    // Parse scope from path (e.g., "datacenter", "node-node1", "vm-100")
    let firewall_scope = parse_firewall_scope(&scope)?;
    state.firewall_manager.apply_rules(firewall_scope).await?;
    Ok(StatusCode::OK)
}

fn parse_firewall_scope(scope: &str) -> Result<FirewallScope, ApiError> {
    if scope == "datacenter" {
        Ok(FirewallScope::Datacenter)
    } else if let Some(node_name) = scope.strip_prefix("node-") {
        Ok(FirewallScope::Node(node_name.to_string()))
    } else if let Some(vm_id) = scope.strip_prefix("vm-") {
        Ok(FirewallScope::Vm(vm_id.to_string()))
    } else if let Some(container_id) = scope.strip_prefix("container-") {
        Ok(FirewallScope::Container(container_id.to_string()))
    } else {
        Err(ApiError::Internal(format!(
            "Invalid firewall scope: {}",
            scope
        )))
    }
}

// Monitoring API handlers

async fn get_node_stats(State(state): State<Arc<AppState>>) -> Json<Option<NodeMetrics>> {
    let metrics = state.monitoring_manager.get_node_metrics().await;
    Json(metrics)
}

async fn get_all_vm_stats(State(state): State<Arc<AppState>>) -> Json<Vec<ResourceMetrics>> {
    let metrics = state.monitoring_manager.list_vm_metrics().await;
    Json(metrics)
}

async fn get_vm_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Option<ResourceMetrics>> {
    let metrics = state.monitoring_manager.get_vm_metrics(&id).await;
    Json(metrics)
}

async fn get_all_container_stats(State(state): State<Arc<AppState>>) -> Json<Vec<ResourceMetrics>> {
    let metrics = state.monitoring_manager.list_container_metrics().await;
    Json(metrics)
}

async fn get_container_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Json<Option<ResourceMetrics>> {
    let metrics = state.monitoring_manager.get_container_metrics(&id).await;
    Json(metrics)
}

async fn get_all_storage_stats(State(state): State<Arc<AppState>>) -> Json<Vec<StorageMetrics>> {
    let metrics = state.monitoring_manager.list_storage_metrics().await;
    Json(metrics)
}

async fn get_storage_stats(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Json<Option<StorageMetrics>> {
    let metrics = state.monitoring_manager.get_storage_metrics(&name).await;
    Json(metrics)
}

async fn get_metric_history(
    State(state): State<Arc<AppState>>,
    Path(metric): Path<String>,
    axum::extract::Query(params): axum::extract::Query<HistoryParams>,
) -> Json<Vec<monitoring::TimeSeriesPoint>> {
    let from = params.from.unwrap_or(0);
    let to = params.to.unwrap_or(chrono::Utc::now().timestamp());

    let history = state
        .monitoring_manager
        .get_history(&metric, from, to)
        .await;
    Json(history)
}

#[derive(serde::Deserialize)]
struct HistoryParams {
    from: Option<i64>,
    to: Option<i64>,
}

// Console API handlers

async fn console_for(
    state: &AppState,
    vm_id: &str,
    kind: ConsoleType,
) -> Result<ConsoleInfo, ApiError> {
    let vm = state.vm_manager.get_vm(vm_id).await?;
    Ok(state.console_manager.create_console(&vm, kind).await?)
}

async fn create_vnc_console(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<ConsoleInfo>, ApiError> {
    Ok(Json(console_for(&state, &vm_id, ConsoleType::Vnc).await?))
}

async fn get_vnc_websocket(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<String>, ApiError> {
    let info = console_for(&state, &vm_id, ConsoleType::Vnc).await?;
    Ok(Json(info.ws_path))
}

async fn verify_console_ticket(
    State(state): State<Arc<AppState>>,
    Path(ticket_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.console_manager.verify_ticket(&ticket_id).await?;
    Ok(StatusCode::OK)
}

async fn get_novnc_page(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<axum::response::Html<String>, ApiError> {
    let info = console_for(&state, &vm_id, ConsoleType::Vnc).await?;
    // Path-only WebSocket URL; the noVNC page resolves it against its own
    // origin (ws/wss + host:port). A hardcoded `ws://localhost:8006` broke the
    // console for every browser not running on the API host itself.
    let html =
        console::novnc::get_novnc_html(&info.ticket, &info.ws_path, info.password.as_deref());
    Ok(axum::response::Html(html))
}

async fn vnc_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    console::novnc::handle_vnc_websocket(ws, Path(ticket_id), State(state.console_manager.clone()))
        .await
}

async fn create_serial_console(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<ConsoleInfo>, ApiError> {
    Ok(Json(
        console_for(&state, &vm_id, ConsoleType::Serial).await?,
    ))
}

async fn serial_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    console::serial_ws::handle_serial_websocket(
        ws,
        Path(ticket_id),
        State(state.console_manager.clone()),
    )
    .await
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn container_logs_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    container::ws_handlers::handle_logs_ws(
        ws,
        Path(ticket_id),
        State(state.container_ws_manager.clone()),
    )
    .await
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn container_exec_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    container::ws_handlers::handle_exec_ws(
        ws,
        Path(ticket_id),
        State(state.container_ws_manager.clone()),
    )
    .await
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn container_console_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    container::ws_handlers::handle_console_ws(
        ws,
        Path(ticket_id),
        State(state.container_ws_manager.clone()),
    )
    .await
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn container_events_websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    Path(ticket_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    container::ws_handlers::handle_events_ws(
        ws,
        Path(ticket_id),
        State(state.container_ws_manager.clone()),
    )
    .await
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
#[derive(serde::Deserialize)]
struct RuntimeQuery {
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
fn parse_runtime(q: &Option<String>) -> horcrux_common::ContainerRuntime {
    match q.as_deref() {
        Some("podman") => horcrux_common::ContainerRuntime::Podman,
        Some("lxc") => horcrux_common::ContainerRuntime::Lxc,
        Some("lxd") => horcrux_common::ContainerRuntime::Lxd,
        Some("incus") => horcrux_common::ContainerRuntime::Incus,
        Some("docker") => horcrux_common::ContainerRuntime::Docker,
        // No runtime named: whichever of docker/podman this build has
        // (features::oci_runtime_gate already refused a disabled one).
        _ => features::default_oci_runtime(),
    }
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn list_images(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<Vec<container::image::ImageInfo>>, ApiError> {
    let images = match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => state.image_manager.podman.list_images().await,
        _ => state.image_manager.docker.list_images().await,
    }
    .map_err(ApiError::from)?;
    Ok(Json(images))
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Deserialize)]
struct PullImageRequest {
    name: String,
    #[serde(default = "default_tag")]
    tag: String,
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(feature = "docker", feature = "podman"))]
fn default_tag() -> String {
    "latest".to_string()
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Serialize)]
struct PullImageResponse {
    lines: Vec<container::image::PullProgress>,
}

#[cfg(any(feature = "docker", feature = "podman"))]
/// Synchronous pull (collects every progress line and returns once done).
/// For live streaming progress, the UI can instead use a logs-ticket-style
/// WebSocket in a future iteration; this endpoint already surfaces full
/// per-layer progress history to the caller, which covers the common case.
async fn pull_image(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PullImageRequest>,
) -> Result<Json<PullImageResponse>, ApiError> {
    let mut lines = Vec::new();
    match parse_runtime(&req.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .image_manager
                .podman
                .pull_image(&req.name, &req.tag, |p| lines.push(p))
                .await
        }
        _ => {
            state
                .image_manager
                .docker
                .pull_image(&req.name, &req.tag, |p| lines.push(p))
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(Json(PullImageResponse { lines }))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn inspect_image(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let value = match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state.image_manager.podman.inspect_image(&id).await
        }
        _ => state.image_manager.docker.inspect_image(&id).await,
    }
    .map_err(ApiError::from)?;
    Ok(Json(value))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn remove_image(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<StatusCode, ApiError> {
    match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state.image_manager.podman.remove_image(&id, true).await
        }
        _ => state.image_manager.docker.remove_image(&id, true).await,
    }
    .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Deserialize)]
struct TagImageRequest {
    repo: String,
    tag: String,
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn tag_image(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<TagImageRequest>,
) -> Result<StatusCode, ApiError> {
    match parse_runtime(&req.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .image_manager
                .podman
                .tag_image(&id, &req.repo, &req.tag)
                .await
        }
        _ => {
            state
                .image_manager
                .docker
                .tag_image(&id, &req.repo, &req.tag)
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn list_volumes(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<Vec<container::volume::VolumeInfo>>, ApiError> {
    let volumes = match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state.volume_manager.podman.list_volumes().await
        }
        _ => state.volume_manager.docker.list_volumes().await,
    }
    .map_err(ApiError::from)?;
    Ok(Json(volumes))
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Deserialize)]
struct CreateContainerVolumeRequest {
    name: String,
    #[serde(default)]
    driver: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn create_container_volume(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateContainerVolumeRequest>,
) -> Result<Json<container::volume::VolumeInfo>, ApiError> {
    let v = match parse_runtime(&req.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .volume_manager
                .podman
                .create_volume(&req.name, req.driver.as_deref())
                .await
        }
        _ => {
            state
                .volume_manager
                .docker
                .create_volume(&req.name, req.driver.as_deref())
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(Json(v))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn inspect_volume(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let value = match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state.volume_manager.podman.inspect_volume(&name).await
        }
        _ => state.volume_manager.docker.inspect_volume(&name).await,
    }
    .map_err(ApiError::from)?;
    Ok(Json(value))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn remove_volume(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<StatusCode, ApiError> {
    match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state.volume_manager.podman.remove_volume(&name, true).await
        }
        _ => state.volume_manager.docker.remove_volume(&name, true).await,
    }
    .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn list_container_networks(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<Vec<container::cnetwork::NetworkInfo>>, ApiError> {
    let nets = match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state.container_network_manager.podman.list_networks().await
        }
        _ => state.container_network_manager.docker.list_networks().await,
    }
    .map_err(ApiError::from)?;
    Ok(Json(nets))
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Deserialize)]
struct CreateContainerNetworkRequest {
    name: String,
    #[serde(default)]
    driver: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn create_container_network(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateContainerNetworkRequest>,
) -> Result<Json<String>, ApiError> {
    let id = match parse_runtime(&req.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .container_network_manager
                .podman
                .create_network(&req.name, req.driver.as_deref())
                .await
        }
        _ => {
            state
                .container_network_manager
                .docker
                .create_network(&req.name, req.driver.as_deref())
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(Json(id))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn inspect_container_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let value = match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .container_network_manager
                .podman
                .inspect_network(&id)
                .await
        }
        _ => {
            state
                .container_network_manager
                .docker
                .inspect_network(&id)
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(Json(value))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn remove_container_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<StatusCode, ApiError> {
    match parse_runtime(&q.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .container_network_manager
                .podman
                .remove_network(&id)
                .await
        }
        _ => {
            state
                .container_network_manager
                .docker
                .remove_network(&id)
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Deserialize)]
struct NetworkContainerRequest {
    container_id: String,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    force: bool,
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn connect_container_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<NetworkContainerRequest>,
) -> Result<StatusCode, ApiError> {
    match parse_runtime(&req.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .container_network_manager
                .podman
                .connect_container(&id, &req.container_id)
                .await
        }
        _ => {
            state
                .container_network_manager
                .docker
                .connect_container(&id, &req.container_id)
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn disconnect_container_network(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<NetworkContainerRequest>,
) -> Result<StatusCode, ApiError> {
    match parse_runtime(&req.runtime) {
        horcrux_common::ContainerRuntime::Podman => {
            state
                .container_network_manager
                .podman
                .disconnect_container(&id, &req.container_id, req.force)
                .await
        }
        _ => {
            state
                .container_network_manager
                .docker
                .disconnect_container(&id, &req.container_id, req.force)
                .await
        }
    }
    .map_err(ApiError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn list_stacks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<container::stack::StackSummary>>, ApiError> {
    Ok(Json(state.stack_manager.list_stacks().await?))
}

#[cfg(any(feature = "docker", feature = "podman"))]
#[derive(serde::Deserialize)]
struct DeployStackRequest {
    name: String,
    compose_yaml: String,
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn deploy_stack(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeployStackRequest>,
) -> Result<Json<container::stack::StackSummary>, ApiError> {
    let runtime = parse_runtime(&req.runtime);
    let summary = state
        .stack_manager
        .deploy_stack(&req.name, &req.compose_yaml, runtime)
        .await?;
    Ok(Json(summary))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn get_stack(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<container::stack::StackSummary>, ApiError> {
    Ok(Json(state.stack_manager.get_stack(&id).await?))
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn remove_stack(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.stack_manager.remove_stack(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn start_stack(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.stack_manager.start_stack(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(feature = "docker", feature = "podman"))]
async fn stop_stack(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.stack_manager.stop_stack(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
#[derive(serde::Serialize)]
struct WsTicketResponse {
    ticket: String,
    ws_path: String,
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn create_container_logs_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<WsTicketResponse>, ApiError> {
    let runtime = parse_runtime(&q.runtime);
    let target = ws_target(&state, &id, &runtime).await?;
    let ticket = state
        .container_ws_manager
        .create_ticket(
            &target,
            runtime,
            container::ws::ContainerWsKind::Logs,
            Vec::new(),
        )
        .await;
    Ok(Json(WsTicketResponse {
        ws_path: format!("/api/containers/ws/logs/{}", ticket.ticket_id),
        ticket: ticket.ticket_id,
    }))
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
#[derive(serde::Deserialize)]
struct ExecTicketRequest {
    #[serde(default)]
    command: Vec<String>,
    #[serde(default)]
    runtime: Option<String>,
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn create_container_exec_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<ExecTicketRequest>,
) -> Result<Json<WsTicketResponse>, ApiError> {
    let runtime = parse_runtime(&req.runtime);
    let target = ws_target(&state, &id, &runtime).await?;
    let ticket = state
        .container_ws_manager
        .create_ticket(
            &target,
            runtime,
            container::ws::ContainerWsKind::Exec,
            req.command,
        )
        .await;
    Ok(Json(WsTicketResponse {
        ws_path: format!("/api/containers/ws/exec/{}", ticket.ticket_id),
        ticket: ticket.ticket_id,
    }))
}

/// Ticket for a container's interactive system console (LXC, LXD, Incus).
#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn create_container_console_ticket(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<WsTicketResponse>, ApiError> {
    let runtime = parse_runtime(&q.runtime);
    if !matches!(
        runtime,
        horcrux_common::ContainerRuntime::Lxc
            | horcrux_common::ContainerRuntime::Lxd
            | horcrux_common::ContainerRuntime::Incus
    ) {
        return Err(ApiError::ValidationError(format!(
            "{runtime:?} containers have no system console: open the Terminal or Logs instead"
        )));
    }
    let target = ws_target(&state, &id, &runtime).await?;
    let ticket = state
        .container_ws_manager
        .create_ticket(
            &target,
            runtime,
            container::ws::ContainerWsKind::Console,
            Vec::new(),
        )
        .await;
    Ok(Json(WsTicketResponse {
        ws_path: format!("/api/containers/ws/console/{}", ticket.ticket_id),
        ticket: ticket.ticket_id,
    }))
}

/// What a container WebSocket session addresses: LXD/Incus sessions talk to
/// the daemon by instance name (the Horcrux id may differ from it); every
/// other runtime resolves the id itself.
#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn ws_target(
    state: &AppState,
    id: &str,
    runtime: &horcrux_common::ContainerRuntime,
) -> Result<String, ApiError> {
    use horcrux_common::ContainerRuntime as R;
    // A runtime this build doesn't have would otherwise fall through to
    // the docker CLI path in the WebSocket handlers.
    features::require_container_runtime(runtime)?;
    if matches!(runtime, R::Lxd | R::Incus) {
        if let Ok(c) = state.container_manager.get_container(id).await {
            return Ok(c.name);
        }
    }
    Ok(id.to_string())
}

#[cfg(any(
    feature = "docker",
    feature = "podman",
    feature = "lxc",
    feature = "lxd",
    feature = "incus"
))]
async fn create_container_events_ticket(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<RuntimeQuery>,
) -> Result<Json<WsTicketResponse>, ApiError> {
    let ticket = state
        .container_ws_manager
        .create_ticket(
            "*",
            parse_runtime(&q.runtime),
            container::ws::ContainerWsKind::Events,
            Vec::new(),
        )
        .await;
    Ok(Json(WsTicketResponse {
        ws_path: format!("/api/containers/ws/events/{}", ticket.ticket_id),
        ticket: ticket.ticket_id,
    }))
}

// Cluster API handlers

async fn list_cluster_nodes(State(state): State<Arc<AppState>>) -> Json<Vec<Node>> {
    let nodes = state.cluster_manager.list_nodes().await;
    Json(nodes)
}

async fn add_cluster_node(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(request): Json<cluster::node::AddNodeRequest>,
) -> Result<StatusCode, ApiError> {
    let next_id = state
        .cluster_manager
        .list_nodes()
        .await
        .iter()
        .map(|n| n.id)
        .max()
        .unwrap_or(0)
        + 1;
    let node = request.into_node(name, next_id);
    let joined = node.name.clone();
    state.cluster_manager.add_node(node).await?;
    let _ = state
        .webhook_manager
        .trigger_event(
            webhooks::WebhookEventType::NodeJoined,
            serde_json::json!({"node": joined}),
        )
        .await;
    Ok(StatusCode::CREATED)
}

/// `DELETE /api/cluster/nodes/:name` -- 204, 404 for an unknown node, 409
/// for the local node.
async fn remove_cluster_node(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.cluster_manager.remove_node(&name).await?;
    let _ = state
        .webhook_manager
        .trigger_event(
            webhooks::WebhookEventType::NodeLeft,
            serde_json::json!({"node": name}),
        )
        .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/cluster/status` -- cluster-wide summary.
async fn get_cluster_status(
    State(state): State<Arc<AppState>>,
) -> Json<cluster::ClusterStatusReport> {
    let total_vms = state.vm_manager.list_vms().await.len();
    Json(state.cluster_manager.status_report(total_vms).await)
}

/// `GET /api/cluster/local-usage` -- this host's live CPU/memory usage.
/// Polled by the other nodes' usage monitors to fill in `cpu_usage` /
/// `memory_used` on their `GET /api/cluster/nodes`.
async fn get_local_node_usage(
    State(state): State<Arc<AppState>>,
) -> Result<Json<cluster::NodeUsage>, ApiError> {
    state
        .cluster_manager
        .local_usage()
        .await
        .map(Json)
        .ok_or_else(|| {
            ApiError::ServiceUnavailable("Host CPU/memory usage is not readable".to_string())
        })
}

async fn get_cluster_architecture(State(state): State<Arc<AppState>>) -> Json<ArchitectureSummary> {
    let summary = state.cluster_manager.get_architecture_summary().await;
    Json(summary)
}

#[derive(serde::Deserialize)]
struct FindNodeRequest {
    architecture: String,
    memory_mb: u64,
    cpu_cores: u32,
    /// Hard filter: only consider nodes whose placement weight (priority)
    /// is at least this value. Omit to consider all online, resource-
    /// compatible nodes regardless of weight.
    #[serde(default)]
    min_priority: Option<u32>,
}

async fn find_best_node_for_vm(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FindNodeRequest>,
) -> Result<Json<String>, ApiError> {
    // Parse architecture string to enum
    let arch = match request.architecture.as_str() {
        "x86_64" => cluster::node::Architecture::X86_64,
        "aarch64" => cluster::node::Architecture::Aarch64,
        "riscv64" => cluster::node::Architecture::Riscv64,
        "ppc64le" => cluster::node::Architecture::Ppc64le,
        _ => {
            return Err(ApiError::Internal(format!(
                "Unknown architecture: {}",
                request.architecture
            )))
        }
    };

    let node_name = state
        .cluster_manager
        .find_best_node(
            &arch,
            request.memory_mb * 1024 * 1024,
            request.cpu_cores,
            request.min_priority,
        )
        .await?;

    Ok(Json(node_name))
}

#[derive(serde::Deserialize)]
struct SetNodePriorityRequest {
    priority: u32,
}

async fn set_node_priority(
    State(state): State<Arc<AppState>>,
    Path(node_name): Path<String>,
    Json(request): Json<SetNodePriorityRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .cluster_manager
        .set_node_priority(&node_name, request.priority)
        .await?;
    Ok(StatusCode::OK)
}

// Alert API handlers

async fn list_alert_rules(State(state): State<Arc<AppState>>) -> Json<Vec<AlertRule>> {
    let rules = state.alert_manager.list_rules().await;
    Json(rules)
}

async fn create_alert_rule(
    State(state): State<Arc<AppState>>,
    Json(rule): Json<AlertRule>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.add_rule(rule).await?;
    Ok(StatusCode::CREATED)
}

async fn delete_alert_rule(
    State(state): State<Arc<AppState>>,
    Path(rule_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.alert_manager.remove_rule(&rule_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_active_alerts(State(state): State<Arc<AppState>>) -> Json<Vec<Alert>> {
    let alerts = state.alert_manager.get_active_alerts().await;
    Json(alerts)
}

#[derive(serde::Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

async fn get_alert_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(query): axum::extract::Query<HistoryQuery>,
) -> Json<Vec<Alert>> {
    let history = state.alert_manager.get_alert_history(query.limit).await;
    Json(history)
}

#[derive(serde::Deserialize)]
struct AcknowledgeRequest {
    user: String,
}

async fn acknowledge_alert(
    State(state): State<Arc<AppState>>,
    Path((rule_id, target)): Path<(String, String)>,
    Json(req): Json<AcknowledgeRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .alert_manager
        .acknowledge_alert(&rule_id, &target, &req.user)
        .await?;
    Ok(StatusCode::OK)
}

async fn list_notification_channels(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<NotificationChannel>> {
    let channels = state.alert_manager.list_notification_channels().await;
    Json(channels.iter().map(NotificationChannel::masked).collect())
}

async fn add_notification_channel(
    State(state): State<Arc<AppState>>,
    Json(channel): Json<NotificationChannel>,
) -> Result<StatusCode, ApiError> {
    state
        .alert_manager
        .add_notification_channel(channel)
        .await?;
    Ok(StatusCode::CREATED)
}

// OpenTelemetry handlers

async fn get_otel_config(State(state): State<Arc<AppState>>) -> Result<Json<OtelConfig>, ApiError> {
    let config = state.otel_manager.get_config().await;
    Ok(Json(config))
}

async fn update_otel_config(
    State(state): State<Arc<AppState>>,
    Json(config): Json<OtelConfig>,
) -> Result<StatusCode, ApiError> {
    state
        .otel_manager
        .update_config(config.clone())
        .await
        .map_err(ApiError::BadRequest)?;
    // Persisted: it used to be lost at every restart.
    crate::db::config_store::ConfigStore::new(&state.database)
        .put(OTEL_KIND, "default", &config)
        .await?;
    Ok(StatusCode::OK)
}

/// Config-store kind of the OpenTelemetry export settings.
const OTEL_KIND: &str = "otel_config";

/// Outcome of the last OTLP metrics export (periodic or "Export now").
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OtelExportStatus {
    pub last_attempt: Option<String>,
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub exports: u64,
}

fn otel_status() -> &'static std::sync::Mutex<OtelExportStatus> {
    static S: std::sync::OnceLock<std::sync::Mutex<OtelExportStatus>> = std::sync::OnceLock::new();
    S.get_or_init(Default::default)
}

async fn export_otel_once(state: &AppState) -> Result<(), String> {
    let metrics = observability::metrics::MetricsCollector::collect_system_metrics();
    let r = state.otel_manager.export_metrics(metrics).await;
    let now = chrono::Utc::now().to_rfc3339();
    let mut st = otel_status().lock().unwrap();
    st.last_attempt = Some(now.clone());
    match &r {
        Ok(()) => {
            st.last_success = Some(now);
            st.last_error = None;
            st.exports += 1;
        }
        Err(e) => st.last_error = Some(e.clone()),
    }
    r
}

/// Load the saved OpenTelemetry settings and export metrics every
/// `export_interval_secs` while export is enabled.
pub fn start_otel_exporter(state: Arc<AppState>) {
    tokio::spawn(async move {
        if let Some(cfg) = crate::db::config_store::ConfigStore::new(&state.database)
            .load_values::<OtelConfig>(OTEL_KIND)
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
        {
            if let Err(e) = state.otel_manager.update_config(cfg).await {
                tracing::warn!("saved OpenTelemetry settings not applied: {e}");
            }
        }
        loop {
            let cfg = state.otel_manager.get_config().await;
            let wait = cfg.export_interval_secs.max(5);
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            if state.otel_manager.get_config().await.enabled {
                if let Err(e) = export_otel_once(&state).await {
                    tracing::debug!("OTLP metrics export failed: {e}");
                }
            }
        }
    });
}

async fn get_otel_status() -> Json<OtelExportStatus> {
    Json(otel_status().lock().unwrap().clone())
}

async fn export_metrics_now(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiError> {
    export_otel_once(&state)
        .await
        .map_err(ApiError::ServiceUnavailable)?;
    Ok(StatusCode::OK)
}

// Vault handlers

async fn get_vault_config(State(state): State<Arc<AppState>>) -> Json<secrets::VaultConfig> {
    let config = state.vault_manager.get_config().await;
    Json(config)
}

async fn update_vault_config(
    State(state): State<Arc<AppState>>,
    Json(config): Json<secrets::VaultConfig>,
) -> Result<StatusCode, ApiError> {
    state
        .vault_manager
        .initialize(config)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn read_vault_secret(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<Json<secrets::Secret>, ApiError> {
    let secret = state
        .vault_manager
        .read_secret(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(secret))
}

#[derive(serde::Deserialize)]
struct WriteSecretRequest {
    data: std::collections::HashMap<String, String>,
}

async fn write_vault_secret(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
    Json(req): Json<WriteSecretRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .vault_manager
        .write_secret(&path, req.data)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::OK)
}

async fn delete_vault_secret(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .vault_manager
        .delete_secret(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_vault_secrets(
    State(state): State<Arc<AppState>>,
    Path(path): Path<String>,
) -> Result<Json<Vec<String>>, ApiError> {
    let secrets = state
        .vault_manager
        .list_secrets(&path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(secrets))
}

// Storage handlers

async fn list_storage_pools(State(state): State<Arc<AppState>>) -> Json<Vec<storage::StoragePool>> {
    let pools = state.storage_manager.list_pools().await;
    Json(pools)
}

async fn get_storage_pool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<storage::StoragePool>, ApiError> {
    let pool = state
        .storage_manager
        .get_pool(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(pool))
}

#[derive(serde::Deserialize)]
struct AddStoragePoolRequest {
    name: String,
    storage_type: String,
    path: String,
}

async fn add_storage_pool(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddStoragePoolRequest>,
) -> Result<Json<storage::StoragePool>, ApiError> {
    // Parse storage type
    let storage_type = match req.storage_type.to_lowercase().as_str() {
        "zfs" => storage::StorageType::Zfs,
        "ceph" => storage::StorageType::Ceph,
        "lvm" => storage::StorageType::Lvm,
        "iscsi" => storage::StorageType::Iscsi,
        "directory" | "dir" => storage::StorageType::Directory,
        "cifs" => storage::StorageType::Cifs,
        "nfs" => storage::StorageType::Nfs,
        "glusterfs" => storage::StorageType::GlusterFs,
        "btrfs" => storage::StorageType::BtrFs,
        "s3" => storage::StorageType::S3,
        _ => {
            return Err(ApiError::Internal(format!(
                "Unknown storage type: {}",
                req.storage_type
            )))
        }
    };

    let pool = storage::StoragePool {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name,
        storage_type,
        path: req.path,
        available: 0, // Will be updated by backend
        total: 0,     // Will be updated by backend
        enabled: true,
    };

    let pool = state
        .storage_manager
        .add_pool(pool)
        .await
        .map_err(ApiError::from)?;
    let _ = state
        .webhook_manager
        .trigger_event(
            webhooks::WebhookEventType::StoragePoolAdded,
            serde_json::json!({
                "pool_id": pool.id, "name": pool.name, "path": pool.path,
                "storage_type": format!("{:?}", pool.storage_type),
            }),
        )
        .await;
    Ok(Json(pool))
}

async fn remove_storage_pool(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .storage_manager
        .remove_pool(&id)
        .await
        .map_err(ApiError::from)?;
    let _ = state
        .webhook_manager
        .trigger_event(
            webhooks::WebhookEventType::StoragePoolRemoved,
            serde_json::json!({"pool_id": id}),
        )
        .await;
    Ok(StatusCode::OK)
}

// HA (High Availability) handlers

#[cfg(feature = "cluster")]
#[derive(serde::Serialize)]
struct HaResourceResponse {
    vm_id: String,
    vm_name: String,
    group: String,
    priority: u32,
    state: String,
}

#[cfg(feature = "cluster")]
async fn list_ha_resources(State(state): State<Arc<AppState>>) -> Json<Vec<HaResourceResponse>> {
    let resources = state.ha_manager.list_resources().await;

    let mut responses = Vec::new();
    for r in resources.iter() {
        // Try to get actual VM name from database
        let vm_name = state
            .database
            .get_vm(&r.vm_id.to_string())
            .await
            .ok()
            .map(|vm| vm.name)
            .unwrap_or_else(|| format!("vm-{}", r.vm_id));

        responses.push(HaResourceResponse {
            vm_id: r.vm_id.to_string(),
            vm_name,
            group: r.group.clone(),
            priority: r.max_restart, // Using max_restart as priority for now
            state: format!("{:?}", r.state),
        });
    }

    Json(responses)
}

#[cfg(feature = "cluster")]
#[derive(serde::Deserialize)]
struct AddHaResourceRequest {
    vm_id: String,
    group: String,
    priority: u32,
}

#[cfg(feature = "cluster")]
async fn add_ha_resource(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddHaResourceRequest>,
) -> Result<StatusCode, ApiError> {
    // HA manages real VMs: refuse ids that don't exist (it used to take a
    // numeric id that could never match the string ids VMs actually have).
    state.vm_manager.get_vm(&req.vm_id).await?;
    if state.ha_manager.get_resource(&req.vm_id).await.is_some() {
        return Err(ApiError::Conflict(format!(
            "VM {} is already managed by HA",
            req.vm_id
        )));
    }
    let vm_id_for_ha = req.vm_id.clone();
    let config = ha::HaConfig {
        vm_id: req.vm_id,
        group: req.group,
        max_restart: req.priority.min(10), // Use priority as max_restart, cap at 10
        max_relocate: 3,                   // Default max relocations
        state: ha::HaState::Started,
        architecture: Default::default(),
        required_passthrough_devices: Default::default(),
    };

    state.ha_manager.add_resource(config).await?;
    // The resource lives where the VM is; HA only acts once switched on
    // (adding the first resource is the operator asking for it).
    let local = state.cluster_manager.get_local_node_name().await?;
    state
        .ha_manager
        .update_resource_state(&vm_id_for_ha, ha::HaState::Started, Some(local))
        .await?;
    if !state.ha_manager.is_enabled().await {
        state.ha_manager.enable().await;
    }

    Ok(StatusCode::CREATED)
}

#[cfg(feature = "cluster")]
async fn remove_ha_resource(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    if state.ha_manager.get_resource(&vm_id).await.is_none() {
        return Err(ApiError::NotFound(format!(
            "VM {} is not managed by HA",
            vm_id
        )));
    }
    state
        .ha_manager
        .remove_resource(&vm_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

#[cfg(feature = "cluster")]
#[derive(serde::Serialize)]
struct HaStatusResponse {
    total_resources: usize,
    running: usize,
    stopped: usize,
    migrating: usize,
    /// Per-resource state, so the HA dashboard can show what is running.
    resources: Vec<ha::HaResource>,
}

#[cfg(feature = "cluster")]
async fn get_ha_status(State(state): State<Arc<AppState>>) -> Json<HaStatusResponse> {
    let resources = state.ha_manager.list_resources().await;

    let mut running = 0;
    let mut stopped = 0;
    let mut migrating = 0;

    for resource in &resources {
        match resource.state {
            ha::HaState::Started => running += 1,
            ha::HaState::Stopped | ha::HaState::Disabled => stopped += 1,
            ha::HaState::Migrating => migrating += 1,
            _ => {}
        }
    }

    Json(HaStatusResponse {
        total_resources: resources.len(),
        running,
        stopped,
        migrating,
        resources,
    })
}

#[cfg(feature = "cluster")]
#[derive(serde::Deserialize)]
struct CreateHaGroupRequest {
    name: String,
    nodes: Vec<String>,
    #[serde(default)]
    restricted: bool,
    #[serde(default)]
    no_failback: bool,
}

#[cfg(feature = "cluster")]
async fn create_ha_group(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateHaGroupRequest>,
) -> Result<StatusCode, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::ValidationError(
            "HA group name is required".into(),
        ));
    }
    let group = ha::HaGroup {
        name: req.name,
        nodes: req.nodes,
        restricted: req.restricted,
        no_failback: req.no_failback,
    };

    state.ha_manager.add_group(group).await?;

    Ok(StatusCode::CREATED)
}

#[cfg(feature = "cluster")]
#[derive(serde::Deserialize)]
struct UpdateHaGroupRequest {
    nodes: Vec<String>,
    #[serde(default)]
    restricted: bool,
    #[serde(default)]
    no_failback: bool,
}

#[cfg(feature = "cluster")]
async fn update_ha_group(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<UpdateHaGroupRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .ha_manager
        .update_group(ha::HaGroup {
            name,
            nodes: req.nodes,
            restricted: req.restricted,
            no_failback: req.no_failback,
        })
        .await?;
    Ok(StatusCode::OK)
}

#[cfg(feature = "cluster")]
async fn delete_ha_group(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.ha_manager.remove_group(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(feature = "cluster")]
#[derive(serde::Deserialize)]
struct AssignHaResourceRequest {
    resource_id: String,
    #[serde(default = "default_ha_priority")]
    priority: u32,
}

#[cfg(feature = "cluster")]
fn default_ha_priority() -> u32 {
    3
}

#[cfg(feature = "cluster")]
/// Put a VM under HA in this group (the UI's "assign to group" action).
async fn assign_ha_group_resource(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(req): Json<AssignHaResourceRequest>,
) -> Result<StatusCode, ApiError> {
    add_ha_resource(
        State(state),
        Json(AddHaResourceRequest {
            vm_id: req.resource_id,
            group: name,
            priority: req.priority,
        }),
    )
    .await
}

#[cfg(feature = "cluster")]
async fn list_ha_groups(State(state): State<Arc<AppState>>) -> Json<Vec<ha::HaGroup>> {
    let groups = state.ha_manager.list_groups().await;
    Json(groups)
}

// Migration handlers

#[derive(serde::Deserialize)]
struct MigrateVmRequest {
    target_node: String,
    migration_type: Option<String>, // "live", "offline", "online"
    online: Option<bool>,
}

async fn migrate_vm(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
    Json(req): Json<MigrateVmRequest>,
) -> Result<Json<String>, ApiError> {
    // Determine migration type
    let online = req.online.unwrap_or(true);
    let migration_type_str = req.migration_type.unwrap_or_else(|| {
        if online {
            "live".to_string()
        } else {
            "offline".to_string()
        }
    });

    let migration_type = match migration_type_str.as_str() {
        "live" => migration::MigrationType::Live,
        "offline" => migration::MigrationType::Offline,
        "online" => migration::MigrationType::Online,
        _ => {
            return Err(ApiError::BadRequest(format!(
                "Invalid migration type: {}",
                migration_type_str
            )))
        }
    };

    // The VM must exist (migration used to require a numeric id, which no
    // VM created through this API has, so every request failed with a 500).
    state.vm_manager.get_vm(&vm_id).await?;

    // Create migration config
    let config = migration::MigrationConfig {
        vm_id: vm_id.clone(),
        target_node: req.target_node.clone(),
        migration_type,
        bandwidth_limit: None,
        force: false,
        with_local_disks: false,
        timeout_secs: None,
    };

    let source_node = state
        .cluster_manager
        .get_local_node_name()
        .await
        .unwrap_or_else(|_| "localhost".to_string());
    let job_id = api::vm_migration::start(&state, config).await?;

    info!(
        "Started migration of VM {} to node {}, job ID: {}",
        vm_id, req.target_node, job_id
    );

    // Broadcast migration started event
    state
        .ws_state
        .broadcast_migration_started(vm_id.clone(), source_node, req.target_node.clone());

    Ok(Json(job_id))
}

async fn get_migration_status(
    State(state): State<Arc<AppState>>,
    Path(vm_id): Path<String>,
) -> Result<Json<migration::MigrationJob>, ApiError> {
    // Latest migration job for this VM (jobs are keyed by job id, so the
    // old `get_job(vm_id)` lookup could never find anything).
    let job = state
        .migration_manager
        .latest_job_for_vm(&vm_id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("No migration found for VM {}", vm_id)))?;

    Ok(Json(job))
}

// GPU Passthrough Endpoints

async fn list_gpu_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<gpu::GpuDevice>>, ApiError> {
    let devices = state.gpu_manager.list_devices().await;
    Ok(Json(devices))
}

async fn scan_gpu_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<gpu::GpuDevice>>, ApiError> {
    let devices = state
        .gpu_manager
        .scan_devices()
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(devices))
}

async fn get_gpu_device(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<gpu::GpuDevice>, ApiError> {
    let device = state
        .gpu_manager
        .get_device(&pci_address)
        .await
        .map_err(|e| ApiError::NotFound(e.to_string()))?;
    Ok(Json(device))
}

async fn bind_gpu_to_vfio(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state
        .gpu_manager
        .bind_to_vfio(&pci_address)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Bound GPU {} to vfio-pci", pci_address);
    Ok(Json("GPU bound to vfio-pci driver"))
}

async fn unbind_gpu_from_vfio(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state
        .gpu_manager
        .unbind_from_vfio(&pci_address)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    info!("Unbound GPU {} from vfio-pci", pci_address);
    Ok(Json("GPU unbound from vfio-pci driver"))
}

async fn get_gpu_iommu_group(
    State(state): State<Arc<AppState>>,
    Path(pci_address): Path<String>,
) -> Result<Json<Vec<gpu::GpuDevice>>, ApiError> {
    let devices = state
        .gpu_manager
        .get_iommu_group_devices(&pci_address)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(devices))
}

#[derive(Serialize)]
struct IommuStatus {
    enabled: bool,
    message: String,
}

async fn check_iommu_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<IommuStatus>, ApiError> {
    let enabled = state.gpu_manager.check_iommu_enabled().await;

    let message = if enabled {
        "IOMMU is enabled and ready for GPU passthrough".to_string()
    } else {
        "IOMMU is not enabled. Add intel_iommu=on or amd_iommu=on to kernel parameters".to_string()
    };

    Ok(Json(IommuStatus { enabled, message }))
}

// Prometheus Metrics Endpoint

async fn prometheus_metrics(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Fill the gauges from the live sources, then export
    api::prometheus_export::refresh(&state).await;
    let metrics = state.prometheus_manager.export_metrics().await;

    // Return as plain text with Prometheus content type
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        metrics,
    )
}

// Webhook Endpoints

// Responses carry the masked configuration (no secret, no header values)
// plus delivery counters: reading webhooks only needs Sys.Audit.
async fn list_webhooks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<webhooks::WebhookSummary>>, ApiError> {
    Ok(Json(state.webhook_manager.summaries().await))
}

async fn create_webhook(
    State(state): State<Arc<AppState>>,
    Json(config): Json<webhooks::WebhookConfig>,
) -> Result<Json<webhooks::WebhookConfig>, ApiError> {
    let webhook = state
        .webhook_manager
        .add_webhook(config)
        .await
        .map_err(ApiError::from)?;

    info!("Created webhook: {} ({})", webhook.name, webhook.id);
    Ok(Json(webhook.masked()))
}

async fn get_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<webhooks::WebhookSummary>, ApiError> {
    let webhook = state
        .webhook_manager
        .summary(&id)
        .await
        .ok_or_else(|| ApiError::NotFound(format!("Webhook {} not found", id)))?;

    Ok(Json(webhook))
}

async fn update_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(config): Json<webhooks::WebhookConfig>,
) -> Result<Json<&'static str>, ApiError> {
    state
        .webhook_manager
        .update_webhook(&id, config)
        .await
        .map_err(ApiError::from)?;

    info!("Updated webhook: {}", id);
    Ok(Json("Webhook updated successfully"))
}

async fn delete_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<&'static str>, ApiError> {
    state
        .webhook_manager
        .remove_webhook(&id)
        .await
        .map_err(ApiError::from)?;

    info!("Deleted webhook: {}", id);
    Ok(Json("Webhook deleted successfully"))
}

/// Send one test event to this webhook (404 if there is none) and return
/// the delivery record. It used to broadcast a `custom: test` event to
/// whichever webhooks subscribed to it - usually none - and answer
/// "Test webhook sent" either way.
async fn test_webhook(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<webhooks::WebhookDelivery>, ApiError> {
    let delivery = state
        .webhook_manager
        .test_webhook(&id)
        .await
        .map_err(ApiError::from)?;
    Ok(Json(delivery))
}

async fn get_webhook_deliveries(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<webhooks::WebhookDelivery>>, ApiError> {
    let deliveries = state.webhook_manager.get_deliveries(Some(&id), 50).await;
    Ok(Json(deliveries))
}

// ============================================================================
// Kubernetes API handlers (conditionally compiled)
// ============================================================================

#[cfg(feature = "kubernetes")]
mod k8s_handlers;

// Re-export handlers for use in routes
#[cfg(feature = "kubernetes")]
use k8s_handlers::{
    connect_cluster as k8s_connect_cluster,
    cordon_node as k8s_cordon_node,
    create_configmap as k8s_create_configmap,
    create_ingress as k8s_create_ingress,
    create_namespace as k8s_create_namespace,
    create_pvc as k8s_create_pvc,
    create_secret as k8s_create_secret,
    create_service as k8s_create_service,
    delete_cluster as k8s_delete_cluster,
    delete_configmap as k8s_delete_configmap,
    delete_cronjob as k8s_delete_cronjob,
    delete_daemonset as k8s_delete_daemonset,
    delete_ingress as k8s_delete_ingress,
    delete_job as k8s_delete_job,
    delete_namespace as k8s_delete_namespace,
    delete_pod as k8s_delete_pod,
    delete_pvc as k8s_delete_pvc,
    delete_secret as k8s_delete_secret,
    delete_service as k8s_delete_service,
    delete_statefulset as k8s_delete_statefulset,
    drain_node as k8s_drain_node,
    get_cluster as k8s_get_cluster,
    get_cluster_health as k8s_get_cluster_health,
    get_cluster_version as k8s_get_cluster_version,
    get_configmap as k8s_get_configmap,
    get_cronjob as k8s_get_cronjob,
    get_daemonset as k8s_get_daemonset,
    get_deployment as k8s_get_deployment,
    get_ingress as k8s_get_ingress,
    get_job as k8s_get_job,
    get_namespace as k8s_get_namespace,
    get_node as k8s_get_node,
    // Metrics
    get_node_metrics as k8s_get_node_metrics,
    get_pod as k8s_get_pod,
    get_pod_logs as k8s_get_pod_logs,
    get_pod_metrics as k8s_get_pod_metrics,
    get_pvc as k8s_get_pvc,
    get_secret as k8s_get_secret,
    get_service as k8s_get_service,
    get_statefulset as k8s_get_statefulset,
    // Cluster
    list_clusters as k8s_list_clusters,
    // ConfigMaps
    list_configmaps as k8s_list_configmaps,
    list_cronjobs as k8s_list_cronjobs,
    // DaemonSets
    list_daemonsets as k8s_list_daemonsets,
    // Deployments
    list_deployments as k8s_list_deployments,
    // Events
    list_events as k8s_list_events,
    // Helm
    // Ingress
    list_ingresses as k8s_list_ingresses,
    // Jobs
    list_jobs as k8s_list_jobs,
    list_namespace_events as k8s_list_namespace_events,
    // Namespaces
    list_namespaces as k8s_list_namespaces,
    // Nodes
    list_nodes as k8s_list_nodes,
    // Pods
    list_pods as k8s_list_pods,
    // PVCs
    list_pvcs as k8s_list_pvcs,
    // Secrets
    list_secrets as k8s_list_secrets,
    // Services
    list_services as k8s_list_services,
    // StatefulSets
    list_statefulsets as k8s_list_statefulsets,
    reconnect_cluster as k8s_reconnect_cluster,
    restart_deployment as k8s_restart_deployment,
    scale_deployment as k8s_scale_deployment,
    scale_statefulset as k8s_scale_statefulset,
    uncordon_node as k8s_uncordon_node,
};
