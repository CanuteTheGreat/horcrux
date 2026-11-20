//! Horcrux Console HTTP API and signalling (docs/design/horcrux-console.md).
//!
//! * `GET|PUT /api/console/settings`: the node's `[console]` settings
//!   (Sys.Audit / Sys.Modify on `/`).
//! * `GET /api/console/:vm/hxc`: the browser client.
//! * `GET /api/console/:vm/hxc/status`: can this VM be viewed, and why not.
//! * `POST /api/console/:vm/hxc/ticket` `{conn_id, profile?}`: a console
//!   ticket for the caller, bound to their API WebSocket connection; the
//!   input/clipboard/microphone rights are taken from their privileges
//!   (`VM.Console.Input`, `.Clipboard`, `.Audio.Mic`) when it is issued.
//! * `POST /api/console/:vm/hxc/local-ticket` `{uid, profile?}`: a ticket
//!   for the native viewer on this node, redeemable once on the local
//!   socket (`[console] local_socket`) by a peer with that Unix uid; it
//!   hands over the VM's frame ring (raw frames, no encode) and takes input.
//! * `GET /api/console/:vm/hxc/sessions`, `DELETE .../sessions/:id`.
//! * `GET /api/vms/:id/console/guest-tools` (VM.Audit): can the node
//!   install the Horcrux guest tools into this VM (guest agent channel,
//!   tools CD attached), are they in use, the latest install.
//! * `POST /api/vms/:id/console/guest-tools/install` (VM.Config): install
//!   them now through the QEMU guest agent (Windows MSI, Linux
//!   .deb/.rpm/.apk from the tools CD); 202, then poll the GET. Audited.
//! * WebSocket `/api/ws` message `{"type":"console.offer","ticket","sdp"}`:
//!   redeems the ticket (same user, same connection, once, before expiry)
//!   and answers `ConsoleAnswer {session, sdp}` or `ConsoleError {message}`.
//!
//! Every route sits under `/api/console/<vm>`, which RBAC already maps to
//! `VM.Console` on `/vms/<vm>`.
use crate::console::hxc_settings;
use crate::error::ApiError;
use crate::AppState;
use axum::{extract::State, routing::get, Extension, Json, Router};
use horcrux_common::console::ConsoleSettings;
use std::sync::Arc;

pub fn routes() -> Router<Arc<AppState>> {
    let r = Router::new().route("/api/console/settings", get(get_settings).put(put_settings));
    #[cfg(feature = "qemu")]
    let r = r
        .route("/api/console/:vm_id/hxc", get(qemu::page))
        .route("/api/console/:vm_id/hxc/status", get(qemu::status))
        .route(
            "/api/console/:vm_id/hxc/ticket",
            axum::routing::post(qemu::ticket),
        )
        .route(
            "/api/console/:vm_id/hxc/local-ticket",
            axum::routing::post(qemu::local_ticket),
        )
        .route("/api/console/:vm_id/hxc/sessions", get(qemu::sessions))
        .route(
            "/api/console/:vm_id/hxc/sessions/:session_id",
            axum::routing::delete(qemu::stop_session),
        )
        .route(
            "/api/vms/:id/console/guest-tools",
            get(qemu::guest_tools_status),
        )
        .route(
            "/api/vms/:id/console/guest-tools/install",
            axum::routing::post(qemu::guest_tools_install),
        );
    r
}

async fn get_settings() -> Json<ConsoleSettings> {
    Json(hxc_settings::get())
}

async fn put_settings(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<crate::middleware::auth::AuthUser>>,
    Json(new): Json<ConsoleSettings>,
) -> Result<Json<ConsoleSettings>, ApiError> {
    let saved = hxc_settings::set(new).map_err(ApiError::ValidationError)?;
    state
        .audit_logger
        .log(crate::audit::AuditEvent {
            timestamp: chrono::Utc::now(),
            event_type: crate::audit::AuditEventType::ConfigChanged,
            severity: crate::audit::AuditSeverity::Info,
            user: user.map(|Extension(u)| u.username),
            source_ip: None,
            resource: Some("console".into()),
            action: "console settings changed".into(),
            result: crate::audit::AuditResult::Success,
            details: None,
            session_id: None,
        })
        .await;
    Ok(Json(saved))
}

/// Serve the same-host viewer socket (`[console] local_socket`).
#[cfg(feature = "qemu")]
pub async fn start_local_socket(state: &Arc<AppState>) {
    use crate::console::hxc::local::{self, LocalEvent};
    let path = std::path::PathBuf::from(hxc_settings::get().local_socket);
    let vms = state.vm_manager.clone();
    let resolve: local::Resolve = Arc::new(move |vm_id: String| {
        let vms = vms.clone();
        Box::pin(async move {
            let vm = vms.get_vm(&vm_id).await.map_err(|e| e.to_string())?;
            crate::console::runtime_disk(&vm).map_err(|e| e.to_string())
        })
    });
    let audit = state.audit_logger.clone();
    let on_event: local::OnEvent = Arc::new(move |ev: LocalEvent| {
        if !hxc_settings::get().audit_sessions {
            return;
        }
        use crate::audit::{AuditEvent, AuditEventType, AuditResult, AuditSeverity};
        let (kind, ok, user, vm, action, details, session) = match ev {
            LocalEvent::Started {
                user,
                vm,
                session,
                details,
            } => (
                AuditEventType::ConsoleSessionStarted,
                true,
                Some(user),
                Some(vm),
                "local console session started".to_string(),
                details,
                Some(session),
            ),
            LocalEvent::Ended {
                user,
                vm,
                session,
                details,
            } => (
                AuditEventType::ConsoleSessionEnded,
                true,
                Some(user),
                Some(vm),
                "local console session ended".to_string(),
                details,
                Some(session),
            ),
            LocalEvent::Refused { reason, uid } => (
                AuditEventType::ConsoleAccessDenied,
                false,
                None,
                None,
                "local console ticket refused".to_string(),
                format!("{reason} (uid {uid})"),
                None,
            ),
        };
        let audit = audit.clone();
        tokio::spawn(async move {
            audit
                .log(AuditEvent {
                    timestamp: chrono::Utc::now(),
                    event_type: kind,
                    severity: if ok {
                        AuditSeverity::Info
                    } else {
                        AuditSeverity::Warning
                    },
                    user,
                    source_ip: None,
                    resource: vm,
                    action,
                    result: if ok {
                        AuditResult::Success
                    } else {
                        AuditResult::Failure
                    },
                    details: Some(details),
                    session_id: session,
                })
                .await;
        });
    });
    if let Err(e) = local::serve(path, qemu::broker(), hxc_settings::get, resolve, on_event).await {
        tracing::warn!("Horcrux Console local socket not available: {e}");
    }
}

/// The `console.*` WebSocket side channel.
#[cfg(feature = "qemu")]
pub fn side_channel(state: &AppState) -> Arc<dyn crate::websocket::WsSideChannel> {
    Arc::new(qemu::Signalling {
        vm_manager: state.vm_manager.clone(),
        audit: state.audit_logger.clone(),
    })
}

#[cfg(feature = "qemu")]
mod qemu {
    use super::*;
    use crate::audit::{AuditEvent, AuditEventType, AuditLogger, AuditResult, AuditSeverity};
    use crate::console::hxc::{self, session, Broker, HxcTicket};
    use crate::middleware::auth::AuthUser;
    use axum::{extract::Path, http::HeaderMap};
    use horcrux_common::auth::Privilege;
    use horcrux_common::console::ConsoleProfile;
    use horcrux_common::VmHypervisor;
    use std::sync::OnceLock;

    /// The node's console broker (tickets and live sessions).
    pub fn broker() -> &'static Broker {
        static B: OnceLock<Broker> = OnceLock::new();
        B.get_or_init(Broker::new)
    }

    async fn vm_disk(
        state: &AppState,
        vm_id: &str,
    ) -> Result<Option<std::path::PathBuf>, ApiError> {
        let vm = state.vm_manager.get_vm(vm_id).await?;
        if vm.hypervisor != VmHypervisor::Qemu {
            return Ok(None);
        }
        Ok(crate::console::runtime_disk(&vm).ok())
    }

    pub async fn page(
        State(state): State<Arc<AppState>>,
        Path(vm_id): Path<String>,
    ) -> Result<axum::response::Html<String>, ApiError> {
        // 404 for unknown VMs, like the other console pages
        state.vm_manager.get_vm(&vm_id).await?;
        Ok(axum::response::Html(hxc::page::html(&vm_id)))
    }

    pub async fn status(
        State(state): State<Arc<AppState>>,
        Path(vm_id): Path<String>,
    ) -> Result<Json<hxc::Availability>, ApiError> {
        let disk = vm_disk(&state, &vm_id).await?;
        Ok(Json(hxc::availability(&hxc_settings::get(), disk).await))
    }

    #[derive(serde::Deserialize)]
    pub struct TicketRequest {
        conn_id: String,
        #[serde(default)]
        profile: Option<ConsoleProfile>,
    }

    #[derive(serde::Serialize)]
    pub struct TicketResponse {
        ticket: String,
        expires_at: i64,
        profile: ConsoleProfile,
        input: bool,
        clipboard: bool,
        microphone: bool,
    }

    pub async fn ticket(
        State(state): State<Arc<AppState>>,
        Path(vm_id): Path<String>,
        headers: HeaderMap,
        user: Option<Extension<AuthUser>>,
        principal: Option<Extension<crate::auth::store::Principal>>,
        Json(req): Json<TicketRequest>,
    ) -> Result<Json<TicketResponse>, ApiError> {
        let Extension(user) = user.ok_or(ApiError::AuthenticationFailed)?;
        let settings = hxc_settings::get();
        let disk = vm_disk(&state, &vm_id).await?;
        let avail = hxc::availability(&settings, disk).await;
        if let Some(reason) = avail.reason {
            return Err(ApiError::ServiceUnavailable(reason));
        }
        if req.conn_id.trim().is_empty() || req.conn_id.len() > 64 {
            return Err(ApiError::BadRequest("conn_id is required".into()));
        }
        let acl_path = format!("/vms/{vm_id}");
        // Cluster service tokens carry no principal; they never get input.
        let has = |p: Privilege| {
            principal
                .as_ref()
                .map(|Extension(pr)| pr.has(p, &acl_path))
                .unwrap_or(false)
        };
        let host = headers
            .get(axum::http::header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_string);
        let ips = hxc::candidate_ips(&settings, host.as_deref()).await;
        let t = broker()
            .issue(
                HxcTicket {
                    ticket: String::new(),
                    vm_id: vm_id.clone(),
                    user_id: user.user_id.clone(),
                    username: user.username.clone(),
                    conn_id: req.conn_id,
                    profile: req.profile.unwrap_or(settings.default_profile),
                    allow_input: has(Privilege::VmConsoleInput),
                    allow_clipboard: has(Privilege::VmConsoleClipboard),
                    allow_mic: has(Privilege::VmConsoleAudioMic),
                    candidate_ips: ips,
                    expires_at: 0,
                },
                settings.ticket_lifetime_secs,
            )
            .await;
        Ok(Json(TicketResponse {
            ticket: t.ticket,
            expires_at: t.expires_at,
            profile: t.profile,
            input: t.allow_input,
            clipboard: t.allow_clipboard,
            microphone: t.allow_mic,
        }))
    }

    #[derive(serde::Deserialize)]
    pub struct LocalTicketRequest {
        /// Unix uid of the viewer process that will redeem the ticket.
        uid: u32,
        #[serde(default)]
        profile: Option<ConsoleProfile>,
    }

    #[derive(serde::Serialize)]
    pub struct LocalTicketResponse {
        ticket: String,
        expires_at: i64,
        profile: ConsoleProfile,
        input: bool,
        socket: String,
    }

    pub async fn local_ticket(
        State(state): State<Arc<AppState>>,
        Path(vm_id): Path<String>,
        user: Option<Extension<AuthUser>>,
        principal: Option<Extension<crate::auth::store::Principal>>,
        Json(req): Json<LocalTicketRequest>,
    ) -> Result<Json<LocalTicketResponse>, ApiError> {
        let Extension(user) = user.ok_or(ApiError::AuthenticationFailed)?;
        let settings = hxc_settings::get();
        if !settings.enabled {
            return Err(ApiError::ServiceUnavailable(
                "The Horcrux Console is turned off on this node (Settings > Console)".into(),
            ));
        }
        let disk = vm_disk(&state, &vm_id).await?.ok_or_else(|| {
            ApiError::ServiceUnavailable("Only QEMU VMs have the Horcrux Console".into())
        })?;
        if !hxc::vm_dbus_display(&disk).await {
            return Err(ApiError::ServiceUnavailable(
                "This VM is not running with the D-Bus display; start (or restart) it".into(),
            ));
        }
        let acl_path = format!("/vms/{vm_id}");
        let has = |p: Privilege| {
            principal
                .as_ref()
                .map(|Extension(pr)| pr.has(p, &acl_path))
                .unwrap_or(false)
        };
        let t = broker()
            .issue(
                HxcTicket {
                    ticket: String::new(),
                    vm_id: vm_id.clone(),
                    user_id: user.user_id.clone(),
                    username: user.username.clone(),
                    conn_id: format!("local:{}", req.uid),
                    profile: req.profile.unwrap_or(settings.default_profile),
                    allow_input: has(Privilege::VmConsoleInput),
                    allow_clipboard: has(Privilege::VmConsoleClipboard),
                    allow_mic: has(Privilege::VmConsoleAudioMic),
                    candidate_ips: Vec::new(),
                    expires_at: 0,
                },
                settings.ticket_lifetime_secs,
            )
            .await;
        Ok(Json(LocalTicketResponse {
            ticket: t.ticket,
            expires_at: t.expires_at,
            profile: t.profile,
            input: t.allow_input,
            socket: settings.local_socket,
        }))
    }

    pub async fn sessions(Path(vm_id): Path<String>) -> Json<Vec<hxc::SessionInfo>> {
        Json(broker().sessions(Some(&vm_id)).await)
    }

    pub async fn stop_session(
        Path((vm_id, session_id)): Path<(String, String)>,
    ) -> Result<axum::http::StatusCode, ApiError> {
        if broker().stop(Some(&vm_id), Some(&session_id)).await == 0 {
            return Err(ApiError::NotFound(format!("console session {session_id}")));
        }
        Ok(axum::http::StatusCode::NO_CONTENT)
    }

    #[derive(serde::Serialize)]
    pub struct GuestToolsStatus {
        vm: String,
        running: bool,
        /// The VM has the QEMU guest agent channel and the agent answers.
        guest_agent: bool,
        guest_os: Option<String>,
        /// The guest tools CD is attached to this run of the VM.
        tools_cd: bool,
        /// The guest-tools port exists in this run.
        tools_port: bool,
        /// A live console session uses the tools (their OS and version).
        in_use: Option<String>,
        auto_install: bool,
        /// Why an install can't run now (None = it can).
        blocked: Option<String>,
        last_install: Option<hxc::deliver::InstallState>,
    }

    async fn tools_status(state: &AppState, vm_id: &str) -> Result<GuestToolsStatus, ApiError> {
        let vm = state.vm_manager.get_vm(vm_id).await?;
        let settings = hxc_settings::get();
        let mut st = GuestToolsStatus {
            vm: vm_id.to_string(),
            running: false,
            guest_agent: false,
            guest_os: None,
            tools_cd: false,
            tools_port: false,
            in_use: None,
            auto_install: settings.guest_tools_auto_install,
            blocked: None,
            last_install: hxc::deliver::last_install(vm_id),
        };
        if vm.hypervisor != VmHypervisor::Qemu {
            st.blocked = Some("the Horcrux guest tools are for QEMU VMs".into());
            return Ok(st);
        }
        let disk = crate::console::runtime_disk(&vm).ok();
        let rt = match &disk {
            Some(d) => crate::vm::display::DisplayRuntime::load(d).await.ok(),
            None => None,
        };
        st.running = crate::vm::qemu::QemuManager::process_running(&vm.id) && rt.is_some();
        if let (Some(d), Some(rt)) = (&disk, &rt) {
            st.tools_cd = rt.guest_tools_iso.is_some();
            st.tools_port = rt.guest_tools_socket.is_some();
            let qga = crate::vm::display::DisplayRuntime::qga_socket(d);
            if st.running && qga.exists() {
                if let Ok(os) =
                    crate::vm::devices::qga_exec(&qga, "guest-get-osinfo", serde_json::json!({}), 5)
                        .await
                {
                    st.guest_agent = true;
                    st.guest_os = os
                        .get("pretty-name")
                        .or_else(|| os.get("name"))
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                }
            }
        }
        st.in_use = broker()
            .sessions(Some(vm_id))
            .await
            .into_iter()
            .find_map(|s| s.stats.guest_tools.clone());
        st.blocked = if !st.running {
            Some("the VM is not running".into())
        } else if !st.tools_cd {
            Some("this run of the VM has no guest tools CD (the node needs the guest tools image, [console] guest_tools_iso; restart the VM after installing it)".into())
        } else if !st.guest_agent {
            Some("the QEMU guest agent does not answer (enable \"QEMU guest agent\" in the VM's hardware and install qemu-guest-agent / the virtio-win guest agent in the guest)".into())
        } else if st
            .last_install
            .as_ref()
            .map(|l| l.state == "installing")
            .unwrap_or(false)
        {
            Some("an install is already running".into())
        } else {
            None
        };
        Ok(st)
    }

    pub async fn guest_tools_status(
        State(state): State<Arc<AppState>>,
        Path(vm_id): Path<String>,
    ) -> Result<Json<GuestToolsStatus>, ApiError> {
        Ok(Json(tools_status(&state, &vm_id).await?))
    }

    pub async fn guest_tools_install(
        State(state): State<Arc<AppState>>,
        Path(vm_id): Path<String>,
        user: Option<Extension<AuthUser>>,
    ) -> Result<(axum::http::StatusCode, Json<GuestToolsStatus>), ApiError> {
        let Extension(user) = user.ok_or(ApiError::AuthenticationFailed)?;
        let st = tools_status(&state, &vm_id).await?;
        if let Some(reason) = &st.blocked {
            return Err(ApiError::Conflict(reason.clone()));
        }
        let vm = state.vm_manager.get_vm(&vm_id).await?;
        let disk = crate::console::runtime_disk(&vm)?;
        let qga = crate::vm::display::DisplayRuntime::qga_socket(&disk);
        let audit = state.audit_logger.clone();
        audit
            .log(event(
                AuditEventType::GuestToolsInstall,
                true,
                &user.username,
                &vm_id,
                "guest tools install requested".into(),
                st.guest_os.clone(),
                None,
            ))
            .await;
        let (vm2, name) = (vm_id.clone(), user.username.clone());
        let timeout = hxc_settings::get().guest_tools_install_timeout_secs.into();
        tokio::spawn(async move {
            let r = hxc::deliver::install_recorded(&vm2, &qga, &name, timeout).await;
            let (ok, detail) = match &r {
                Ok(rep) => (
                    rep.ok,
                    format!("{} (exit {:?})", rep.message, rep.exit_code),
                ),
                Err(e) => (false, e.clone()),
            };
            audit
                .log(event(
                    AuditEventType::GuestToolsInstall,
                    ok,
                    &name,
                    &vm2,
                    if ok {
                        "guest tools installed".into()
                    } else {
                        "guest tools install failed".into()
                    },
                    Some(detail),
                    None,
                ))
                .await;
        });
        // let the record show "installing" before answering
        let st = tools_status(&state, &vm_id).await?;
        Ok((axum::http::StatusCode::ACCEPTED, Json(st)))
    }

    pub struct Signalling {
        pub vm_manager: Arc<crate::vm::VmManager>,
        pub audit: Arc<AuditLogger>,
    }

    fn event(
        kind: AuditEventType,
        ok: bool,
        user: &str,
        vm: &str,
        action: String,
        details: Option<String>,
        session: Option<String>,
    ) -> AuditEvent {
        AuditEvent {
            timestamp: chrono::Utc::now(),
            event_type: kind,
            severity: if ok {
                AuditSeverity::Info
            } else {
                AuditSeverity::Warning
            },
            user: Some(user.to_string()),
            source_ip: None,
            resource: Some(vm.to_string()),
            action,
            result: if ok {
                AuditResult::Success
            } else {
                AuditResult::Failure
            },
            details,
            session_id: session,
        }
    }

    fn error(message: impl Into<String>) -> Option<serde_json::Value> {
        Some(serde_json::json!({"type": "ConsoleError", "data": {"message": message.into()}}))
    }

    impl Signalling {
        async fn offer(
            &self,
            user: AuthUser,
            conn_id: String,
            msg: serde_json::Value,
        ) -> Option<serde_json::Value> {
            let ticket = msg.get("ticket").and_then(|t| t.as_str()).unwrap_or("");
            let sdp = msg.get("sdp").and_then(|t| t.as_str()).unwrap_or("");
            if sdp.is_empty() || sdp.len() > 64 * 1024 {
                return error("an SDP offer is required");
            }
            let settings = hxc_settings::get();
            let t = match broker().redeem(ticket, &user.user_id, &conn_id).await {
                Ok(t) => t,
                Err(e) => {
                    if settings.audit_sessions {
                        self.audit
                            .log(event(
                                AuditEventType::ConsoleAccessDenied,
                                false,
                                &user.username,
                                "",
                                "console ticket refused".into(),
                                Some(e.to_string()),
                                None,
                            ))
                            .await;
                    }
                    return error(e.to_string());
                }
            };
            if broker().live_sessions_for(&t.vm_id).await
                >= settings.max_sessions_per_guest as usize
            {
                return error(format!(
                    "this VM already has {} console sessions (max_sessions_per_guest)",
                    settings.max_sessions_per_guest
                ));
            }
            let vm = match self.vm_manager.get_vm(&t.vm_id).await {
                Ok(v) => v,
                Err(e) => return error(e.to_string()),
            };
            let disk = match crate::console::runtime_disk(&vm) {
                Ok(d) => d,
                Err(e) => return error(e.to_string()),
            };
            let rt = match crate::vm::display::DisplayRuntime::load(&disk).await {
                Ok(rt) if rt.dbus_display => rt,
                Ok(_) => return error("This VM is not running with the D-Bus display; restart it"),
                Err(e) => return error(e.to_string()),
            };
            let session_id = uuid::Uuid::new_v4().simple().to_string();
            let req = session::SessionRequest {
                session_id: session_id.clone(),
                vm_id: t.vm_id.clone(),
                user: t.username.clone(),
                profile: t.profile,
                allow_input: t.allow_input,
                encoder_caps: hxc::encode::caps(&settings).await,
                settings: settings.clone(),
                qmp_socket: rt.qmp_socket,
                candidate_ips: t.candidate_ips.clone(),
                offer_sdp: sdp.to_string(),
                allow_clipboard: t.allow_clipboard,
                guest_tools_socket: rt.guest_tools_socket.clone(),
                serial_socket: Some(rt.serial_socket.clone()),
                qga_socket: Some(crate::vm::display::DisplayRuntime::qga_socket(&disk)),
                guest_tools_cd: rt.guest_tools_iso.is_some(),
            };
            match session::start(req).await {
                Ok((handle, answer)) => {
                    let handle = broker().add_session(handle).await;
                    if settings.audit_sessions {
                        self.audit
                            .log(event(
                                AuditEventType::ConsoleSessionStarted,
                                true,
                                &t.username,
                                &t.vm_id,
                                format!("console session started ({})", t.profile.name()),
                                Some(format!(
                                    "input={} clipboard={} path=direct candidates={:?}",
                                    t.allow_input, t.allow_clipboard, t.candidate_ips
                                )),
                                Some(session_id.clone()),
                            ))
                            .await;
                        let audit = self.audit.clone();
                        let (user, vm, sid) =
                            (t.username.clone(), t.vm_id.clone(), session_id.clone());
                        tokio::spawn(async move {
                            let mut done = handle.done.clone();
                            while !*done.borrow() {
                                if done.changed().await.is_err() {
                                    break;
                                }
                            }
                            let s = handle.stats.lock().map(|s| s.clone()).unwrap_or_default();
                            audit
                                .log(event(
                                    AuditEventType::ConsoleSessionEnded,
                                    true,
                                    &user,
                                    &vm,
                                    "console session ended".into(),
                                    Some(format!(
                                        "reason={} frames={} bytes={} keys={} pointer={} input_denied={}",
                                        s.end_reason.unwrap_or_default(),
                                        s.frames_sent,
                                        s.bytes_sent,
                                        s.keys_in,
                                        s.pointer_in,
                                        s.input_denied
                                    )),
                                    Some(sid),
                                ))
                                .await;
                        });
                    }
                    Some(serde_json::json!({"type": "ConsoleAnswer",
                        "data": {"session": session_id, "sdp": answer}}))
                }
                Err(e) => error(e),
            }
        }
    }

    impl crate::websocket::WsSideChannel for Signalling {
        fn handle(
            &self,
            user: AuthUser,
            conn_id: String,
            message: serde_json::Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>>
        {
            let this = Signalling {
                vm_manager: self.vm_manager.clone(),
                audit: self.audit.clone(),
            };
            Box::pin(async move {
                match message.get("type").and_then(|t| t.as_str()) {
                    Some("console.offer") => this.offer(user, conn_id, message).await,
                    Some("console.stop") => {
                        let sid = message
                            .get("session")
                            .and_then(|s| s.as_str())
                            .unwrap_or("");
                        // only the session's own user may stop it this way
                        if let Some(s) = broker().get(sid).await {
                            if s.user == user.username {
                                s.stop();
                            }
                        }
                        None
                    }
                    _ => error("unknown console message"),
                }
            })
        }
    }
}
