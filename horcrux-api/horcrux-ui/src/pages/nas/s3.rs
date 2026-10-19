//! NAS > S3 Gateway: run the S3 server, manage buckets (browse, upload,
//! download, delete objects), S3 users and access keys (create, rotate,
//! disable, delete) - all over `/api/nas/s3/*` (horcrux-api api::s3_gateway).
use crate::api::s3::{
    self as s3, CreateS3AccessKey, CreateS3Bucket, CreateS3User, S3AccessKey, S3Bucket,
    S3GatewaySettings, S3GatewayStatus, S3Object, S3User,
};
use leptos::*;
use wasm_bindgen::JsCast;

/// A secret shown exactly once (new key, rotated key, new user).
#[derive(Clone, Debug, PartialEq)]
struct Secret {
    title: String,
    access_key: String,
    secret_key: String,
}

fn endpoint(st: &S3GatewayStatus) -> String {
    let host = st
        .address
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_default();
    let host = if host.is_empty() || host == "0.0.0.0" || host == "::" || host == "[::]" {
        web_sys::window()
            .and_then(|w| w.location().hostname().ok())
            .unwrap_or_else(|| "localhost".into())
    } else {
        host
    };
    format!("{}://{}:{}", st.scheme, host, st.port)
}

fn confirm(msg: &str) -> bool {
    web_sys::window()
        .and_then(|w| w.confirm_with_message(msg).ok())
        .unwrap_or(false)
}

fn fmt_time(t: i64) -> String {
    if t <= 0 {
        return "-".into();
    }
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "-".into())
}

#[component]
pub fn S3Page() -> impl IntoView {
    let (status, set_status) = create_signal(None::<S3GatewayStatus>);
    let (buckets, set_buckets) = create_signal(Vec::<S3Bucket>::new());
    let (keys, set_keys) = create_signal(Vec::<S3AccessKey>::new());
    let (users, set_users) = create_signal(Vec::<S3User>::new());
    let (error, set_error) = create_signal(None::<String>);
    let (notice, set_notice) = create_signal(None::<String>);
    let (busy, set_busy) = create_signal(false);
    let (secret, set_secret) = create_signal(None::<Secret>);
    let (tab, set_tab) = create_signal("buckets".to_string());

    // settings form
    let (show_settings, set_show_settings) = create_signal(false);
    let (f_address, set_f_address) = create_signal(String::new());
    let (f_port, set_f_port) = create_signal(String::new());
    let (f_cport, set_f_cport) = create_signal(String::new());
    let (f_region, set_f_region) = create_signal(String::new());
    let (f_dirs, set_f_dirs) = create_signal(String::new());
    let (f_server, set_f_server) = create_signal(String::new());
    let (f_client, set_f_client) = create_signal(String::new());

    // create forms
    let (b_name, set_b_name) = create_signal(String::new());
    let (b_versioning, set_b_versioning) = create_signal(false);
    let (k_user, set_k_user) = create_signal(String::new());
    let (k_desc, set_k_desc) = create_signal(String::new());
    let (u_name, set_u_name) = create_signal(String::new());
    let (u_policy, set_u_policy) = create_signal("readwrite".to_string());

    // object browser
    let (browsing, set_browsing) = create_signal(None::<String>);
    let (objects, set_objects) = create_signal(Vec::<S3Object>::new());
    let (o_prefix, set_o_prefix) = create_signal(String::new());

    let reload = move || {
        spawn_local(async move {
            match s3::status().await {
                Ok(st) => {
                    let running = st.running;
                    set_status.set(Some(st));
                    if running {
                        match s3::buckets().await {
                            Ok(b) => set_buckets.set(b),
                            Err(e) => set_error.set(Some(format!("Buckets: {}", e.message))),
                        }
                        match s3::access_keys().await {
                            Ok(k) => set_keys.set(k),
                            Err(e) => set_error.set(Some(format!("Access keys: {}", e.message))),
                        }
                        match s3::users().await {
                            Ok(u) => set_users.set(u),
                            Err(e) => set_error.set(Some(format!("Users: {}", e.message))),
                        }
                    } else {
                        set_buckets.set(vec![]);
                        set_keys.set(vec![]);
                        set_users.set(vec![]);
                    }
                }
                Err(e) => set_error.set(Some(format!("Gateway status: {}", e.message))),
            }
        });
    };
    let load_objects = move |bucket: String| {
        spawn_local(async move {
            match s3::objects(&bucket).await {
                Ok(o) => set_objects.set(o),
                Err(e) => set_error.set(Some(format!("Objects: {}", e.message))),
            }
        });
    };
    create_effect(move |_| reload());

    // run an action, show its error or a notice, reload
    let act = move |label: String,
                    fut: std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<String>, String>>>,
    >| {
        set_busy.set(true);
        set_error.set(None);
        set_notice.set(None);
        spawn_local(async move {
            match fut.await {
                Ok(msg) => set_notice.set(Some(msg.unwrap_or(label))),
                Err(e) => set_error.set(Some(e)),
            }
            set_busy.set(false);
            reload();
            if let Some(b) = browsing.get_untracked() {
                load_objects(b);
            }
        });
    };

    let start = move |_| {
        act(
            "S3 gateway started".into(),
            Box::pin(async { s3::start().await.map(|_| None).map_err(|e| e.message) }),
        )
    };
    let stop = move |_| {
        if confirm("Stop the S3 gateway? Clients lose access until it is started again.") {
            act(
                "S3 gateway stopped".into(),
                Box::pin(async { s3::stop().await.map(|_| None).map_err(|e| e.message) }),
            )
        }
    };
    let open_settings = move |_| {
        spawn_local(async move {
            match s3::settings().await {
                Ok(s) => {
                    set_f_address.set(s.address.unwrap_or_default());
                    set_f_port.set(s.port.map(|p| p.to_string()).unwrap_or_default());
                    set_f_cport.set(s.console_port.map(|p| p.to_string()).unwrap_or_default());
                    set_f_region.set(s.region.unwrap_or_default());
                    set_f_dirs.set(s.data_dirs.unwrap_or_default().join("\n"));
                    set_f_server.set(s.server_bin.unwrap_or_default());
                    set_f_client.set(s.client_bin.unwrap_or_default());
                    set_show_settings.set(true);
                }
                Err(e) => set_error.set(Some(format!("Settings: {}", e.message))),
            }
        });
    };
    let save_settings = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let req = S3GatewaySettings {
            address: Some(f_address.get()),
            port: f_port.get().trim().parse().ok(),
            console_port: f_cport.get().trim().parse().ok(),
            region: Some(f_region.get()),
            data_dirs: Some(
                f_dirs
                    .get()
                    .lines()
                    .map(|l| l.trim().to_string())
                    .filter(|l| !l.is_empty())
                    .collect(),
            ),
            server_bin: Some(f_server.get()),
            client_bin: Some(f_client.get()),
            ..Default::default()
        };
        set_show_settings.set(false);
        act(
            "Settings saved".into(),
            Box::pin(async move {
                s3::save_settings(&req)
                    .await
                    .map(|_| None)
                    .map_err(|e| e.message)
            }),
        )
    };

    let create_bucket = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let req = CreateS3Bucket {
            name: b_name.get().trim().to_string(),
            versioning: b_versioning.get(),
        };
        set_b_name.set(String::new());
        act(
            format!("Bucket {} created", req.name),
            Box::pin(async move {
                s3::create_bucket(&req)
                    .await
                    .map(|_| None)
                    .map_err(|e| e.message)
            }),
        )
    };
    let delete_bucket = move |b: S3Bucket| {
        let force = b.object_count > 0;
        let msg = if force {
            format!(
                "Delete bucket {} and its {} object(s)?",
                b.name, b.object_count
            )
        } else {
            format!("Delete bucket {}?", b.name)
        };
        if confirm(&msg) {
            if browsing.get_untracked().as_deref() == Some(b.name.as_str()) {
                set_browsing.set(None);
            }
            act(
                format!("Bucket {} deleted", b.name),
                Box::pin(async move {
                    s3::delete_bucket(&b.name, force)
                        .await
                        .map(|_| None)
                        .map_err(|e| e.message)
                }),
            )
        }
    };
    let browse = move |name: String| {
        set_o_prefix.set(String::new());
        set_objects.set(vec![]);
        set_browsing.set(Some(name.clone()));
        load_objects(name);
    };
    let upload = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let Some(bucket) = browsing.get() else { return };
        let input = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("s3-upload-file"))
            .and_then(|e| e.dyn_into::<web_sys::HtmlInputElement>().ok());
        let Some(input) = input else { return };
        let Some(file) = input.files().and_then(|f| f.get(0)) else {
            set_error.set(Some("Choose a file to upload".into()));
            return;
        };
        let prefix = o_prefix.get().trim().trim_matches('/').to_string();
        let key = if prefix.is_empty() {
            file.name()
        } else {
            format!("{}/{}", prefix, file.name())
        };
        input.set_value("");
        act(
            format!("Uploaded {}", key),
            Box::pin(async move {
                s3::upload_object(&bucket, &key, &file)
                    .await
                    .map(|_| None)
                    .map_err(|e| e.message)
            }),
        )
    };
    let download = move |key: String| {
        let Some(bucket) = browsing.get_untracked() else {
            return;
        };
        spawn_local(async move {
            if let Err(e) = s3::download_object(&bucket, &key).await {
                set_error.set(Some(format!("Download: {}", e.message)));
            }
        });
    };
    let delete_object = move |key: String| {
        let Some(bucket) = browsing.get_untracked() else {
            return;
        };
        if confirm(&format!("Delete {}?", key)) {
            act(
                format!("Deleted {}", key),
                Box::pin(async move {
                    s3::delete_object(&bucket, &key)
                        .await
                        .map(|_| None)
                        .map_err(|e| e.message)
                }),
            )
        }
    };

    let create_key = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let req = CreateS3AccessKey {
            user_id: Some(k_user.get()).filter(|u| !u.is_empty()),
            description: Some(k_desc.get()).filter(|d| !d.trim().is_empty()),
        };
        set_k_desc.set(String::new());
        act(
            "Access key created".into(),
            Box::pin(async move {
                let k = s3::create_access_key(&req).await.map_err(|e| e.message)?;
                set_secret.set(Some(Secret {
                    title: format!("New access key for {}", k.user_id),
                    access_key: k.access_key.clone(),
                    secret_key: k.secret_key.clone().unwrap_or_default(),
                }));
                Ok(Some(format!("Access key {} created", k.access_key)))
            }),
        )
    };
    let rotate = move |key: String| {
        if confirm(&format!(
            "Rotate {}? The current secret stops working immediately.",
            key
        )) {
            act(
                "Access key rotated".into(),
                Box::pin(async move {
                    let k = s3::rotate_access_key(&key).await.map_err(|e| e.message)?;
                    set_secret.set(Some(Secret {
                        title: format!("New secret for {}", k.access_key),
                        access_key: k.access_key.clone(),
                        secret_key: k.secret_key.clone().unwrap_or_default(),
                    }));
                    Ok(Some(format!("Access key {} rotated", k.access_key)))
                }),
            )
        }
    };
    let toggle_key = move |key: String, enable: bool| {
        act(
            format!(
                "Access key {} {}",
                key,
                if enable { "enabled" } else { "disabled" }
            ),
            Box::pin(async move {
                s3::set_access_key_enabled(&key, enable)
                    .await
                    .map(|_| None)
                    .map_err(|e| e.message)
            }),
        )
    };
    let delete_key = move |key: String| {
        if confirm(&format!("Delete access key {}?", key)) {
            act(
                format!("Access key {} deleted", key),
                Box::pin(async move {
                    s3::delete_access_key(&key)
                        .await
                        .map(|_| None)
                        .map_err(|e| e.message)
                }),
            )
        }
    };

    let create_user = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let req = CreateS3User {
            username: u_name.get().trim().to_string(),
            secret_key: None,
            policy: u_policy.get(),
        };
        set_u_name.set(String::new());
        act(
            "User created".into(),
            Box::pin(async move {
                let u = s3::create_user(&req).await.map_err(|e| e.message)?;
                set_secret.set(Some(Secret {
                    title: format!("S3 user {} ({})", u.username, u.policies.join(", ")),
                    access_key: u.username.clone(),
                    secret_key: u.secret_key.clone().unwrap_or_default(),
                }));
                Ok(Some(format!("S3 user {} created", u.username)))
            }),
        )
    };
    let delete_user = move |name: String| {
        if confirm(&format!("Delete S3 user {} and its access keys?", name)) {
            act(
                format!("S3 user {} deleted", name),
                Box::pin(async move {
                    s3::delete_user(&name)
                        .await
                        .map(|_| None)
                        .map_err(|e| e.message)
                }),
            )
        }
    };

    let running = move || status.get().map(|s| s.running).unwrap_or(false);

    view! {
        <div class="s3-page">
            <div class="page-header">
                <h1>"S3 Gateway"</h1>
                <p class="subtitle">"S3-compatible object storage (MinIO / Silo) run and managed by Horcrux"</p>
            </div>

            {move || error.get().map(|e| view! { <div class="alert alert-error" id="s3-error">{e}</div> })}
            {move || notice.get().map(|n| view! { <div class="alert alert-success" id="s3-notice">{n}</div> })}
            {move || secret.get().map(|s| view! {
                <div class="alert alert-warning secret-box" id="s3-secret">
                    <strong>{s.title.clone()}</strong>
                    <p>"Copy the secret now - it is not shown again."</p>
                    <div>"Access key: " <code id="s3-secret-access">{s.access_key.clone()}</code></div>
                    <div>"Secret key: " <code id="s3-secret-secret">{s.secret_key.clone()}</code></div>
                    <button class="btn btn-sm" on:click=move |_| set_secret.set(None)>"Done"</button>
                </div>
            })}

            // Gateway status card
            {move || status.get().map(|st| {
                let ep = endpoint(&st);
                view! {
                    <div class="status-card" id="s3-status" data-running=st.running.to_string()>
                        <div class="status-header">
                            <h3>"Gateway"</h3>
                            <span class=if st.running { "status-active" } else { "status-inactive" } id="s3-state">
                                {if st.running { "Running" } else { "Stopped" }}
                            </span>
                        </div>
                        {(!st.missing.is_empty()).then(|| view! {
                            <p class="error" id="s3-missing">
                                "Not installed on this node: " {st.missing.join(", ")}
                                ". Install the server and client (Gentoo: USE=s3-gateway pulls them in) or set their paths in Settings."
                            </p>
                        })}
                        <div class="status-info">
                            <div class="info-item"><span class="label">"S3 endpoint: "</span><code id="s3-endpoint">{ep.clone()}</code></div>
                            <div class="info-item"><span class="label">"Region: "</span>{st.region.clone()}</div>
                            <div class="info-item"><span class="label">"Root user: "</span><code>{st.root_user.clone()}</code>" (password generated, never shown; use access keys)"</div>
                            <div class="info-item"><span class="label">"Data: "</span>{st.data_dirs.join(", ")}</div>
                            {st.version.clone().map(|v| view! { <div class="info-item"><span class="label">"Version: "</span>{v}</div> })}
                            {st.uptime_secs.map(|u| view! { <div class="info-item"><span class="label">"Uptime: "</span>{format_uptime(u)}</div> })}
                            {st.running.then(|| view! {
                                <div class="info-item">
                                    <span class="label">"Usage: "</span>
                                    {format!("{} bucket(s), {} object(s), {} used, {} free",
                                        st.bucket_count, st.object_count,
                                        format_bytes(st.storage_used_bytes), format_bytes(st.storage_available_bytes))}
                                </div>
                            })}
                        </div>
                        <div class="status-actions">
                            {if st.running {
                                view! { <button class="btn btn-danger" id="s3-stop" disabled=busy on:click=stop>"Stop Gateway"</button> }.into_view()
                            } else {
                                view! { <button class="btn btn-success" id="s3-start" disabled=move || busy.get() on:click=start>"Start Gateway"</button> }.into_view()
                            }}
                            <button class="btn" id="s3-settings" on:click=open_settings>"Settings"</button>
                        </div>
                    </div>
                }
            })}

            {move || show_settings.get().then(|| view! {
                <form class="s3-settings-form" id="s3-settings-form" on:submit=save_settings>
                    <h3>"Gateway settings"</h3>
                    <p class="hint">"Saving restarts a running gateway."</p>
                    <div class="form-row">
                        <div class="form-group"><label>"Listen address"</label>
                            <input type="text" id="s3-f-address" prop:value=f_address on:input=move |ev| set_f_address.set(event_target_value(&ev))/></div>
                        <div class="form-group"><label>"S3 port"</label>
                            <input type="number" id="s3-f-port" prop:value=f_port on:input=move |ev| set_f_port.set(event_target_value(&ev))/></div>
                        <div class="form-group"><label>"Console port"</label>
                            <input type="number" id="s3-f-cport" prop:value=f_cport on:input=move |ev| set_f_cport.set(event_target_value(&ev))/></div>
                        <div class="form-group"><label>"Region"</label>
                            <input type="text" id="s3-f-region" prop:value=f_region on:input=move |ev| set_f_region.set(event_target_value(&ev))/></div>
                    </div>
                    <div class="form-group"><label>"Data directories (one per line; 4+ = erasure coding)"</label>
                        <textarea id="s3-f-dirs" prop:value=f_dirs on:input=move |ev| set_f_dirs.set(event_target_value(&ev))></textarea></div>
                    <div class="form-row">
                        <div class="form-group"><label>"Server binary (empty = minio or silo)"</label>
                            <input type="text" id="s3-f-server" prop:value=f_server on:input=move |ev| set_f_server.set(event_target_value(&ev))/></div>
                        <div class="form-group"><label>"Client binary (empty = mcli or mc)"</label>
                            <input type="text" id="s3-f-client" prop:value=f_client on:input=move |ev| set_f_client.set(event_target_value(&ev))/></div>
                    </div>
                    <div class="form-actions">
                        <button type="submit" class="btn btn-primary" id="s3-f-save">"Save"</button>
                        <button type="button" class="btn" on:click=move |_| set_show_settings.set(false)>"Cancel"</button>
                    </div>
                </form>
            })}

            <div class="tabs">
                {[("buckets", "Buckets"), ("keys", "Access Keys"), ("users", "Users")].into_iter().map(|(id, label)| view! {
                    <button
                        class=move || if tab.get() == id { "tab active" } else { "tab" }
                        id=format!("s3-tab-{}", id)
                        on:click=move |_| set_tab.set(id.to_string())
                    >{label}</button>
                }).collect_view()}
            </div>

            {move || if !running() {
                view! { <div class="no-data" id="s3-not-running"><p>"Start the gateway to manage buckets, users and access keys."</p></div> }.into_view()
            } else if tab.get() == "buckets" {
                view! {
                    <div class="tab-content" id="s3-buckets">
                        <form class="inline-form" id="s3-bucket-form" on:submit=create_bucket>
                            <input type="text" id="s3-bucket-name" placeholder="bucket-name" required
                                pattern="[a-z0-9][a-z0-9.\\-]{1,61}[a-z0-9]"
                                title="3-63 lowercase letters, digits, '-' or '.'"
                                prop:value=b_name on:input=move |ev| set_b_name.set(event_target_value(&ev))/>
                            <label class="checkbox"><input type="checkbox" id="s3-bucket-versioning"
                                prop:checked=b_versioning on:change=move |_| set_b_versioning.update(|v| *v = !*v)/>" Versioning"</label>
                            <button type="submit" class="btn btn-primary" id="s3-bucket-create" disabled=busy>"Create Bucket"</button>
                        </form>
                        {move || {
                            let list = buckets.get();
                            if list.is_empty() {
                                view! { <div class="no-data"><p>"No buckets yet."</p></div> }.into_view()
                            } else {
                                view! {
                                    <table class="data-table" id="s3-bucket-table">
                                        <thead><tr><th>"Bucket"</th><th>"Objects"</th><th>"Size"</th><th>"Versioning"</th><th>"Created"</th><th>"Actions"</th></tr></thead>
                                        <tbody>
                                            {list.into_iter().map(|b| {
                                                let name = b.name.clone();
                                                let bd = b.clone();
                                                view! {
                                                    <tr data-bucket=b.name.clone()>
                                                        <td><strong>{b.name.clone()}</strong></td>
                                                        <td>{b.object_count}</td>
                                                        <td>{format_bytes(b.size_bytes)}</td>
                                                        <td>{if b.versioning { "Enabled" } else { "Off" }}</td>
                                                        <td>{fmt_time(b.created_at)}</td>
                                                        <td class="actions">
                                                            <button class="btn btn-sm s3-browse" on:click=move |_| browse(name.clone())>"Browse"</button>
                                                            <button class="btn btn-sm btn-danger s3-bucket-delete" on:click=move |_| delete_bucket(bd.clone())>"Delete"</button>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }
                        }}
                        {move || browsing.get().map(|bucket| view! {
                            <div class="object-browser" id="s3-objects" data-bucket=bucket.clone()>
                                <h3>{format!("Objects in {}", bucket)}</h3>
                                <form class="inline-form" id="s3-upload-form" on:submit=upload>
                                    <input type="text" id="s3-upload-prefix" placeholder="folder/ (optional)"
                                        prop:value=o_prefix on:input=move |ev| set_o_prefix.set(event_target_value(&ev))/>
                                    <input type="file" id="s3-upload-file"/>
                                    <button type="submit" class="btn btn-primary" id="s3-upload" disabled=busy>"Upload"</button>
                                    <button type="button" class="btn" on:click=move |_| set_browsing.set(None)>"Close"</button>
                                </form>
                                {move || {
                                    let list = objects.get();
                                    if list.is_empty() {
                                        view! { <p class="no-data">"Empty bucket."</p> }.into_view()
                                    } else {
                                        view! {
                                            <table class="data-table" id="s3-object-table">
                                                <thead><tr><th>"Key"</th><th>"Size"</th><th>"Modified"</th><th>"Actions"</th></tr></thead>
                                                <tbody>
                                                    {list.into_iter().map(|o| {
                                                        let k1 = o.key.clone();
                                                        let k2 = o.key.clone();
                                                        view! {
                                                            <tr data-key=o.key.clone()>
                                                                <td><code>{o.key.clone()}</code></td>
                                                                <td>{format_bytes(o.size)}</td>
                                                                <td>{fmt_time(o.last_modified)}</td>
                                                                <td class="actions">
                                                                    <button class="btn btn-sm s3-download" on:click=move |_| download(k1.clone())>"Download"</button>
                                                                    <button class="btn btn-sm btn-danger s3-object-delete" on:click=move |_| delete_object(k2.clone())>"Delete"</button>
                                                                </td>
                                                            </tr>
                                                        }
                                                    }).collect_view()}
                                                </tbody>
                                            </table>
                                        }.into_view()
                                    }
                                }}
                            </div>
                        })}
                    </div>
                }.into_view()
            } else if tab.get() == "keys" {
                view! {
                    <div class="tab-content" id="s3-keys">
                        <form class="inline-form" id="s3-key-form" on:submit=create_key>
                            <select id="s3-key-user" on:change=move |ev| set_k_user.set(event_target_value(&ev))>
                                <option value="" selected=move || k_user.get().is_empty()>
                                    {move || format!("{} (root, full access)", status.get().map(|s| s.root_user).unwrap_or_default())}
                                </option>
                                {move || users.get().into_iter().map(|u| {
                                    let n = u.username.clone();
                                    view! { <option value=n.clone() selected=move || k_user.get() == n>{format!("{} ({})", u.username, u.policies.join(", "))}</option> }
                                }).collect_view()}
                            </select>
                            <input type="text" id="s3-key-desc" placeholder="Description (e.g. backup script)"
                                prop:value=k_desc on:input=move |ev| set_k_desc.set(event_target_value(&ev))/>
                            <button type="submit" class="btn btn-primary" id="s3-key-create" disabled=busy>"Create Access Key"</button>
                        </form>
                        {move || {
                            let list = keys.get();
                            if list.is_empty() {
                                view! { <div class="no-data"><p>"No access keys yet."</p></div> }.into_view()
                            } else {
                                view! {
                                    <table class="data-table" id="s3-key-table">
                                        <thead><tr><th>"Access key"</th><th>"User"</th><th>"Description"</th><th>"Status"</th><th>"Created"</th><th>"Actions"</th></tr></thead>
                                        <tbody>
                                            {list.into_iter().map(|k| {
                                                let (k1, k2, k3) = (k.access_key.clone(), k.access_key.clone(), k.access_key.clone());
                                                let enabled = k.enabled;
                                                view! {
                                                    <tr data-key=k.access_key.clone() data-enabled=k.enabled.to_string()>
                                                        <td><code>{k.access_key.clone()}</code></td>
                                                        <td>{k.user_id.clone()}</td>
                                                        <td>{k.description.clone().unwrap_or_default()}</td>
                                                        <td><span class=if k.enabled { "status-active" } else { "status-inactive" }>
                                                            {if k.enabled { "Active" } else { "Disabled" }}</span></td>
                                                        <td>{fmt_time(k.created_at)}</td>
                                                        <td class="actions">
                                                            {if k.managed {
                                                                view! {
                                                                    <button class="btn btn-sm s3-key-rotate" on:click=move |_| rotate(k1.clone())>"Rotate"</button>
                                                                    <button class="btn btn-sm s3-key-toggle" on:click=move |_| toggle_key(k2.clone(), !enabled)>
                                                                        {if enabled { "Disable" } else { "Enable" }}</button>
                                                                    <button class="btn btn-sm btn-danger s3-key-delete" on:click=move |_| delete_key(k3.clone())>"Delete"</button>
                                                                }.into_view()
                                                            } else {
                                                                view! { <span class="hint">"created outside Horcrux"</span> }.into_view()
                                                            }}
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }
                        }}
                    </div>
                }.into_view()
            } else {
                view! {
                    <div class="tab-content" id="s3-users">
                        <form class="inline-form" id="s3-user-form" on:submit=create_user>
                            <input type="text" id="s3-user-name" placeholder="username" required
                                prop:value=u_name on:input=move |ev| set_u_name.set(event_target_value(&ev))/>
                            <select id="s3-user-policy" on:change=move |ev| set_u_policy.set(event_target_value(&ev))>
                                <option value="readwrite">"readwrite"</option>
                                <option value="readonly">"readonly"</option>
                                <option value="writeonly">"writeonly"</option>
                                <option value="consoleAdmin">"consoleAdmin (full admin)"</option>
                            </select>
                            <button type="submit" class="btn btn-primary" id="s3-user-create" disabled=busy>"Create User"</button>
                        </form>
                        {move || {
                            let list = users.get();
                            if list.is_empty() {
                                view! { <div class="no-data"><p>"No S3 users yet (only the root user)."</p></div> }.into_view()
                            } else {
                                view! {
                                    <table class="data-table" id="s3-user-table">
                                        <thead><tr><th>"User"</th><th>"Policy"</th><th>"Status"</th><th>"Actions"</th></tr></thead>
                                        <tbody>
                                            {list.into_iter().map(|u| {
                                                let n = u.username.clone();
                                                view! {
                                                    <tr data-user=u.username.clone()>
                                                        <td>{u.username.clone()}</td>
                                                        <td>{u.policies.join(", ")}</td>
                                                        <td>{if u.enabled { "Enabled" } else { "Disabled" }}</td>
                                                        <td class="actions">
                                                            <button class="btn btn-sm btn-danger s3-user-delete" on:click=move |_| delete_user(n.clone())>"Delete"</button>
                                                        </td>
                                                    </tr>
                                                }
                                            }).collect_view()}
                                        </tbody>
                                    </table>
                                }.into_view()
                            }
                        }}
                    </div>
                }.into_view()
            }}
        </div>
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;

    if days > 0 {
        format!("{}d {}h {}m", days, hours, mins)
    } else if hours > 0 {
        format!("{}h {}m", hours, mins)
    } else {
        format!("{}m {}s", mins, secs % 60)
    }
}
