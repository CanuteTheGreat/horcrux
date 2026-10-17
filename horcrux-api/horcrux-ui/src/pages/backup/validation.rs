//! /backup/verify: re-check archives (checksum, unpack, per-file checksums,
//! `qemu-img check` of every disk) one by one or all at once.
use super::common::short;
use crate::api::backup as bk;
use leptos::*;

#[component]
pub fn BackupVerifyPage() -> impl IntoView {
    let (backups, set_backups) = create_signal(Vec::<bk::Backup>::new());
    let (running, set_running) = create_signal(None::<String>);
    let (error, set_error) = create_signal(None::<String>);
    let (summary, set_summary) = create_signal(None::<String>);

    let reload = move || {
        spawn_local(async move {
            match bk::list_backups().await {
                Ok(b) => set_backups.set(b),
                Err(e) => set_error.set(Some(format!("Loading backups: {}", e.message))),
            }
        });
    };
    create_effect(move |_| reload());

    let verify_one = move |id: String| {
        set_running.set(Some(id.clone()));
        spawn_local(async move {
            if let Err(e) = bk::verify_backup(&id).await {
                set_error.set(Some(format!("Verify {}: {}", short(&id), e.message)));
            }
            set_running.set(None);
            reload();
        });
    };

    let verify_all = move |_| {
        let ids: Vec<String> = backups.get().into_iter().map(|b| b.id).collect();
        spawn_local(async move {
            let (mut ok, mut bad) = (0, 0);
            for id in ids {
                set_running.set(Some(id.clone()));
                match bk::verify_backup(&id).await {
                    Ok(v) if v.ok => ok += 1,
                    _ => bad += 1,
                }
            }
            set_running.set(None);
            set_summary.set(Some(format!("Verified all backups: {ok} OK, {bad} failed")));
            reload();
        });
    };

    view! {
        <div class="backup-verify">
            <div class="page-header">
                <h1>"Verify Backups"</h1>
                <div class="header-actions">
                    <a href="/backup" class="btn btn-secondary">"Backups"</a>
                    " "
                    <button id="verify-all-btn" class="btn btn-primary" disabled=move || running.get().is_some()
                        on:click=verify_all>"Verify all"</button>
                </div>
            </div>
            <p>"Verify recomputes the archive SHA-256, unpacks it, checks every file against the checksums recorded at backup time and runs qemu-img check on every VM disk - the same checks a restore does before touching a guest."</p>
            {move || summary.get().map(|s| view! { <div class="alert alert-success" id="verify-summary">{s}</div> })}
            {move || error.get().map(|e| view! { <div class="error">{e}</div> })}
            <table class="vm-table" id="verify-table">
                <thead>
                    <tr><th>"Backup"</th><th>"Guest"</th><th>"Time"</th><th>"State"</th><th>"Checked"</th><th>"Actions"</th></tr>
                </thead>
                <tbody>
                    {move || backups.get().into_iter().map(|b| {
                        let id = b.id.clone();
                        let id2 = b.id.clone();
                        let state = match b.verify_state.as_deref() {
                            Some("ok") => view! { <span class="status-running">"OK"</span> }.into_view(),
                            Some(_) => view! { <span class="status-stopped">{format!("FAILED: {}", b.verify_error.clone().unwrap_or_default())}</span> }.into_view(),
                            None => view! { <span class="status-unknown">"not verified"</span> }.into_view(),
                        };
                        view! {
                            <tr class="verify-row" data-backup-id=b.id.clone()>
                                <td>{short(&b.id)}</td>
                                <td>{format!("{} ({})", b.target_id, b.target_name)}</td>
                                <td>{bk::format_time(b.timestamp)}</td>
                                <td class="verify-state">{state}</td>
                                <td>{b.verified_at.map(bk::format_time).unwrap_or_else(|| "-".into())}</td>
                                <td>
                                    <button class="btn btn-sm btn-secondary verify-btn"
                                        disabled=move || running.get().is_some()
                                        on:click=move |_| verify_one(id.clone())>
                                        {move || if running.get().as_deref() == Some(id2.as_str()) { "Verifying..." } else { "Verify" }}
                                    </button>
                                </td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}
