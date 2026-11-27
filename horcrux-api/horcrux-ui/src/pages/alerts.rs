use crate::api;
use leptos::*;

#[component]
pub fn Alerts() -> impl IntoView {
    let (alerts, set_alerts) = create_signal(Vec::<api::ActiveAlert>::new());
    let (loading, set_loading) = create_signal(true);
    // A failed load used to read "No active alerts": say it failed instead.
    let (error, set_error) = create_signal(None::<String>);

    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match api::get_active_alerts().await {
                Ok(alert_list) => {
                    set_alerts.set(alert_list);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(format!("Could not load the alerts: {}", e.message))),
            }
            set_loading.set(false);
        });
    });

    view! {
        <div class="alerts-page">
            <h1>"Alert Monitoring"</h1>

            {move || {
                if loading.get() {
                    view! { <p class="loading">"Loading alerts..."</p> }.into_view()
                } else if let Some(e) = error.get() {
                    view! { <div class="alert alert-error" id="alerts-error" role="alert">{e}</div> }.into_view()
                } else {
                    let alert_list = alerts.get();
                    if alert_list.is_empty() {
                        view! { <p class="no-data">"No active alerts"</p> }.into_view()
                    } else {
                        view! {
                            <div class="alerts-grid">
                                {alert_list.into_iter().map(|alert| {
                                    let severity_class = format!("alert-card severity-{}", alert.severity.to_lowercase());
                                    view! {
                                        <div class={severity_class}>
                                            <div class="alert-header">
                                                <h3>{&alert.rule_name}</h3>
                                                <span class="alert-status">{&alert.status}</span>
                                            </div>
                                            <p class="alert-target"><strong>"Target:"</strong> " " {&alert.target}</p>
                                            <p class="alert-message">{&alert.message}</p>
                                            <p class="alert-time">
                                                "Fired at: "
                                                {api::format_unix_time(alert.fired_at)}
                                            </p>
                                        </div>
                                    }
                                }).collect_view()}
                            </div>
                        }.into_view()
                    }
                }
            }}
        </div>
    }
}
