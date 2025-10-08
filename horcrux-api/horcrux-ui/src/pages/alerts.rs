use leptos::*;
use crate::api;

#[component]
pub fn Alerts() -> impl IntoView {
    let (alerts, set_alerts) = create_signal(Vec::<api::Alert>::new());
    let (loading, set_loading) = create_signal(true);

    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            if let Ok(alert_list) = api::get_active_alerts().await {
                set_alerts.set(alert_list);
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
                                                {chrono::DateTime::from_timestamp(alert.fired_at, 0)
                                                    .map(|dt| dt.to_rfc2822())
                                                    .unwrap_or_else(|| "Unknown".to_string())}
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
