//! Wizard step header (numbered steps, done / current / upcoming) and a radio
//! "choice card" list, both self-contained so a shared component library can
//! absorb them later.
use leptos::*;

#[component]
pub fn StepHeader(
    /// Step titles, in order.
    steps: Vec<&'static str>,
    /// Zero-based current step.
    #[prop(into)]
    current: Signal<usize>,
    /// Jump back to an earlier (done) step.
    #[prop(optional, into)]
    on_select: Option<Callback<usize>>,
) -> impl IntoView {
    view! {
        <ol class="hx-steps">
            {steps.into_iter().enumerate().map(|(i, t)| {
                let cls = move || {
                    let c = current.get();
                    if i < c { "hx-step done" } else if i == c { "hx-step current" } else { "hx-step" }
                };
                let cur = move || (current.get() == i).then_some("step");
                view! {
                    <li class=cls aria-current=cur>
                        <button type="button" class="hx-step-btn" disabled=move || { i >= current.get() }
                            on:click=move |_| { if let Some(cb) = on_select { cb.call(i) } }>
                            <span class="hx-step-num">{(i + 1).to_string()}</span>
                            <span class="hx-step-title">{t}</span>
                        </button>
                    </li>
                }
            }).collect_view()}
        </ol>
    }
}

/// One option of a [`ChoiceCards`] list.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: String,
    pub title: String,
    /// Second line (version, what it does).
    pub detail: String,
    /// Small tags shown next to the title ("Recommended", "Testing").
    pub tags: Vec<(&'static str, String)>,
}

#[component]
pub fn ChoiceCards(
    /// Radio group name (also the id prefix: `<name>-<value>`).
    name: &'static str,
    choices: Vec<Choice>,
    selected: RwSignal<String>,
    /// Called after a change, with the new value.
    #[prop(optional, into)]
    on_change: Option<Callback<String>>,
) -> impl IntoView {
    view! {
        <div class="hx-choices" role="radiogroup">
            {choices.into_iter().map(|c| {
                let v = c.value.clone();
                let v2 = c.value.clone();
                let v3 = c.value.clone();
                let id = format!("{name}-{}", c.value);
                view! {
                    <label class=move || if selected.get() == v { "hx-choice selected" } else { "hx-choice" } for=id.clone()>
                        <input type="radio" name=name id=id value=c.value.clone()
                            prop:checked=move || selected.get() == v2
                            on:change=move |_| {
                                selected.set(v3.clone());
                                if let Some(cb) = on_change { cb.call(v3.clone()) }
                            }/>
                        <span class="hx-choice-text">
                            <span class="hx-choice-title">{c.title}
                                {c.tags.into_iter().map(|(cls, t)| view! { <span class=cls>{t}</span> }).collect_view()}
                            </span>
                            <span class="hx-choice-detail">{c.detail}</span>
                        </span>
                    </label>
                }
            }).collect_view()}
        </div>
    }
}
