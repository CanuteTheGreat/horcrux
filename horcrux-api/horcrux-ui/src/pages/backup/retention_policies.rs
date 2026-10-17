//! /backup/retention: preview and apply a keep-* retention policy to one
//! guest's backups (Proxmox "prune" semantics; protected backups are kept).
use super::common::{confirm, load_guests, short, Guest};
use crate::api::backup as bk;
use leptos::*;

#[component]
pub fn RetentionPoliciesPage() -> impl IntoView {
    let (guests, set_guests) = create_signal(Vec::<Guest>::new());
    let (backups, set_backups) = create_signal(Vec::<bk::Backup>::new());
    let (target, set_target) = create_signal(String::new());
    let (storage, set_storage) = create_signal(String::new());
    let (decisions, set_decisions) = create_signal(None::<Vec<bk::PruneDecision>>);
    let (applied, set_applied) = create_signal(false);
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let keep: [(ReadSignal<String>, WriteSignal<String>); 6] =
        std::array::from_fn(|_| create_signal(String::new()));
    let labels = [
        "Keep last",
        "Keep hourly",
        "Keep daily",
        "Keep weekly",
        "Keep monthly",
        "Keep yearly",
    ];

    let reload = move || {
        spawn_local(async move {
            if let Ok(b) = bk::list_backups().await {
                set_backups.set(b);
            }
        });
    };
    create_effect(move |_| {
        reload();
        spawn_local(async move {
            let g = load_guests().await;
            if let Some(first) = g.first() {
                set_target.set(first.id.clone());
            }
            set_guests.set(g);
        });
    });

    let run = move |dry_run: bool| {
        let k: Vec<String> = keep.iter().map(|(r, _)| r.get()).collect();
        let policy = match bk::Retention::from_fields([&k[0], &k[1], &k[2], &k[3], &k[4], &k[5]]) {
            Ok(p) => p,
            Err(e) => {
                set_error.set(Some(e));
                return;
            }
        };
        let t = target.get();
        if t.is_empty() {
            set_error.set(Some("Choose a guest".into()));
            return;
        }
        if !dry_run {
            if policy == bk::Retention::default() {
                set_error.set(Some("Set at least one keep-* option".into()));
                return;
            }
            if !confirm(&format!(
                "Delete every backup of {t} the policy ({}) does not keep?",
                policy.summary()
            )) {
                return;
            }
        }
        let req = bk::PruneRequest {
            target_id: t.clone(),
            storage: Some(storage.get()).filter(|s| !s.is_empty()),
            policy,
            dry_run,
        };
        spawn_local(async move {
            match bk::prune_backups(&req).await {
                Ok(d) => {
                    let removed = d.iter().filter(|x| x.mark == "remove").count();
                    set_notice.set(Some(if dry_run {
                        format!(
                            "Preview: {removed} of {} backup(s) of {t} would be removed",
                            d.len()
                        )
                    } else {
                        format!("Pruned {removed} backup(s) of {t}")
                    }));
                    set_error.set(None);
                    set_applied.set(!dry_run);
                    set_decisions.set(Some(d));
                }
                Err(e) => set_error.set(Some(format!("Prune: {}", e.message))),
            }
            reload();
        });
    };

    let storages_of_target = move || {
        let t = target.get();
        let mut s: Vec<String> = backups
            .get()
            .into_iter()
            .filter(|b| b.target_id == t)
            .map(|b| b.storage)
            .collect();
        s.sort();
        s.dedup();
        s
    };

    view! {
        <div class="backup-retention">
            <div class="page-header">
                <h1>"Retention / Prune"</h1>
                <div class="header-actions">
                    <a href="/backup" class="btn btn-secondary">"Backups"</a>
                    " "
                    <a href="/backup/jobs" class="btn btn-secondary">"Backup Jobs"</a>
                </div>
            </div>
            <p>"Options apply in order (last, hourly, daily, weekly, monthly, yearly); each keeps the newest backup of N periods no earlier option covered. Protected backups are always kept. Backup jobs apply their own retention after every run."</p>

            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="prune-notice">{n}</div> })}
            {move || error.get().map(|e| view! { <div class="error" id="prune-error">{e}</div> })}

            <div class="dashboard-section">
                <div class="form-group">
                    <label for="prune-target">"Guest"</label>
                    <select id="prune-target" on:change=move |ev| { set_target.set(event_target_value(&ev)); set_decisions.set(None); }>
                        {move || guests.get().into_iter().map(|g| {
                            let sel = g.id == target.get();
                            view! { <option value=g.id.clone() selected=sel>{g.label}</option> }
                        }).collect_view()}
                    </select>
                </div>
                <div class="form-group">
                    <label for="prune-storage">"Storage"</label>
                    <select id="prune-storage" on:change=move |ev| set_storage.set(event_target_value(&ev))>
                        <option value="">"all (each pruned separately)"</option>
                        {move || storages_of_target().into_iter().map(|s| view! { <option value=s.clone()>{s.clone()}</option> }).collect_view()}
                    </select>
                </div>
                <div class="form-group">
                    {keep.iter().enumerate().map(|(i, (r, w))| {
                        let (r, w) = (*r, *w);
                        view! {
                            <label>{labels[i]}
                                <input id=format!("prune-keep-{i}") type="number" min="0" prop:value=move || r.get()
                                    on:input=move |ev| w.set(event_target_value(&ev))/>
                            </label>
                        }
                    }).collect_view()}
                </div>
                <div class="form-actions">
                    <button id="prune-preview" class="btn btn-secondary" on:click=move |_| run(true)>"Preview"</button>
                    " "
                    <button id="prune-apply" class="btn btn-danger" on:click=move |_| run(false)>"Prune"</button>
                </div>
            </div>

            {move || decisions.get().map(|d| view! {
                <table class="vm-table" id="prune-table">
                    <thead><tr><th>"Backup"</th><th>"Time"</th><th>{if applied.get() { "Result" } else { "Would" }}</th></tr></thead>
                    <tbody>
                        {d.into_iter().map(|x| {
                            let class = match x.mark.as_str() { "remove" => "status-stopped", "protected" => "status-paused", _ => "status-running" };
                            view! {
                                <tr class="prune-row" data-backup-id=x.backup_id.clone()>
                                    <td>{short(&x.backup_id)}</td>
                                    <td>{bk::format_time(x.timestamp)}</td>
                                    <td><span class=class>{x.mark.clone()}</span></td>
                                </tr>
                            }
                        }).collect_view()}
                    </tbody>
                </table>
            })}
        </div>
    }
}
