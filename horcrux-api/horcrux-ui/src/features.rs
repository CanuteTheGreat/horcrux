//! Build-time features of the server this UI talks to.
//!
//! Horcrux is built with Gentoo USE flags; a backend that was off at emerge
//! time is not compiled into horcrux-api and its routes answer 404. The UI
//! is one WASM bundle for every USE set, so it asks the server which
//! features it has (`GET /api/features`, public) and hides the menu
//! entries, pages and form options of everything else.
//!
//! Feature expressions are a feature name or alternatives separated by `|`
//! (`"docker|podman"` = either OCI runtime).

use leptos::*;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

#[derive(Debug, Clone, Deserialize)]
pub struct FeaturesResponse {
    #[serde(default)]
    pub version: String,
    pub features: Vec<String>,
    #[serde(default)]
    pub all: Vec<String>,
}

/// What the server was built with. `enabled == None` means the server
/// could not tell us (an older horcrux-api without `/api/features`, or the
/// request failed): then nothing is hidden, so the UI never loses pages
/// because of a transient error.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FeatureSet {
    pub enabled: Option<HashSet<String>>,
}

impl FeatureSet {
    pub fn from_names<I: IntoIterator<Item = S>, S: Into<String>>(names: I) -> Self {
        Self {
            enabled: Some(names.into_iter().map(Into::into).collect()),
        }
    }

    /// `expr` is `"name"` or `"a|b|c"` (true if any is enabled).
    pub fn has(&self, expr: &str) -> bool {
        match &self.enabled {
            None => true,
            Some(set) => expr.split('|').map(str::trim).any(|f| set.contains(f)),
        }
    }
}

thread_local! {
    static CACHE: RefCell<Option<Rc<FeatureSet>>> = const { RefCell::new(None) };
}

/// The server's feature set, fetched once per page load.
pub async fn load() -> FeatureSet {
    if let Some(f) = CACHE.with(|c| c.borrow().clone()) {
        return (*f).clone();
    }
    let set = match crate::api::fetch_json::<FeaturesResponse>("/features").await {
        Ok(r) => FeatureSet::from_names(r.features),
        Err(_) => FeatureSet::default(),
    };
    CACHE.with(|c| *c.borrow_mut() = Some(Rc::new(set.clone())));
    set
}

/// Reactive handle stored in context by [`provide_features`].
#[derive(Clone, Copy)]
pub struct Features(pub RwSignal<Option<FeatureSet>>);

impl Features {
    /// True once loaded and `expr` is enabled. False while loading, so
    /// disabled entries never flash up.
    pub fn has(&self, expr: &str) -> bool {
        self.0.with(|f| f.as_ref().is_some_and(|f| f.has(expr)))
    }

    pub fn loaded(&self) -> bool {
        self.0.with(|f| f.is_some())
    }
}

/// Call once at the app root.
pub fn provide_features() -> Features {
    let sig = create_rw_signal(None::<FeatureSet>);
    spawn_local(async move { sig.set(Some(load().await)) });
    let f = Features(sig);
    provide_context(f);
    f
}

pub fn use_features() -> Features {
    use_context::<Features>().unwrap_or_else(provide_features)
}

/// Renders `children` only if the server has `feature`; otherwise a notice
/// that the feature is not compiled into this build (for routes reached by
/// URL even though the menu hides them).
#[component]
pub fn Gated(feature: &'static str, children: ChildrenFn) -> impl IntoView {
    let f = use_features();
    let children = store_value(children);
    move || {
        if !f.loaded() {
            view! { <p class="loading">"Loading..."</p> }.into_view()
        } else if f.has(feature) {
            children.with_value(|c| c()).into_view()
        } else {
            view! {
                <div class="feature-disabled" id="feature-disabled" data-feature=feature>
                    <h2>"Not available in this build"</h2>
                    <p>
                        "This Horcrux server was built without "
                        <code>{feature.replace('|', " / ")}</code>
                        ". Rebuild with the matching USE flag (emerge app-emulation/horcrux with USE=\""
                        {feature.split('|').next().unwrap_or(feature).to_string()}
                        "\") to enable it."
                    </p>
                </div>
            }
            .into_view()
        }
    }
}

/// Menu entry (or any markup) shown only when `feature` is enabled.
#[component]
pub fn IfFeature(feature: &'static str, children: ChildrenFn) -> impl IntoView {
    let f = use_features();
    view! { <Show when=move || f.has(feature)>{children()}</Show> }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_match_any_alternative() {
        let f = FeatureSet::from_names(["lxc", "podman"]);
        assert!(f.has("lxc"));
        assert!(f.has("docker|podman"));
        assert!(!f.has("docker"));
        assert!(!f.has("backup"));
    }

    #[test]
    fn unknown_feature_set_hides_nothing() {
        assert!(FeatureSet::default().has("backup"));
    }

    #[test]
    fn server_response_parses() {
        let r: FeaturesResponse = serde_json::from_str(
            r#"{"version":"0.1.5","features":["lxc","backup"],"all":["lxc","docker","backup"]}"#,
        )
        .unwrap();
        assert!(FeatureSet::from_names(r.features).has("backup"));
    }
}
