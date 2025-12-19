use leptos::*;

#[component]
pub fn DashboardsPage() -> impl IntoView {
    view! {
        <div class="dashboards-page">
            <div class="page-header">
                <h1>"Custom Dashboards"</h1>
                <div class="header-actions">
                    <button class="btn btn-primary">
                        "Create Dashboard"
                    </button>
                </div>
            </div>

            <div class="coming-soon">
                <h2>"Custom Dashboard Builder"</h2>
                <p>"Advanced dashboard creation and visualization tools coming soon."</p>
                <ul>
                    <li>"Drag-and-drop dashboard builder"</li>
                    <li>"Custom chart and graph creation"</li>
                    <li>"Real-time metric visualization"</li>
                    <li>"Dashboard sharing and templates"</li>
                    <li>"Time range selection and zooming"</li>
                    <li>"Multi-panel layouts"</li>
                </ul>
            </div>
        </div>
    }
}