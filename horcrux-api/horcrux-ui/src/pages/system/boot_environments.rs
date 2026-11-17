use crate::api::*;
use leptos::*;

fn when(t: i64) -> String {
    let d = web_sys::js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(t as f64 * 1000.0));
    d.to_locale_string("default", &wasm_bindgen::JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_default()
}

fn ask(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

/// The banner on the Packages page: the snapshot taken before the last upgrade and a one-click way back.
/// Hidden on nodes without ZFS boot environments, and once the running environment has no upgrade snapshot
/// (right after a rollback). `tick` re-reads it (the page bumps it when an upgrade task starts).
#[component]
pub fn PreUpgradeBanner(tick: ReadSignal<u32>) -> impl IntoView {
    let (info, set_info) = create_signal(None::<BootEnvInfo>);
    let (msg, set_msg) = create_signal(None::<Result<String, String>>);
    let (busy, set_busy) = create_signal(false);
    let load = create_action(move |_: &()| async move {
        // 424 (no ZFS root / no horcrux-be) and every other failure just hide the banner: it is a convenience, not the page
        set_info.set(get_boot_environments().await.ok());
    });
    create_effect(move |_| {
        tick.get();
        load.dispatch(());
    });
    let rollback = create_action(move |snap: &String| {
        let snap = snap.clone();
        async move {
            set_busy.set(true);
            let r = boot_env_action("/rollback", serde_json::json!({ "snapshot": snap })).await;
            set_msg.set(Some(match r {
                Ok(_) => Ok("Done. The system as it was before that upgrade boots at the next reboot (nothing rebooted now). VM disks and /home are untouched.".to_string()),
                Err(e) => Err(format!("Roll back failed: {e}")),
            }));
            set_busy.set(false);
            load.dispatch(());
        }
    });
    view! {
        {move || msg.get().map(|m| match m {
            Ok(t) => view! { <div class="alert alert-success" id="pre-upgrade-rolled-back">{t}</div> }.into_view(),
            Err(t) => view! { <div class="alert alert-error">{t}</div> }.into_view(),
        })}
        {move || info.get().and_then(|i| i.last_upgrade_snapshot().cloned()).map(|s| {
            let full = format!("{}@{}", s.be, s.name);
            let target = full.clone();
            view! {
                <div class="alert alert-info" id="pre-upgrade-banner">
                    "Before the last upgrade Horcrux saved the system as " <a href="/system/boot-environments"><strong>{full}</strong></a>
                    " (" {when(s.created)} "). If something broke, you can boot the system as it was: "
                    <button class="btn btn-sm btn-warning" id="pre-upgrade-rollback" disabled=busy
                        on:click=move |_| if ask("Boot the system as it was before the last upgrade? Nothing reboots now; the next reboot enters the previous version. VM disks and /home are not touched.") {
                            rollback.dispatch(target.clone());
                        }>"Roll back to before this upgrade"</button>
                </div>
            }
        })}
    }
}

/// Boot environments: every system upgrade is snapshotted first; roll back to the previous version from here
/// (takes effect at the next reboot; VM disks, /home and logs are separate datasets and are never rolled back).
#[component]
pub fn BootEnvironmentsPage() -> impl IntoView {
    let (info, set_info) = create_signal(None::<BootEnvInfo>);
    let (busy, set_busy) = create_signal(false);
    let (error_message, set_error) = create_signal(None::<String>);
    let (ok_message, set_ok) = create_signal(None::<String>);
    let (unavailable, set_unavailable) = create_signal(None::<Unavailable>);

    let load = create_action(move |_: &()| async move {
        match get_boot_environments().await {
            Ok(i) => {
                set_unavailable.set(None);
                set_info.set(Some(i));
            }
            Err(e) => match e.unavailable() {
                Some(u) => set_unavailable.set(Some(u)),
                None => set_error.set(Some(format!("Failed to load boot environments: {e}"))),
            },
        }
    });
    load.dispatch(());

    // (path, body, confirmation) -> POST, or DELETE when the body is null
    let act = create_action(
        move |(path, body, label): &(String, serde_json::Value, String)| {
            let (path, body, label) = (path.clone(), body.clone(), label.clone());
            async move {
                set_busy.set(true);
                set_error.set(None);
                set_ok.set(None);
                let r = if body.is_null() {
                    delete_boot_environment(&path)
                        .await
                        .map(|_| format!("{label}: done"))
                } else {
                    boot_env_action(&path, body)
                        .await
                        .map(|m| format!("{label}: {m}"))
                };
                match r {
                    Ok(m) => set_ok.set(Some(m)),
                    Err(e) => set_error.set(Some(format!("{label} failed: {e}"))),
                }
                set_busy.set(false);
                load.dispatch(());
            }
        },
    );
    let post = move |path: &str, body: serde_json::Value, label: &str, confirm: Option<String>| {
        if confirm.map(|c| ask(&c)).unwrap_or(true) {
            act.dispatch((path.to_string(), body, label.to_string()));
        }
    };

    view! {
        <div class="be-page">
            <div class="page-header">
                <h1>"Boot Environments"</h1>
                <div class="header-actions">
                    <button class="btn btn-secondary" disabled=busy on:click=move |_| load.dispatch(())>"Refresh"</button>
                    <button class="btn btn-secondary" disabled=busy
                        on:click=move |_| post("/snapshot", serde_json::json!({}), "Snapshot", None)>"Take snapshot"</button>
                    <button class="btn btn-warning" disabled=busy
                        on:click=move |_| post("/rollback", serde_json::json!({}), "Roll back",
                            Some("Boot the previous version? This creates a boot environment from the newest pre-upgrade snapshot and makes it the next boot. Nothing reboots now; VM disks and /home are not touched.".into()))>
                        "Roll back to previous version"
                    </button>
                </div>
            </div>

            <p class="help-text">
                "Before every system upgrade Horcrux snapshots the root file system. Roll back to boot the system exactly as it was; "
                "the newest " {move || info.get().map(|i| i.keep).unwrap_or(5)} " automatic snapshots are kept, pinned ones forever."
            </p>

            {move || error_message.get().map(|m| view! { <div class="alert alert-error">{m}</div> })}
            {move || ok_message.get().map(|m| view! { <div class="alert alert-success">{m}</div> })}
            {move || unavailable.get().map(|u| view! {
                <crate::components::UnavailableState info=u on_retry=Callback::new(move |_| load.dispatch(()))/>
            })}

            {move || info.get().filter(|_| unavailable.get().is_none()).map(|i| {
                let unsafe_paths: Vec<_> = i.data.iter().filter(|d| !d.safe).cloned().collect();
                view! {
                    {(!unsafe_paths.is_empty()).then(|| view! {
                        <div class="alert alert-error">
                            "These directories live on the root boot environment itself, so a rollback would cut you off from their current contents: "
                            {unsafe_paths.iter().map(|d| d.path.clone()).collect::<Vec<_>>().join(", ")}
                            ". Move them to their own datasets before rolling back."
                            <button class="btn btn-sm btn-primary" id="be-migrate" disabled=busy
                                on:click=move |_| post("/migrate-data", serde_json::json!({}), "Move data",
                                    Some("Move these directories onto their own datasets? Stop the VMs and services that use them first; the originals stay in a snapshot until you destroy it. This can take a while for big VM disks and runs as a task.".into()))>
                                "Move them now"
                            </button>
                        </div>
                    })}
                    {i.fallback.trial.clone().map(|t| view! {
                        <div class="alert alert-warning" id="be-trial">
                            "'" {t.clone()} "' is on trial: if it does not come up healthy in " {i.fallback.tries_max}
                            " boots (" {i.fallback.tries} " so far), " {i.fallback.good.clone().unwrap_or_else(|| "the previous environment".into())}
                            " boots again by itself."
                        </div>
                    })}
                    {i.fallback.failed.clone().map(|f| view! {
                        <div class="alert alert-error" id="be-fallback-pending">
                            "GRUB gave up on '" {f} "' and booted the previous environment instead."
                        </div>
                    })}
                    <h2>"Environments"</h2>
                    <table class="data-table">
                        <thead><tr><th>"Name"</th><th>"State"</th><th>"Kernel"</th><th>"Created"</th><th>"Actions"</th></tr></thead>
                        <tbody>
                            {i.environments.iter().map(|e| {
                                let n1 = e.name.clone(); let n2 = e.name.clone(); let n3 = e.name.clone(); let n4 = e.name.clone();
                                let on_trial = i.fallback.trial.as_deref() == Some(e.name.as_str());
                                let state = [(e.running, "running"), (e.active, "boots next"), (e.pinned, "pinned"),
                                             (on_trial, "on trial"), (e.failed, "did not boot healthy")]
                                    .iter().filter(|(on, _)| *on).map(|(_, t)| *t).collect::<Vec<_>>().join(", ");
                                let (running, active, pinned) = (e.running, e.active, e.pinned);
                                view! {
                                    <tr>
                                        <td><strong>{e.name.clone()}</strong></td>
                                        <td>{state}</td>
                                        <td>{e.kernel.clone()}</td>
                                        <td>{when(e.created)}</td>
                                        <td class="actions">
                                            <button class="btn btn-xs btn-primary" disabled=move || busy.get() || active
                                                on:click=move |_| post(&format!("/{n1}/activate"), serde_json::json!({}), "Activate",
                                                    Some(format!("Boot '{n1}' at the next reboot?")))>"Boot next"</button>
                                            <button class="btn btn-xs" disabled=busy
                                                on:click=move |_| post("/pin", serde_json::json!({"target": n2, "pinned": !pinned}), "Pin", None)>
                                                {if pinned { "Unpin" } else { "Pin" }}</button>
                                            <button class="btn btn-xs btn-danger" disabled=move || busy.get() || running || active || pinned
                                                on:click=move |_| post(&format!("/{n3}"), serde_json::Value::Null, "Destroy",
                                                    Some(format!("Destroy boot environment '{n4}'? Its root file system is deleted.")))>"Destroy"</button>
                                        </td>
                                    </tr>
                                }
                            }).collect_view()}
                        </tbody>
                    </table>

                    <h2>"Snapshots"</h2>
                    {if i.snapshots.is_empty() {
                        view! { <div class="empty-state">"No snapshots yet. One is taken automatically before the next system upgrade; "<strong>"Take snapshot"</strong>" makes one now."</div> }.into_view()
                    } else {
                        view! {
                            <table class="data-table">
                                <thead><tr><th>"Snapshot"</th><th>"Kernel"</th><th>"Created"</th><th>"Kind"</th><th>"Actions"</th></tr></thead>
                                <tbody>
                                    {i.snapshots.iter().rev().map(|s| {
                                        let full = format!("{}@{}", s.be, s.name);
                                        let (f1, f2, f3, f4) = (full.clone(), full.clone(), full.clone(), full.clone());
                                        let (be, sn) = (s.be.clone(), s.name.clone());
                                        let pinned = s.pinned;
                                        view! {
                                            <tr>
                                                <td><strong>{full}</strong>{pinned.then_some(" (pinned)")}</td>
                                                <td>{s.kernel.clone()}</td>
                                                <td>{when(s.created)}</td>
                                                <td>{if s.auto { "automatic" } else { "manual" }}</td>
                                                <td class="actions">
                                                    <button class="btn btn-xs btn-warning" disabled=busy
                                                        on:click=move |_| post("/rollback", serde_json::json!({"snapshot": f1}), "Roll back",
                                                            Some(format!("Create a boot environment from {f2} and boot it next? Nothing reboots now.")))>"Roll back to this"</button>
                                                    <button class="btn btn-xs" disabled=busy
                                                        on:click=move |_| post("/pin", serde_json::json!({"target": f3, "pinned": !pinned}), "Pin", None)>
                                                        {if pinned { "Unpin" } else { "Pin" }}</button>
                                                    <button class="btn btn-xs btn-danger" disabled=move || busy.get() || pinned
                                                        on:click=move |_| post(&format!("/snapshots/{be}/{sn}"), serde_json::Value::Null, "Destroy snapshot",
                                                            Some(format!("Destroy snapshot {f4}?")))>"Destroy"</button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_view()
                    }}

                    <h2>"Boot events"</h2>
                    {if i.events.is_empty() {
                        view! { <div class="empty-state">"Nothing yet: activating or rolling back an environment, and any automatic fallback, are recorded here."</div> }.into_view()
                    } else {
                        view! {
                            <table class="data-table" id="be-events">
                                <thead><tr><th>"When"</th><th>"Event"</th><th>"Details"</th></tr></thead>
                                <tbody>
                                    {i.events.iter().rev().map(|e| {
                                        let label = match e.kind.as_str() {
                                            "fallback" => "Automatic fallback",
                                            "boot-ok" => "Booted healthy",
                                            "armed" => "On trial",
                                            "health-failed" => "Health check failed",
                                            "snapshot" => "Upgrade snapshot",
                                            "migrated" => "Data moved",
                                            other => other,
                                        }.to_string();
                                        view! { <tr class=format!("be-event-{}", e.kind)>
                                            <td>{when(e.ts)}</td><td><strong>{label}</strong></td>
                                            <td>{e.detail.clone().or_else(|| e.be.clone()).unwrap_or_default()}</td></tr> }
                                    }).collect_view()}
                                </tbody>
                            </table>
                        }.into_view()
                    }}

                    <h2>"Never rolled back"</h2>
                    <p class="help-text">"These datasets are outside every boot environment: VM disks, user data and logs stay exactly as they are."</p>
                    <ul>
                        {i.preserved.iter().cloned().map(|d| view! { <li><code>{d}</code></li> }).collect_view()}
                    </ul>
                }
            })}
        </div>
    }
}
