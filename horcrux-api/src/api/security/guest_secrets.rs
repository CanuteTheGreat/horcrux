//! Guest provisioning secrets: the encrypted store, the provisioning
//! records (references only) and the seed-disc lifecycle.
//!
//! * `GET|POST /api/security/guest-secrets`            list (no values) / create
//! * `GET|PUT|DELETE /api/security/guest-secrets/:name` metadata / rotate or edit / delete
//! * `GET /api/security/guest-secrets-audit?name=&limit=` audit trail
//! * `GET|PUT|DELETE /api/security/provisioning/:vm`   what the VM is provisioned with
//! * `POST /api/security/provisioning/:vm/seed`        inject now (builds the seed disc)
//!
//! Values are AES-256-GCM encrypted (horcrux-core `EncryptionManager`) with a
//! key kept in its own 0600 file, so a copy of the database, an export or a
//! backup never holds a value. The API never returns one. A value leaves the
//! store only in `resolve`, while a seed disc is built; every such use is in
//! the audit trail. The seed disc is built in a private 0700 directory,
//! stored 0600 next to the VM's disk, and removed after the first boot.
use crate::api::system_admin::data_dir;
use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Extension, Json, Router,
};
use horcrux_common::cloudinit::CloudInitConfig;
use horcrux_common::guest_secrets::{
    valid_name, GuestProvisioning, GuestSecretAudit, GuestSecretMeta, GuestSecretPut, SecretKind,
};
use serde::{Deserialize, Serialize};
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

const SECRET_KIND: &str = "guest_secret";
const AUDIT_KIND: &str = "guest_secret_audit";
const PROVISIONING_KIND: &str = "vm_provisioning";
const PROVISIONED_KIND: &str = "vm_provisioned";

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/security/guest-secrets",
            get(list_secrets).post(create_secret),
        )
        .route(
            "/api/security/guest-secrets/:name",
            get(get_secret).put(put_secret).delete(delete_secret),
        )
        .route("/api/security/guest-secrets-audit", get(list_audit))
        .route(
            "/api/security/provisioning/:vm",
            get(get_provisioning)
                .put(put_provisioning)
                .delete(delete_provisioning),
        )
        .route("/api/security/provisioning/:vm/seed", post(inject_now))
}

// ------------------------------------------------------------ the vault

/// The stored form: metadata plus the encrypted value.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    meta: GuestSecretMeta,
    /// base64(nonce + AES-256-GCM ciphertext)
    ct: String,
}

/// The encrypted store over a `ConfigStore`.
pub struct Vault {
    store: crate::db::config_store::ConfigStore,
    enc: Arc<crate::encryption::EncryptionManager>,
    settings: horcrux_common::guest_secrets::GuestSecretsSettings,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Read or create the key file (hex, 0600).
async fn init_encryption(
    path: &FsPath,
) -> Result<Arc<crate::encryption::EncryptionManager>, ApiError> {
    use crate::encryption::{EncryptionConfig, EncryptionManager};
    if !path.exists() {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let key = EncryptionManager::generate_key_hex();
        let tmp = path.with_extension("tmp");
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(|e| ApiError::Internal(format!("guest secrets key file: {e}")))?;
            f.write_all(key.as_bytes())
                .map_err(|e| ApiError::Internal(format!("guest secrets key file: {e}")))?;
        }
        tokio::fs::rename(&tmp, path).await?;
    }
    let enc = Arc::new(EncryptionManager::new());
    enc.initialize(EncryptionConfig {
        master_key: None,
        key_file: Some(path.to_string_lossy().to_string()),
        auto_generate: false,
    })
    .await?;
    Ok(enc)
}

impl Vault {
    pub async fn open(state: &AppState) -> Result<Vault, ApiError> {
        let settings = super::load_settings(state).await.guest_secrets;
        let path = if settings.key_file.is_empty() {
            data_dir().join("guest-secrets.key")
        } else {
            PathBuf::from(&settings.key_file)
        };
        Ok(Vault {
            store: crate::db::config_store::ConfigStore::new(&state.database),
            enc: init_encryption(&path).await?,
            settings,
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        store: crate::db::config_store::ConfigStore,
        enc: Arc<crate::encryption::EncryptionManager>,
    ) -> Vault {
        Vault {
            store,
            enc,
            settings: Default::default(),
        }
    }

    async fn load(&self, name: &str) -> Result<Option<Stored>, ApiError> {
        Ok(self
            .store
            .load::<Stored>(SECRET_KIND)
            .await?
            .into_iter()
            .find(|(id, _)| id == name)
            .map(|(_, s)| s))
    }

    fn flag_due(&self, mut m: GuestSecretMeta) -> GuestSecretMeta {
        let days = self.settings.rotation_warn_days as i64;
        m.rotation_due = days > 0 && now() - m.rotated > days * 86_400;
        m
    }

    pub async fn list(&self) -> Result<Vec<GuestSecretMeta>, ApiError> {
        let mut v: Vec<GuestSecretMeta> = self
            .store
            .load_values::<Stored>(SECRET_KIND)
            .await?
            .into_iter()
            .map(|s| self.flag_due(s.meta))
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(v)
    }

    pub async fn meta(&self, name: &str) -> Result<GuestSecretMeta, ApiError> {
        self.load(name)
            .await?
            .map(|s| self.flag_due(s.meta))
            .ok_or_else(|| ApiError::NotFound(format!("secret {name:?} does not exist")))
    }

    fn check_value(&self, v: &str) -> Result<(), ApiError> {
        if v.is_empty() {
            return Err(ApiError::BadRequest("value must not be empty".into()));
        }
        if v.len() > self.settings.max_value_bytes as usize {
            return Err(ApiError::BadRequest(format!(
                "value larger than guest_secrets.max_value_bytes ({})",
                self.settings.max_value_bytes
            )));
        }
        Ok(())
    }

    pub async fn create(
        &self,
        actor: &str,
        name: &str,
        kind: SecretKind,
        description: &str,
        value: &str,
    ) -> Result<GuestSecretMeta, ApiError> {
        valid_name(name).map_err(ApiError::BadRequest)?;
        self.check_value(value)?;
        if self.load(name).await?.is_some() {
            return Err(ApiError::Conflict(format!(
                "secret {name:?} exists (PUT with a value rotates it)"
            )));
        }
        let t = now();
        let meta = GuestSecretMeta {
            name: name.into(),
            kind,
            description: description.chars().take(200).collect(),
            created: t,
            rotated: t,
            version: 1,
            used_count: 0,
            last_used: None,
            rotation_due: false,
        };
        let ct = self.enc.encrypt_string(value).await?;
        self.store
            .put(
                SECRET_KIND,
                name,
                &Stored {
                    meta: meta.clone(),
                    ct,
                },
            )
            .await?;
        self.audit(actor, "created", name, "").await;
        Ok(meta)
    }

    /// Edit metadata and/or rotate (a present `value`).
    pub async fn update(
        &self,
        actor: &str,
        name: &str,
        req: &GuestSecretPut,
    ) -> Result<GuestSecretMeta, ApiError> {
        let mut s = self
            .load(name)
            .await?
            .ok_or_else(|| ApiError::NotFound(format!("secret {name:?} does not exist")))?;
        if let Some(k) = req.kind {
            s.meta.kind = k;
        }
        if let Some(d) = &req.description {
            s.meta.description = d.chars().take(200).collect();
        }
        let action = if let Some(v) = &req.value {
            self.check_value(v)?;
            s.ct = self.enc.encrypt_string(v).await?;
            s.meta.rotated = now();
            s.meta.version += 1;
            "rotated"
        } else {
            "updated"
        };
        self.store.put(SECRET_KIND, name, &s).await?;
        self.audit(actor, action, name, &format!("version {}", s.meta.version))
            .await;
        Ok(self.flag_due(s.meta))
    }

    pub async fn delete(&self, actor: &str, name: &str) -> Result<(), ApiError> {
        if !self.store.delete(SECRET_KIND, name).await? {
            return Err(ApiError::NotFound(format!(
                "secret {name:?} does not exist"
            )));
        }
        self.audit(actor, "deleted", name, "").await;
        Ok(())
    }

    /// The value, for injection only. Counts and audits the use.
    pub async fn reveal(&self, actor: &str, name: &str, why: &str) -> Result<String, ApiError> {
        let mut s = self.load(name).await?.ok_or_else(|| {
            ApiError::BadRequest(format!("secret {name:?} is referenced but does not exist"))
        })?;
        let v = self.enc.decrypt_string(&s.ct).await?;
        s.meta.used_count += 1;
        s.meta.last_used = Some(now());
        self.store.put(SECRET_KIND, name, &s).await?;
        self.audit(actor, "injected", name, why).await;
        Ok(v)
    }

    /// Fail unless every secret a provisioning record names exists.
    pub async fn require(&self, p: &GuestProvisioning) -> Result<(), ApiError> {
        for n in p.secret_refs() {
            if self.load(&n).await?.is_none() {
                return Err(ApiError::BadRequest(format!(
                    "secret {n:?} does not exist (create it under Security > Secrets)"
                )));
            }
        }
        Ok(())
    }

    /// Resolve every reference in a cloud-init config (values only live in
    /// the returned copy).
    pub async fn resolve(
        &self,
        actor: &str,
        cfg: &CloudInitConfig,
        why: &str,
    ) -> Result<CloudInitConfig, ApiError> {
        let mut values = std::collections::BTreeMap::new();
        for n in cfg.secret_refs() {
            let v = self.reveal(actor, &n, why).await?;
            values.insert(n, v);
        }
        cfg.resolved(&|n| {
            values
                .get(n)
                .cloned()
                .ok_or_else(|| format!("secret {n:?} was not loaded"))
        })
        .map_err(ApiError::BadRequest)
    }

    pub async fn audit(&self, actor: &str, action: &str, name: &str, detail: &str) {
        let ts = now();
        // ordered, unique id: seconds + a counter from the table size is racy,
        // so use nanoseconds
        let id = format!(
            "{:020}",
            chrono::Utc::now()
                .timestamp_nanos_opt()
                .unwrap_or(ts * 1_000_000_000)
        );
        let e = GuestSecretAudit {
            ts,
            actor: actor.into(),
            action: action.into(),
            name: name.into(),
            detail: detail.chars().take(200).collect(),
        };
        if let Err(err) = self.store.put(AUDIT_KIND, &id, &e).await {
            tracing::warn!("guest secret audit: {err}");
            return;
        }
        // keep the newest `audit_keep`
        if let Ok(all) = self.store.load::<GuestSecretAudit>(AUDIT_KIND).await {
            let keep = self.settings.audit_keep as usize;
            if all.len() > keep {
                for (old, _) in all.iter().take(all.len() - keep) {
                    let _ = self.store.delete(AUDIT_KIND, old).await;
                }
            }
        }
    }

    pub async fn audit_list(
        &self,
        name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<GuestSecretAudit>, ApiError> {
        let mut v: Vec<GuestSecretAudit> = self
            .store
            .load_values::<GuestSecretAudit>(AUDIT_KIND)
            .await?
            .into_iter()
            .filter(|e| name.is_none_or(|n| e.name == n))
            .collect();
        v.reverse();
        v.truncate(limit);
        Ok(v)
    }

    pub async fn provisioning(&self, vm: &str) -> Result<Option<GuestProvisioning>, ApiError> {
        Ok(self
            .store
            .load::<GuestProvisioning>(PROVISIONING_KIND)
            .await?
            .into_iter()
            .find(|(id, _)| id == vm)
            .map(|(_, p)| p))
    }

    pub async fn all_provisioning(&self) -> Result<Vec<(String, GuestProvisioning)>, ApiError> {
        Ok(self.store.load(PROVISIONING_KIND).await?)
    }

    pub async fn set_provisioning(&self, vm: &str, p: &GuestProvisioning) -> Result<(), ApiError> {
        p.validate().map_err(ApiError::BadRequest)?;
        self.require(p).await?;
        self.store.put(PROVISIONING_KIND, vm, p).await?;
        // a changed record is injected again at the next start
        self.store.delete(PROVISIONED_KIND, vm).await?;
        Ok(())
    }

    pub async fn clear_provisioning(&self, vm: &str) -> Result<bool, ApiError> {
        self.store.delete(PROVISIONED_KIND, vm).await?;
        Ok(self.store.delete(PROVISIONING_KIND, vm).await?)
    }

    async fn is_provisioned(&self, vm: &str) -> bool {
        self.store
            .load::<i64>(PROVISIONED_KIND)
            .await
            .map(|v| v.iter().any(|(id, _)| id == vm))
            .unwrap_or(false)
    }

    async fn mark_provisioned(&self, vm: &str) {
        let _ = self.store.put(PROVISIONED_KIND, vm, &now()).await;
    }
}

fn actor(u: &Option<Extension<AuthUser>>) -> String {
    u.as_ref()
        .map(|Extension(u)| u.username.clone())
        .unwrap_or_else(|| "system".into())
}

// ------------------------------------------------------------ handlers

async fn list_secrets(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<GuestSecretMeta>>, ApiError> {
    Ok(Json(Vault::open(&state).await?.list().await?))
}

async fn get_secret(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<GuestSecretMeta>, ApiError> {
    Ok(Json(Vault::open(&state).await?.meta(&name).await?))
}

async fn create_secret(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<GuestSecretPut>,
) -> Result<(StatusCode, Json<GuestSecretMeta>), ApiError> {
    let name = req
        .name
        .clone()
        .ok_or_else(|| ApiError::BadRequest("name is required".into()))?;
    let value = req
        .value
        .clone()
        .ok_or_else(|| ApiError::BadRequest("value is required".into()))?;
    let m = Vault::open(&state)
        .await?
        .create(
            &actor(&user),
            &name,
            req.kind.unwrap_or_default(),
            req.description.as_deref().unwrap_or(""),
            &value,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(m)))
}

async fn put_secret(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(name): Path<String>,
    Json(req): Json<GuestSecretPut>,
) -> Result<Json<GuestSecretMeta>, ApiError> {
    Ok(Json(
        Vault::open(&state)
            .await?
            .update(&actor(&user), &name, &req)
            .await?,
    ))
}

async fn delete_secret(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(name): Path<String>,
) -> Result<StatusCode, ApiError> {
    let v = Vault::open(&state).await?;
    // refuse while a provisioning record still names it
    for (vm, p) in v.all_provisioning().await? {
        if p.secret_refs().contains(&name) {
            return Err(ApiError::Conflict(format!(
                "secret {name:?} is used by the provisioning of VM {vm}"
            )));
        }
    }
    v.delete(&actor(&user), &name).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AuditQuery {
    name: Option<String>,
    limit: Option<usize>,
}

async fn list_audit(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<GuestSecretAudit>>, ApiError> {
    Ok(Json(
        Vault::open(&state)
            .await?
            .audit_list(q.name.as_deref(), q.limit.unwrap_or(200).min(5000))
            .await?,
    ))
}

async fn get_provisioning(
    State(state): State<Arc<AppState>>,
    Path(vm): Path<String>,
) -> Result<Json<GuestProvisioning>, ApiError> {
    Ok(Json(
        Vault::open(&state)
            .await?
            .provisioning(&vm)
            .await?
            .unwrap_or_default(),
    ))
}

async fn put_provisioning(
    State(state): State<Arc<AppState>>,
    Path(vm): Path<String>,
    Json(p): Json<GuestProvisioning>,
) -> Result<Json<GuestProvisioning>, ApiError> {
    state.vm_manager.get_vm(&vm).await?;
    Vault::open(&state).await?.set_provisioning(&vm, &p).await?;
    Ok(Json(p))
}

async fn delete_provisioning(
    State(state): State<Arc<AppState>>,
    Path(vm): Path<String>,
) -> Result<StatusCode, ApiError> {
    Vault::open(&state).await?.clear_provisioning(&vm).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct SeedResponse {
    vm_id: String,
    attached_as: Option<String>,
}

async fn inject_now(
    State(state): State<Arc<AppState>>,
    user: Option<Extension<AuthUser>>,
    Path(vm): Path<String>,
) -> Result<Json<SeedResponse>, ApiError> {
    let v = Vault::open(&state).await?;
    let p = v
        .provisioning(&vm)
        .await?
        .and_then(|p| p.cloud_init)
        .ok_or_else(|| ApiError::NotFound(format!("VM {vm} has no cloud-init provisioning")))?;
    let seed = build_seed(&state, &v, &actor(&user), &vm, &p, "inject now").await?;
    Ok(Json(SeedResponse {
        vm_id: vm,
        attached_as: seed.map(|s| s.to_string_lossy().to_string()),
    }))
}

// ------------------------------------------------------- seed lifecycle

/// Resolve `cfg`, build the seed disc and store it next to the VM's disk
/// (0600; the manager's own copy is removed at once). Returns the seed path
/// (None for a VM without a disk).
pub async fn build_seed(
    state: &Arc<AppState>,
    v: &Vault,
    who: &str,
    vm_id: &str,
    cfg: &CloudInitConfig,
    why: &str,
) -> Result<Option<PathBuf>, ApiError> {
    let vm = state.vm_manager.get_vm(vm_id).await?;
    let resolved = v.resolve(who, cfg, &format!("VM {vm_id}: {why}")).await?;
    let iso = state
        .cloudinit_manager
        .generate_iso(vm_id, &resolved)
        .await?;
    drop(resolved);
    let seed = match vm.disks.first() {
        Some(d) => {
            let seed = crate::vm::qemu::cloudinit_seed(FsPath::new(&d.path));
            tokio::fs::copy(&iso, &seed).await?;
            set_private(&seed).await;
            Some(seed)
        }
        None => None,
    };
    // the manager's copy served only to build this one
    let _ = tokio::fs::remove_file(&iso).await;
    Ok(seed)
}

async fn set_private(p: &FsPath) {
    use std::os::unix::fs::PermissionsExt;
    let _ = tokio::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).await;
}

/// Before a VM starts: inject its stored provisioning once.
pub async fn provision_on_start(state: &Arc<AppState>, vm_id: &str) -> Result<(), ApiError> {
    let v = Vault::open(state).await?;
    let Some(p) = v.provisioning(vm_id).await?.and_then(|p| p.cloud_init) else {
        return Ok(());
    };
    if v.is_provisioned(vm_id).await {
        return Ok(());
    }
    build_seed(state, &v, "system", vm_id, &p, "first start").await?;
    v.mark_provisioned(vm_id).await;
    Ok(())
}

/// After a VM started: remove its seed disc once it has booted.
pub async fn watch_seed(state: &Arc<AppState>, vm_id: &str) {
    let settings = super::load_settings(state).await.guest_secrets;
    if !settings.remove_seed_after_first_boot {
        return;
    }
    let Ok(vm) = state.vm_manager.get_vm(vm_id).await else {
        return;
    };
    let Some(disk) = vm.disks.first().map(|d| PathBuf::from(&d.path)) else {
        return;
    };
    if !crate::vm::qemu::cloudinit_seed(&disk).exists() {
        return;
    }
    spawn_seed_cleanup(state.clone(), vm_id.to_string(), disk, settings);
}

fn spawn_seed_cleanup(
    state: Arc<AppState>,
    vm_id: String,
    disk: PathBuf,
    s: horcrux_common::guest_secrets::GuestSecretsSettings,
) {
    tokio::spawn(async move {
        let t0 = std::time::Instant::now();
        let poll = std::time::Duration::from_secs(s.seed_poll_secs.max(5));
        loop {
            tokio::time::sleep(poll).await;
            let seed = crate::vm::qemu::cloudinit_seed(&disk);
            if !seed.exists() {
                return;
            }
            let running = matches!(
                state.vm_manager.get_vm(&vm_id).await,
                Ok(vm) if vm.status == horcrux_common::VmStatus::Running
            );
            let up = t0.elapsed().as_secs();
            let booted = running
                && (up >= s.seed_grace_secs || crate::vm::gaming::guest_agent_answers(&disk).await);
            // a VM that stopped after its grace time has booted at least once
            let stopped_after = !running && up >= s.seed_grace_secs;
            if booted || stopped_after {
                if tokio::fs::remove_file(&seed).await.is_ok() {
                    tracing::info!("VM {vm_id}: cloud-init seed removed after first boot");
                    if let Ok(v) = Vault::open(&state).await {
                        v.audit("system", "seed-removed", "", &format!("VM {vm_id}"))
                            .await;
                    }
                }
                return;
            }
            if !running {
                // stopped early: picked up again at the next start
                return;
            }
        }
    });
}

/// Upgrade step, run at startup: the manager used to keep a copy of every
/// seed (`cloudinit-<vm>.iso`) forever, next to the one attached to the VM.
/// Remove those copies, and watch the seeds of running VMs so they go too.
pub async fn migrate_existing(state: &Arc<AppState>) {
    let dir = state.config.paths.cloudinit.clone();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("cloudinit-")
                && n.ends_with(".iso")
                && tokio::fs::remove_file(e.path()).await.is_ok()
            {
                tracing::info!("guest secrets: removed leftover seed copy {n}");
            }
        }
    }
    for vm in state.vm_manager.list_vms().await {
        if vm.status == horcrux_common::VmStatus::Running {
            watch_seed(state, &vm.id).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::config_store::ConfigStore;
    use crate::encryption::{EncryptionConfig, EncryptionManager};
    use horcrux_common::cloudinit::UserConfig;

    async fn vault() -> (Vault, tempfile::TempDir, ConfigStore) {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite://{}/t.db", dir.path().display());
        let db = crate::db::Database::new(&url).await.unwrap();
        db.migrate().await.unwrap();
        let store = ConfigStore::new(&db);
        let enc = Arc::new(EncryptionManager::new());
        enc.initialize(EncryptionConfig {
            master_key: Some(EncryptionManager::generate_key_hex()),
            key_file: None,
            auto_generate: false,
        })
        .await
        .unwrap();
        (Vault::for_test(store.clone(), enc), dir, store)
    }

    fn cfg() -> CloudInitConfig {
        CloudInitConfig {
            hostname: "web1".into(),
            user: Some(UserConfig {
                name: "ops".into(),
                password: None,
                plain_password: Some("secret:ops-pw".into()),
                sudo: true,
                shell: None,
            }),
            ssh_keys: vec![],
            network: None,
            packages: vec![],
            runcmd: vec!["curl -H 'X-Token: {{secret:api-tok}}' https://x/".into()],
        }
    }

    #[tokio::test]
    async fn value_is_encrypted_at_rest_and_never_listed() {
        let (v, _d, store) = vault().await;
        v.create(
            "ron",
            "ops-pw",
            SecretKind::Password,
            "ops login",
            "Tr0ub4dor&3",
        )
        .await
        .unwrap();
        // the raw database row holds no value
        let raw = store.load::<serde_json::Value>(SECRET_KIND).await.unwrap();
        let text = serde_json::to_string(&raw).unwrap();
        assert!(!text.contains("Tr0ub4dor"), "{text}");
        // listing shows metadata only
        let l = serde_json::to_string(&v.list().await.unwrap()).unwrap();
        assert!(!l.contains("Tr0ub4dor") && l.contains("ops-pw"));
        assert!(v
            .create("ron", "ops-pw", SecretKind::Password, "", "x")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn rotation_audit_and_resolution() {
        let (v, _d, _s) = vault().await;
        v.create("ron", "ops-pw", SecretKind::Password, "", "one")
            .await
            .unwrap();
        v.create("ron", "api-tok", SecretKind::Token, "", "tok-1")
            .await
            .unwrap();
        let m = v
            .update(
                "ron",
                "ops-pw",
                &GuestSecretPut {
                    value: Some("two".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(m.version, 2);
        let r = v.resolve("system", &cfg(), "test").await.unwrap();
        assert_eq!(r.user.unwrap().plain_password.as_deref(), Some("two"));
        assert_eq!(r.runcmd[0], "curl -H 'X-Token: tok-1' https://x/");
        assert_eq!(v.meta("ops-pw").await.unwrap().used_count, 1);
        let a = v.audit_list(Some("ops-pw"), 50).await.unwrap();
        let acts: Vec<&str> = a.iter().map(|e| e.action.as_str()).collect();
        assert_eq!(acts, vec!["injected", "rotated", "created"]);
        // the audit trail holds no value
        assert!(!serde_json::to_string(&a).unwrap().contains("two"));
        // a missing secret refuses the injection
        v.delete("ron", "api-tok").await.unwrap();
        assert!(v.resolve("system", &cfg(), "test").await.is_err());
    }

    #[tokio::test]
    async fn provisioning_stores_references_only() {
        let (v, _d, store) = vault().await;
        v.create("ron", "ops-pw", SecretKind::Password, "", "one")
            .await
            .unwrap();
        v.create("ron", "api-tok", SecretKind::Token, "", "tok")
            .await
            .unwrap();
        let p = GuestProvisioning {
            cloud_init: Some(cfg()),
        };
        v.set_provisioning("vm1", &p).await.unwrap();
        let raw = serde_json::to_string(
            &store
                .load::<serde_json::Value>(PROVISIONING_KIND)
                .await
                .unwrap(),
        )
        .unwrap();
        assert!(raw.contains("secret:ops-pw") && !raw.contains("one"));
        // a literal password and an unknown reference are refused
        let mut bad = cfg();
        bad.user.as_mut().unwrap().plain_password = Some("hunter2".into());
        assert!(v
            .set_provisioning(
                "vm1",
                &GuestProvisioning {
                    cloud_init: Some(bad)
                }
            )
            .await
            .is_err());
        let mut missing = cfg();
        missing.runcmd = vec!["echo {{secret:nope}}".into()];
        assert!(v
            .set_provisioning(
                "vm1",
                &GuestProvisioning {
                    cloud_init: Some(missing)
                }
            )
            .await
            .is_err());
    }

    #[tokio::test]
    async fn audit_is_trimmed() {
        let (mut v, _d, _s) = vault().await;
        v.settings.audit_keep = 100;
        for i in 0..130 {
            v.audit("a", "x", &format!("n{i}"), "").await;
        }
        assert_eq!(v.audit_list(None, 1000).await.unwrap().len(), 100);
    }
}
