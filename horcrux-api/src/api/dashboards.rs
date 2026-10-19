//! Monitoring > Dashboards: user-defined dashboards.
//!
//! A dashboard is a named list of chart panels, each a Metrics Explorer
//! query (`horcrux_node_cpu_usage_percent`, `avg(horcrux_vm_cpu_usage_percent)`,
//! `horcrux_storage_usage_percent{pool="local"}`...) drawn as a line, area or
//! bar chart or a single-value stat over the dashboard's time range. The
//! page renders panels from `POST /api/metrics/query`.
//!
//! - `GET|POST /api/monitoring/dashboards`
//! - `GET|PUT|DELETE /api/monitoring/dashboards/:id`
//!
//! Dashboards are stored in `config_objects` (kind `monitoring_dashboard`)
//! and shared by all operators (editing needs Sys.Modify on `/`).
use crate::db::config_store::ConfigStore;
use crate::error::ApiError;
use crate::middleware::auth::AuthUser;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::get,
    Extension, Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const KIND: &str = "monitoring_dashboard";
const MAX_PANELS: usize = 24;
pub const CHARTS: &[&str] = &["line", "area", "bar", "stat"];
pub const RANGES: &[&str] = &["15m", "1h", "6h", "24h"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DashboardPanel {
    #[serde(default)]
    pub id: String,
    pub title: String,
    /// Metrics Explorer query
    pub query: String,
    /// line | area | bar | stat
    #[serde(default = "default_chart")]
    pub chart: String,
    /// 1 = half width, 2 = full width
    #[serde(default = "default_width")]
    pub width: u8,
}

fn default_chart() -> String {
    "line".into()
}
fn default_width() -> u8 {
    1
}
fn default_range() -> String {
    "1h".into()
}
fn default_refresh() -> u32 {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// 15m | 1h | 6h | 24h
    #[serde(default = "default_range")]
    pub time_range: String,
    /// Auto-refresh period in seconds (0 = off)
    #[serde(default = "default_refresh")]
    pub refresh_secs: u32,
    #[serde(default)]
    pub panels: Vec<DashboardPanel>,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// Body of POST / PUT.
#[derive(Debug, Clone, Deserialize)]
pub struct DashboardRequest {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_range")]
    pub time_range: String,
    #[serde(default = "default_refresh")]
    pub refresh_secs: u32,
    #[serde(default)]
    pub panels: Vec<DashboardPanel>,
}

pub fn routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    let store = Arc::new(ConfigStore::new(&state.database));
    Router::new()
        .route("/api/monitoring/dashboards", get(list).post(create))
        .route(
            "/api/monitoring/dashboards/:id",
            get(get_one).put(update).delete(remove),
        )
        .layer(Extension(store))
}

type Store = Extension<Arc<ConfigStore>>;

fn validate(req: &DashboardRequest) -> Result<Vec<DashboardPanel>, ApiError> {
    let bad = |m: String| Err(ApiError::ValidationError(m));
    if req.name.trim().is_empty() || req.name.len() > 100 {
        return bad("Dashboard name is required (at most 100 characters)".into());
    }
    if !RANGES.contains(&req.time_range.as_str()) {
        return bad(format!(
            "Time range '{}' is not one of {}",
            req.time_range,
            RANGES.join(", ")
        ));
    }
    if req.refresh_secs != 0 && !(5..=3600).contains(&req.refresh_secs) {
        return bad("Refresh must be 0 (off) or 5-3600 seconds".into());
    }
    if req.panels.len() > MAX_PANELS {
        return bad(format!("At most {} panels per dashboard", MAX_PANELS));
    }
    let mut panels = Vec::with_capacity(req.panels.len());
    for (i, p) in req.panels.iter().enumerate() {
        let n = i + 1;
        if p.title.trim().is_empty() || p.title.len() > 100 {
            return bad(format!("Panel {}: a title is required", n));
        }
        if !CHARTS.contains(&p.chart.as_str()) {
            return bad(format!(
                "Panel {}: chart '{}' is not one of {}",
                n,
                p.chart,
                CHARTS.join(", ")
            ));
        }
        if !(1..=2).contains(&p.width) {
            return bad(format!("Panel {}: width is 1 (half) or 2 (full)", n));
        }
        crate::metrics_explorer::validate_query(p.query.trim())
            .map_err(|e| ApiError::ValidationError(format!("Panel {}: {}", n, e)))?;
        let mut p = p.clone();
        p.title = p.title.trim().to_string();
        p.query = p.query.trim().to_string();
        if p.id.is_empty() {
            p.id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        }
        panels.push(p);
    }
    Ok(panels)
}

async fn list(Extension(store): Store) -> Result<Json<Vec<Dashboard>>, ApiError> {
    Ok(Json(store.load_values::<Dashboard>(KIND).await?))
}

async fn find(store: &ConfigStore, id: &str) -> Result<Dashboard, ApiError> {
    store
        .load::<Dashboard>(KIND)
        .await?
        .into_iter()
        .find(|(i, _)| i == id)
        .map(|(_, d)| d)
        .ok_or_else(|| ApiError::NotFound(format!("Dashboard {} not found", id)))
}

async fn get_one(
    Extension(store): Store,
    Path(id): Path<String>,
) -> Result<Json<Dashboard>, ApiError> {
    Ok(Json(find(&store, &id).await?))
}

async fn create(
    State(_state): State<Arc<AppState>>,
    Extension(store): Store,
    user: Option<Extension<AuthUser>>,
    Json(req): Json<DashboardRequest>,
) -> Result<(StatusCode, Json<Dashboard>), ApiError> {
    let panels = validate(&req)?;
    let now = chrono::Utc::now().timestamp();
    let d = Dashboard {
        id: uuid::Uuid::new_v4().to_string(),
        name: req.name.trim().to_string(),
        description: req.description.trim().to_string(),
        time_range: req.time_range,
        refresh_secs: req.refresh_secs,
        panels,
        created_by: user.map(|Extension(u)| u.username).unwrap_or_default(),
        created_at: now,
        updated_at: now,
    };
    store.put(KIND, &d.id, &d).await?;
    Ok((StatusCode::CREATED, Json(d)))
}

async fn update(
    Extension(store): Store,
    Path(id): Path<String>,
    Json(req): Json<DashboardRequest>,
) -> Result<Json<Dashboard>, ApiError> {
    let mut d = find(&store, &id).await?;
    d.panels = validate(&req)?;
    d.name = req.name.trim().to_string();
    d.description = req.description.trim().to_string();
    d.time_range = req.time_range;
    d.refresh_secs = req.refresh_secs;
    d.updated_at = chrono::Utc::now().timestamp();
    store.put(KIND, &d.id, &d).await?;
    Ok(Json(d))
}

async fn remove(Extension(store): Store, Path(id): Path<String>) -> Result<StatusCode, ApiError> {
    if store.delete(KIND, &id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound(format!("Dashboard {} not found", id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(panels: Vec<DashboardPanel>) -> DashboardRequest {
        DashboardRequest {
            name: "Ops".into(),
            description: String::new(),
            time_range: "1h".into(),
            refresh_secs: 30,
            panels,
        }
    }

    fn panel(query: &str, chart: &str) -> DashboardPanel {
        DashboardPanel {
            id: String::new(),
            title: "CPU".into(),
            query: query.into(),
            chart: chart.into(),
            width: 1,
        }
    }

    #[test]
    fn panels_are_validated_against_the_metrics_catalog() {
        let ok = validate(&req(vec![panel("horcrux_node_cpu_usage_percent", "line")])).unwrap();
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].id.len(), 8, "panel ids are assigned");
        assert!(validate(&req(vec![panel("no_such_metric", "line")])).is_err());
        assert!(validate(&req(vec![panel("horcrux_node_load1", "pie")])).is_err());
        let mut r = req(vec![]);
        r.time_range = "2y".into();
        assert!(validate(&r).is_err());
        let mut r = req(vec![]);
        r.name = "  ".into();
        assert!(validate(&r).is_err());
    }
}
