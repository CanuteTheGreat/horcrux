//! NAS Services Management Page

use leptos::*;

#[derive(Clone, Debug, serde::Deserialize)]
#[allow(dead_code)]
pub struct NasService {
    pub name: String,
    pub display_name: String,
    pub running: bool,
    pub enabled: bool,
    pub description: Option<String>,
}

#[component]
pub fn ServicesPage() -> impl IntoView {
    let (services, set_services) = create_signal(Vec::<NasService>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match reqwasm::http::Request::get("/api/nas/services")
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.ok() {
                        if let Ok(data) = resp.json::<Vec<NasService>>().await {
                            set_services.set(data);
                            set_error.set(None);
                        }
                    } else {
                        set_error.set(Some("Failed to load services".to_string()));
                    }
                }
                Err(e) => set_error.set(Some(format!("Network error: {}", e))),
            }
            set_loading.set(false);
        });
    });

    let service_action = move |name: String, action: &'static str| {
        spawn_local(async move {
            let _ = reqwasm::http::Request::post(&format!("/api/nas/services/{}/{}", name, action))
                .send()
                .await;
        });
    };

    view! {
        <div class="services-page">
            <div class="page-header">
                <h1>"NAS Services"</h1>
                <p class="subtitle">"Manage file sharing and storage services"</p>
            </div>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading services..."</p> }.into_view()
                } else if let Some(err) = error.get() {
                    view! { <p class="error">"Error: " {err}</p> }.into_view()
                } else {
                    view! {
                        <div class="service-cards">
                            {services.get().into_iter().map(|service| {
                                let name_start = service.name.clone();
                                let name_stop = service.name.clone();
                                let name_restart = service.name.clone();
                                let status_class = if service.running { "status-running" } else { "status-stopped" };

                                view! {
                                    <div class="service-card">
                                        <div class="service-header">
                                            <h3>{&service.display_name}</h3>
                                            <span class={status_class}>
                                                {if service.running { "Running" } else { "Stopped" }}
                                            </span>
                                        </div>
                                        <p class="service-description">
                                            {service.description.clone().unwrap_or_else(|| "No description".to_string())}
                                        </p>
                                        <div class="service-actions">
                                            <button
                                                class="btn btn-sm btn-success"
                                                disabled=service.running
                                                on:click=move |_| service_action(name_start.clone(), "start")
                                            >"Start"</button>
                                            <button
                                                class="btn btn-sm btn-danger"
                                                disabled=!service.running
                                                on:click=move |_| service_action(name_stop.clone(), "stop")
                                            >"Stop"</button>
                                            <button
                                                class="btn btn-sm"
                                                on:click=move |_| service_action(name_restart.clone(), "restart")
                                            >"Restart"</button>
                                        </div>
                                    </div>
                                }
                            }).collect_view()}
                        </div>
                    }.into_view()
                }
            }}
        </div>
    }
}
