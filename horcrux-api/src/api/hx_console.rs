//! Horcrux Console HTTP API and signalling (docs/design/horcrux-console.md).
//!
//! * `GET|PUT /api/console/settings`: the node's `[console]` settings
//!   (Sys.Audit / Sys.Modify on `/`).
//! * `GET /api/console/relay`: the node's relay (WAN access) and port
//!   mapping status (Sys.Audit on `/`).
//! * `GET /api/console/:vm/hxc`: the browser client.
//! * `GET /api/console/:vm/hxc/status`: can this VM be viewed, and why not.
//! * `POST /api/console/:vm/hxc/ticket` `{conn_id, profile?}`: a console
//!   ticket for the caller, bound to their API WebSocket connection; the
//!   input/clipboard/microphone rights are taken from their privileges
//!   (`VM.Console.Input`, `.Clipboard`, `.Audio.Mic`) when it is issued.
//! * `GET /api/console/:vm/hxc/sessions`, `DELETE .../sessions/:id`.
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
        .route("/api/console/relay", get(qemu::relay_status))
        .route("/api/console/:vm_id/hxc", get(qemu::page))
        .route("/api/console/:vm_id/hxc/status", get(qemu::status))
        .route(
            "/api/console/:vm_id/hxc/ticket",
            axum::routing::post(qemu::ticket),
        )
        .route("/api/console/:vm_id/hxc/sessions", get(qemu::sessions))
        .route(
            "/api/console/:vm_id/hxc/sessions/:session_id",
            axum::routing::delete(qemu::stop_session),
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
    #[cfg(feature = "qemu")]
    crate::console::hxc::relay::apply(&saved).await;
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

    pub async fn relay_status() -> Json<hxc::relay::RelayStatus> {
        Json(hxc::relay::status(&hxc_settings::get()))
    }

    #[derive(serde::Serialize)]
    pub struct TicketResponse {
        /// ICE servers (the node's relay with this session's credentials)
        /// when WAN access is on; empty otherwise.
        ice_servers: Vec<hxc::relay::IceServer>,
        /// `relay` when clients must use the relay only, else `all`.
        ice_transport_policy: &'static str,
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
        uri: axum::http::Uri,
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
        // HTTP/1.1 sends Host; HTTP/2 (browsers over TLS) carries the
        // authority in the request URI instead.
        let host = headers
            .get(axum::http::header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_string)
            .or_else(|| uri.authority().map(|a| a.to_string()));
        let ips = hxc::candidate_ips(&settings, host.as_deref()).await;
        let relay_tag = if hxc::relay::running() {
            uuid::Uuid::new_v4().simple().to_string()
        } else {
            String::new()
        };
        let ice_servers = if relay_tag.is_empty() {
            Vec::new()
        } else {
            hxc::relay::ice_servers(host.as_deref(), &relay_tag)
        };
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
                    relay_tag,
                    expires_at: 0,
                },
                settings.ticket_lifetime_secs,
            )
            .await;
        let policy = if !ice_servers.is_empty() && hxc::relay::force_relay() {
            "relay"
        } else {
            "all"
        };
        Ok(Json(TicketResponse {
            ice_servers,
            ice_transport_policy: policy,
            ticket: t.ticket,
            expires_at: t.expires_at,
            profile: t.profile,
            input: t.allow_input,
            clipboard: t.allow_clipboard,
            microphone: t.allow_mic,
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
                relay_tag: (!t.relay_tag.is_empty()).then(|| t.relay_tag.clone()),
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
                                    "input={} clipboard={} relay_offered={} candidates={:?}",
                                    t.allow_input,
                                    t.allow_clipboard,
                                    !t.relay_tag.is_empty(),
                                    t.candidate_ips
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
                                        "reason={} path={} frames={} bytes={} keys={} pointer={} input_denied={}",
                                        s.end_reason.unwrap_or_default(),
                                        s.path.unwrap_or_else(|| "none".into()),
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
