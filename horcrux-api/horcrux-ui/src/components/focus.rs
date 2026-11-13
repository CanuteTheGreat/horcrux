//! Empty-state actions that point at a form already on the page.
use leptos::*;
use wasm_bindgen::JsCast;

/// Scroll to and focus the element `id` (a form field above the empty table).
pub fn focus_id(id: &str) {
    if let Some(el) = document().get_element_by_id(id) {
        el.scroll_into_view();
        if let Ok(h) = el.dyn_into::<web_sys::HtmlElement>() {
            let _ = h.focus();
        }
    }
}
