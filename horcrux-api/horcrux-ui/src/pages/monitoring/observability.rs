use leptos::*;

#[component]
pub fn ObservabilityPage() -> impl IntoView {
    view! {
        <div class="observability-page">
            <div class="page-header">
                <h1>"Observability Configuration"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary">
                        "Configure Integration"
                    </button>
                </div>
            </div>

            <div class="observability-sections">
                <div class="section">
                    <h2>"OpenTelemetry Integration"</h2>
                    <p>"Configure distributed tracing and observability."</p>
                    <div class="coming-soon">
                        <ul>
                            <li>"OTEL collector configuration"</li>
                            <li>"Trace sampling and export settings"</li>
                            <li>"Custom instrumentation setup"</li>
                            <li>"Service topology visualization"</li>
                        </ul>
                    </div>
                </div>

                <div class="section">
                    <h2>"Prometheus Integration"</h2>
                    <p>"Configure metric collection and storage."</p>
                    <div class="coming-soon">
                        <ul>
                            <li>"Prometheus configuration management"</li>
                            <li>"Scrape target discovery"</li>
                            <li>"Retention and storage policies"</li>
                            <li>"Federation and remote write"</li>
                        </ul>
                    </div>
                </div>

                <div class="section">
                    <h2>"Log Aggregation"</h2>
                    <p>"Configure centralized logging and analysis."</p>
                    <div class="coming-soon">
                        <ul>
                            <li>"Log shipping and collection"</li>
                            <li>"Log parsing and enrichment"</li>
                            <li>"Log retention policies"</li>
                            <li>"Search and analysis interface"</li>
                        </ul>
                    </div>
                </div>
            </div>
        </div>
    }
}