//! Reading the live state and carrying a plan out. Everything goes through
//! the same managers and handlers the HTTP API uses, so apply has exactly
//! the validation and side effects of the individual calls.
use super::{plan, pool_type, to_disk, Action, Change, Live, Plan, Spec};
use crate::api::sdn_zones as sdn;
use crate::AppState;
use axum::extract::{Path, State};
use axum::Json;
use horcrux_common::{VmConfig, VmStatus};
use serde::Serialize;
use std::sync::Arc;

/// One apply at a time on a node.
static APPLY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub async fn gather(state: &Arc<AppState>) -> Live {
    Live {
        pools: state.storage_manager.list_pools().await,
        zones: sdn::list_zones(State(state.clone())).await.0,
        vnets: sdn::list_vnets(State(state.clone())).await.0,
        subnets: sdn::list_subnets(State(state.clone())).await.0,
        vms: state.vm_manager.list_vms().await,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeResult {
    #[serde(flatten)]
    pub change: Change,
    /// applied | failed | skipped
    pub status: &'static str,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ApplyReport {
    pub ok: bool,
    pub plan: Plan,
    pub results: Vec<ChangeResult>,
}

/// Plan against the live node, then apply. `dry_run` stops after the plan.
/// `Err` = another apply is running.
pub async fn apply(
    state: &Arc<AppState>,
    spec: &Spec,
    prune: bool,
    dry_run: bool,
) -> Result<ApplyReport, String> {
    let _guard = APPLY_LOCK
        .try_lock()
        .map_err(|_| "another apply is already running on this node".to_string())?;
    let live = gather(state).await;
    let p = plan(spec, &live, prune);
    let mut results = Vec::new();
    if dry_run {
        return Ok(ApplyReport {
            ok: p.errors.is_empty() && !p.changes.iter().any(|c| c.action == Action::Blocked),
            plan: p,
            results,
        });
    }
    if !p.errors.is_empty() {
        return Ok(ApplyReport {
            ok: false,
            plan: p,
            results,
        });
    }
    let mut ok = true;
    for c in &p.changes {
        let r = run_change(state, spec, &live, c).await;
        let (status, message) = match r {
            Ok(Some(m)) => ("applied", m),
            Ok(None) => ("applied", String::new()),
            Err(e) if c.action == Action::Blocked => ("skipped", e),
            Err(e) => ("failed", e),
        };
        ok &= status == "applied";
        results.push(ChangeResult {
            change: c.clone(),
            status,
            message,
        });
    }
    Ok(ApplyReport {
        ok,
        plan: p,
        results,
    })
}

async fn run_change(
    state: &Arc<AppState>,
    spec: &Spec,
    live: &Live,
    c: &Change,
) -> Result<Option<String>, String> {
    let e = |e: &dyn std::fmt::Debug| format!("{e:?}");
    let api = api_msg;
    match (c.kind.as_str(), c.action) {
        (_, Action::Blocked) => Err(c.details.join("; ")),

        ("storage_pool", Action::Create) => {
            let w = spec
                .storage_pools
                .iter()
                .flatten()
                .find(|p| p.name == c.name)
                .ok_or("not in spec")?;
            let pool = crate::storage::StoragePool {
                id: uuid::Uuid::new_v4().to_string(),
                name: w.name.clone(),
                storage_type: pool_type(&w.storage_type)?,
                path: w.path.clone(),
                available: 0,
                total: 0,
                enabled: w.enabled,
            };
            state
                .storage_manager
                .add_pool(pool)
                .await
                .map_err(|x| e(&x))?;
            Ok(None)
        }
        ("storage_pool", Action::Delete) => {
            let id = live
                .pools
                .iter()
                .find(|p| p.name == c.name)
                .map(|p| p.id.clone())
                .ok_or("pool is gone")?;
            state
                .storage_manager
                .remove_pool(&id)
                .await
                .map_err(|x| e(&x))?;
            Ok(None)
        }

        ("zone", Action::Create) | ("zone", Action::Update) => {
            let z = spec
                .networks
                .iter()
                .flat_map(|n| n.zones.iter().flatten())
                .find(|z| z.id == c.name)
                .ok_or("not in spec")?
                .clone();
            if c.action == Action::Create {
                sdn::create_zone(State(state.clone()), Json(z))
                    .await
                    .map(|_| ())
            } else {
                sdn::update_zone(State(state.clone()), Path(c.name.clone()), Json(z))
                    .await
                    .map(|_| ())
            }
            .map_err(api)?;
            Ok(None)
        }
        ("zone", Action::Delete) => {
            let _ = sdn::delete_zone(State(state.clone()), Path(c.name.clone()))
                .await
                .map_err(api)?;
            Ok(None)
        }
        ("vnet", Action::Create) => {
            let v = spec
                .networks
                .iter()
                .flat_map(|n| n.vnets.iter().flatten())
                .find(|v| v.id == c.name)
                .ok_or("not in spec")?
                .clone();
            let _ = sdn::create_vnet(State(state.clone()), Json(v))
                .await
                .map_err(api)?;
            Ok(None)
        }
        ("vnet", Action::Delete) => {
            let _ = sdn::delete_vnet(State(state.clone()), Path(c.name.clone()))
                .await
                .map_err(api)?;
            Ok(None)
        }
        ("subnet", Action::Create) => {
            let s = spec
                .networks
                .iter()
                .flat_map(|n| n.subnets.iter().flatten())
                .find(|s| s.id == c.name)
                .ok_or("not in spec")?
                .clone();
            let _ = sdn::create_subnet(State(state.clone()), Path(s.vnet.clone()), Json(s))
                .await
                .map_err(api)?;
            Ok(None)
        }
        ("subnet", Action::Delete) => {
            let _ = sdn::delete_subnet(State(state.clone()), Path(c.name.clone()))
                .await
                .map_err(api)?;
            Ok(None)
        }

        ("vm", Action::Create) => create_vm(state, spec, &c.name).await.map(Some),
        ("vm", Action::Start) => {
            let id = vm_id(state, spec, &c.name).await?;
            state.vm_manager.start_vm(&id).await.map_err(|x| e(&x))?;
            Ok(None)
        }
        ("vm", Action::Stop) => {
            let id = vm_id(state, spec, &c.name).await?;
            state.vm_manager.stop_vm(&id).await.map_err(|x| e(&x))?;
            Ok(None)
        }
        ("vm", Action::Delete) => {
            let id = live
                .vms
                .iter()
                .find(|v| v.name == c.name)
                .map(|v| v.id.clone())
                .ok_or("VM is gone")?;
            state.vm_manager.delete_vm(&id).await.map_err(|x| e(&x))?;
            if let Err(x) = state
                .firewall_manager
                .remove_guest_rules(crate::firewall::FirewallScope::Vm(id.clone()))
                .await
            {
                return Ok(Some(format!("deleted, but its firewall rules stayed: {x}")));
            }
            Ok(None)
        }
        (k, a) => Err(format!("no executor for {k} {a:?}")),
    }
}

/// The id of the VM called `name` (created earlier in this apply, or live).
async fn vm_id(state: &Arc<AppState>, spec: &Spec, name: &str) -> Result<String, String> {
    if let Some(v) = state
        .vm_manager
        .list_vms()
        .await
        .into_iter()
        .find(|v| v.name == name)
    {
        return Ok(v.id);
    }
    spec.vms
        .iter()
        .flatten()
        .find(|v| v.name == name)
        .map(|v| v.vm_id())
        .ok_or_else(|| format!("VM {name} not found"))
}

async fn create_vm(state: &Arc<AppState>, spec: &Spec, name: &str) -> Result<String, String> {
    let w = spec
        .vms
        .iter()
        .flatten()
        .find(|v| v.name == name)
        .ok_or("not in spec")?;
    let pools = state.storage_manager.list_pools().await;
    let mut disks = Vec::new();
    for d in &w.disks {
        let id = match d.storage.as_deref() {
            None => None,
            Some(n) => Some(
                pools
                    .iter()
                    .find(|p| p.name == n)
                    .map(|p| p.id.clone())
                    .ok_or_else(|| format!("storage pool {n} does not exist"))?,
            ),
        };
        disks.push(to_disk(d, id));
    }
    let mut cfg = VmConfig {
        id: w.vm_id(),
        name: w.name.clone(),
        hypervisor: w.hypervisor.clone(),
        memory: w.memory,
        cpus: w.cpus,
        disk_size: w.disks.first().map(|d| d.size_gb).unwrap_or(10),
        status: VmStatus::Stopped,
        architecture: w.architecture.clone().unwrap_or_default(),
        disks,
        display: w.display.unwrap_or_default(),
        hardware: w.hardware.clone().unwrap_or_default(),
    };
    let api = api_msg;
    crate::api::storage_volumes::place_vm_disks(state, &mut cfg)
        .await
        .map_err(api)?;
    crate::api::vm_hw::check_host_hardware(state, &cfg.hardware)
        .await
        .map_err(api)?;
    let vm = state
        .vm_manager
        .create_vm_from_image(cfg, w.image.clone())
        .await
        .map_err(|e| format!("{e:?}"))?;
    Ok(format!("id {}", vm.id))
}

/// The message inside an `ApiError`.
fn api_msg(e: crate::error::ApiError) -> String {
    use crate::error::ApiError as E;
    match e {
        E::Internal(m)
        | E::NotFound(m)
        | E::Forbidden(m)
        | E::BadRequest(m)
        | E::Conflict(m)
        | E::ValidationError(m)
        | E::ServiceUnavailable(m)
        | E::RateLimited(m) => m,
        other => format!("{other:?}"),
    }
}
