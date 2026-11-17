//! A resource slider whose stops are the supported configurations, with an
//! optional "Custom" override (validated number input; warns on overcommit).
use leptos::*;

/// Logical CPUs: 1..=host (capped at 256 stops).
pub fn cpu_steps(host_cpus: u32) -> Vec<u64> {
    (1..=host_cpus.clamp(1, 256) as u64).collect()
}

/// RAM stops in MB: 512, 1 GB, 2, 4, 8 ... doubling up to `max_mb`.
pub fn memory_steps(max_mb: u64) -> Vec<u64> {
    let mut v = vec![];
    let mut m = 512;
    while m <= max_mb {
        v.push(m);
        m *= 2;
    }
    if v.is_empty() {
        v.push(512);
    }
    v
}

/// Disk stops in GB: 4, 8, 16, 32 ... doubling up to the free space.
pub fn disk_steps(available_gb: u64) -> Vec<u64> {
    let mut v = vec![];
    let mut g = 4;
    while g <= available_gb {
        v.push(g);
        g *= 2;
    }
    if v.is_empty() {
        v.push(4);
    }
    v
}

/// "512 MB", "4 GB", "1.5 GB".
pub fn human_mb(mb: u64) -> String {
    if mb >= 1024 {
        let g = mb as f64 / 1024.0;
        if mb.is_multiple_of(1024) {
            format!("{} GB", mb / 1024)
        } else {
            format!("{g:.1} GB")
        }
    } else {
        format!("{mb} MB")
    }
}

pub fn human_gb(gb: u64) -> String {
    if gb >= 1024 && gb.is_multiple_of(1024) {
        format!("{} TB", gb / 1024)
    } else {
        format!("{gb} GB")
    }
}

pub fn human_cpus(n: u64) -> String {
    format!("{n} vCPU{}", if n == 1 { "" } else { "s" })
}

/// Index of the stop closest to `value`.
pub fn nearest_step(steps: &[u64], value: u64) -> usize {
    steps
        .iter()
        .enumerate()
        .min_by_key(|(_, s)| s.abs_diff(value))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// `value` is the chosen amount in the unit of `steps`; `limit` is what the
/// host can supply (above it the Custom input warns about overcommit);
/// `hard_max` is the most the Custom input accepts.
#[component]
pub fn ResourceSlider(
    #[prop(into)] label: String,
    #[prop(into)] id: String,
    value: RwSignal<u64>,
    #[prop(into)] steps: Signal<Vec<u64>>,
    #[prop(into)] limit: Signal<u64>,
    #[prop(into)] hard_max: Signal<u64>,
    fmt: fn(u64) -> String,
    /// Unit shown beside the Custom input.
    #[prop(into)]
    unit: String,
    /// What the warning says is exceeded ("host CPUs", ...).
    #[prop(into)]
    limit_what: String,
) -> impl IntoView {
    let (custom, set_custom) = create_signal(false);
    let idx = move || steps.with(|s| nearest_step(s, value.get()));
    let over = move || value.get() > limit.get();
    let (id_slider, id_custom, id_input, id_value, id_warn) = (
        format!("{id}-slider"),
        format!("{id}-custom"),
        format!("{id}-input"),
        format!("{id}-value"),
        format!("{id}-warning"),
    );
    let limit_what = store_value(limit_what);
    view! {
        <div class="form-group resource-slider" id=id.clone()>
            <label for=id_slider.clone()>{label}": "<strong id=id_value>{move || fmt(value.get())}</strong></label>
            <div class="form-row">
                <input type="range" id=id_slider min="0"
                    max=move || steps.with(|s| s.len().saturating_sub(1)).to_string()
                    step="1"
                    prop:value=move || idx().to_string()
                    disabled=move || custom.get()
                    on:input=move |ev| {
                        let i: usize = event_target_value(&ev).parse().unwrap_or(0);
                        if let Some(v) = steps.with(|s| s.get(i).copied()) { value.set(v); }
                    }/>
                <label class="checkbox-label">
                    <input type="checkbox" id=id_custom prop:checked=custom
                        on:change=move |ev| set_custom.set(event_target_checked(&ev))/>
                    " Custom"
                </label>
            </div>
            <div class="slider-stops" style="display:flex;justify-content:space-between;font-size:.8em;opacity:.7">
                <span>{move || steps.with(|s| s.first().map(|v| fmt(*v)).unwrap_or_default())}</span>
                <span>{move || steps.with(|s| s.last().map(|v| fmt(*v)).unwrap_or_default())}</span>
            </div>
            <Show when=move || custom.get()>
                <div class="form-row">
                    <input type="number" id=id_input.clone() min="1" max=move || hard_max.get().to_string()
                        required prop:value=move || value.get().to_string()
                        on:input=move |ev| {
                            if let Ok(v) = event_target_value(&ev).parse::<u64>() { value.set(v); }
                        }/>
                    <span>{unit.clone()}</span>
                    <small>{move || format!("(1 to {})", hard_max.get())}</small>
                </div>
            </Show>
            <Show when=over>
                <p class="form-help warning" id=id_warn.clone()>
                    {move || format!("{} is more than this host has ({} {}): the VM may not start, or will contend with the host.",
                        fmt(value.get()), fmt(limit.get()), limit_what.get_value())}
                </p>
            </Show>
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stops_are_supported_configurations() {
        assert_eq!(cpu_steps(4), vec![1, 2, 3, 4]);
        assert_eq!(cpu_steps(0), vec![1]);
        assert_eq!(memory_steps(4096), vec![512, 1024, 2048, 4096]);
        assert_eq!(memory_steps(5000), vec![512, 1024, 2048, 4096]);
        assert_eq!(memory_steps(100), vec![512]);
        assert_eq!(disk_steps(300), vec![4, 8, 16, 32, 64, 128, 256]);
        assert_eq!(disk_steps(1), vec![4]);
    }

    #[test]
    fn values_are_shown_in_human_units() {
        assert_eq!(human_mb(512), "512 MB");
        assert_eq!(human_mb(4096), "4 GB");
        assert_eq!(human_mb(1536), "1.5 GB");
        assert_eq!(human_gb(256), "256 GB");
        assert_eq!(human_gb(2048), "2 TB");
        assert_eq!(human_cpus(1), "1 vCPU");
        assert_eq!(human_cpus(4), "4 vCPUs");
    }

    #[test]
    fn slider_snaps_to_the_nearest_stop() {
        let s = memory_steps(8192);
        assert_eq!(s[nearest_step(&s, 4096)], 4096);
        assert_eq!(s[nearest_step(&s, 3000)], 2048);
        assert_eq!(nearest_step(&[], 5), 0);
    }
}
