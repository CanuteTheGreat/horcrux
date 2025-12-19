use leptos::*;
use crate::api::*;

#[component]
pub fn DiagnosticsPage() -> impl IntoView {
    let (diagnostic_data, set_diagnostic_data) = create_signal(None::<SystemDiagnostics>);
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);
    let (running_tests, set_running_tests) = create_signal(std::collections::HashSet::<String>::new());
    let (test_results, set_test_results) = create_signal(std::collections::HashMap::<String, DiagnosticTestResult>::new());
    let (selected_tests, set_selected_tests) = create_signal(std::collections::HashSet::<String>::new());

    // Load system diagnostics
    let load_diagnostics = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_system_diagnostics().await {
            Ok(diagnostics) => set_diagnostic_data.set(Some(diagnostics)),
            Err(e) => set_error_message.set(Some(format!("Failed to load diagnostics: {}", e))),
        }

        set_loading.set(false);
    });

    // Run individual diagnostic test
    let run_test_action = create_action(move |test_name: &String| {
        let test_name = test_name.clone();
        async move {
            set_running_tests.update(|tests| { tests.insert(test_name.clone()); });
            set_error_message.set(None);

            match run_diagnostic_test(&test_name).await {
                Ok(result) => {
                    set_test_results.update(|results| {
                        results.insert(test_name.clone(), result);
                    });
                    if test_results.get().get(&test_name).unwrap().passed {
                        set_success_message.set(Some(format!("Test '{}' passed", test_name)));
                    }
                }
                Err(e) => set_error_message.set(Some(format!("Test '{}' failed: {}", test_name, e))),
            }

            set_running_tests.update(|tests| { tests.remove(&test_name); });
        }
    });

    // Run all diagnostic tests
    let run_all_tests_action = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match run_all_diagnostic_tests().await {
            Ok(results) => {
                set_test_results.set(results.clone());
                let passed_count = results.values().filter(|r| r.passed).count();
                let total_count = results.len();
                set_success_message.set(Some(format!("Completed {} tests: {} passed, {} failed",
                    total_count, passed_count, total_count - passed_count)));
            }
            Err(e) => set_error_message.set(Some(format!("Failed to run diagnostic tests: {}", e))),
        }

        set_loading.set(false);
    });

    // Generate diagnostic report
    let generate_report_action = create_action(move |format: &String| {
        let format = format.clone();
        async move {
            set_loading.set(true);
            set_error_message.set(None);

            match generate_diagnostic_report(&format).await {
                Ok(report_data) => {
                    // Trigger download
                    use wasm_bindgen::prelude::*;
                    let window = web_sys::window().unwrap();
                    let document = window.document().unwrap();

                    let element = document.create_element("a").unwrap();
                    let element = element.dyn_into::<web_sys::HtmlAnchorElement>().unwrap();

                    let blob_parts = js_sys::Array::new();
                    blob_parts.push(&JsValue::from_str(&report_data));

                    let blob = web_sys::Blob::new_with_str_sequence(&blob_parts).unwrap();
                    let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();

                    element.set_href(&url);
                    element.set_download(&format!("diagnostics-report.{}",
                        match format.as_str() {
                            "html" => "html",
                            "pdf" => "pdf",
                            _ => "json",
                        }));
                    element.click();

                    web_sys::Url::revoke_object_url(&url).unwrap();
                    set_success_message.set(Some("Diagnostic report generated successfully".to_string()));
                }
                Err(e) => set_error_message.set(Some(format!("Failed to generate report: {}", e))),
            }

            set_loading.set(false);
        }
    });

    // Run selected tests
    let run_selected_tests_action = create_action(move |_: &()| async move {
        let tests_to_run: Vec<String> = selected_tests.get().into_iter().collect();
        if tests_to_run.is_empty() {
            return;
        }

        set_loading.set(true);
        set_error_message.set(None);

        for test_name in tests_to_run {
            set_running_tests.update(|tests| { tests.insert(test_name.clone()); });

            match run_diagnostic_test(&test_name).await {
                Ok(result) => {
                    set_test_results.update(|results| {
                        results.insert(test_name.clone(), result);
                    });
                }
                Err(e) => {
                    set_error_message.set(Some(format!("Test '{}' failed: {}", test_name, e)));
                    break;
                }
            }

            set_running_tests.update(|tests| { tests.remove(&test_name); });
        }

        let results_snapshot = test_results.get();
        let passed_count = selected_tests.get().iter()
            .filter_map(|name| results_snapshot.get(name))
            .filter(|result| result.passed)
            .count();
        let total_count = selected_tests.get().len();

        set_success_message.set(Some(format!("Completed {} selected tests: {} passed, {} failed",
            total_count, passed_count, total_count - passed_count)));

        set_selected_tests.set(std::collections::HashSet::new());
        set_loading.set(false);
    });

    // Helper functions
    let get_severity_color = |severity: &str| match severity {
        "critical" => "text-red-600 bg-red-50",
        "high" => "text-orange-600 bg-orange-50",
        "medium" => "text-yellow-600 bg-yellow-50",
        "low" => "text-blue-600 bg-blue-50",
        "info" => "text-green-600 bg-green-50",
        _ => "text-gray-600 bg-gray-50",
    };

    let get_status_color = |passed: bool| {
        if passed { "text-green-600 bg-green-50" } else { "text-red-600 bg-red-50" }
    };

    let format_duration = |duration_ms: u64| -> String {
        if duration_ms >= 60000 {
            format!("{:.1}m", duration_ms as f64 / 60000.0)
        } else if duration_ms >= 1000 {
            format!("{:.1}s", duration_ms as f64 / 1000.0)
        } else {
            format!("{}ms", duration_ms)
        }
    };

    let toggle_test_selection = move |test_name: String| {
        set_selected_tests.update(|selected| {
            if selected.contains(&test_name) {
                selected.remove(&test_name);
            } else {
                selected.insert(test_name);
            }
        });
    };

    // Clear messages after delay
    let clear_messages = move || {
        set_timeout(
            move || {
                set_success_message.set(None);
                set_error_message.set(None);
            },
            std::time::Duration::from_secs(5),
        );
    };

    // Initial load
    create_effect(move |_| {
        load_diagnostics.dispatch(());
    });

    view! {
        <div class="diagnostics-page">
            <div class="page-header">
                <h1>"System Diagnostics"</h1>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_diagnostics.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <button
                        class="btn btn-warning"
                        on:click=move |_| run_all_tests_action.dispatch(())
                        disabled=loading
                    >
                        "Run All Tests"
                    </button>
                    <div class="dropdown">
                        <button class="btn btn-primary dropdown-toggle">"Generate Report"</button>
                        <div class="dropdown-menu">
                            <button
                                class="dropdown-item"
                                on:click=move |_| generate_report_action.dispatch("json".to_string())
                                disabled=loading
                            >
                                "JSON Report"
                            </button>
                            <button
                                class="dropdown-item"
                                on:click=move |_| generate_report_action.dispatch("html".to_string())
                                disabled=loading
                            >
                                "HTML Report"
                            </button>
                            <button
                                class="dropdown-item"
                                on:click=move |_| generate_report_action.dispatch("pdf".to_string())
                                disabled=loading
                            >
                                "PDF Report"
                            </button>
                        </div>
                    </div>
                </div>
            </div>

            {move || error_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-error">{msg}</div>
                }
            })}

            {move || success_message.get().map(|msg| {
                clear_messages();
                view! {
                    <div class="alert alert-success">{msg}</div>
                }
            })}

            {move || if loading.get() && diagnostic_data.get().is_none() {
                view! { <div class="loading">"Loading system diagnostics..."</div> }.into_view()
            } else if let Some(diagnostics) = diagnostic_data.get() {
                view! {
                    <div class="diagnostics-dashboard">
                        // System Overview Cards
                        <div class="diagnostic-cards">
                            <div class="diagnostic-card">
                                <div class="card-icon system-icon">"🔍"</div>
                                <div class="card-content">
                                    <h3>"System Health Summary"</h3>
                                    <div class="health-overview">
                                        <div class="health-metric">
                                            <span class="metric-label">"Overall Status:"</span>
                                            <span class={format!("metric-value {}",
                                                if diagnostics.overall_health_score > 80.0 { "text-green-600" }
                                                else if diagnostics.overall_health_score > 60.0 { "text-yellow-600" }
                                                else { "text-red-600" }
                                            )}>
                                                {format!("{:.0}%", diagnostics.overall_health_score)}
                                            </span>
                                        </div>
                                        <div class="health-metric">
                                            <span class="metric-label">"Critical Issues:"</span>
                                            <span class="metric-value text-red-600">
                                                {diagnostics.critical_issues_count}
                                            </span>
                                        </div>
                                        <div class="health-metric">
                                            <span class="metric-label">"Warnings:"</span>
                                            <span class="metric-value text-yellow-600">
                                                {diagnostics.warning_count}
                                            </span>
                                        </div>
                                        <div class="health-metric">
                                            <span class="metric-label">"Last Check:"</span>
                                            <span class="metric-value text-gray-600">
                                                {&diagnostics.last_check_time}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="diagnostic-card">
                                <div class="card-icon performance-icon">"⚡"</div>
                                <div class="card-content">
                                    <h3>"Performance Metrics"</h3>
                                    <div class="performance-overview">
                                        <div class="perf-metric">
                                            <span class="metric-label">"CPU Performance:"</span>
                                            <span class="metric-value">
                                                {format!("{:.1}%", diagnostics.performance_metrics.cpu_score)}
                                            </span>
                                        </div>
                                        <div class="perf-metric">
                                            <span class="metric-label">"Memory Efficiency:"</span>
                                            <span class="metric-value">
                                                {format!("{:.1}%", diagnostics.performance_metrics.memory_score)}
                                            </span>
                                        </div>
                                        <div class="perf-metric">
                                            <span class="metric-label">"Disk I/O:"</span>
                                            <span class="metric-value">
                                                {format!("{:.1}%", diagnostics.performance_metrics.disk_score)}
                                            </span>
                                        </div>
                                        <div class="perf-metric">
                                            <span class="metric-label">"Network:"</span>
                                            <span class="metric-value">
                                                {format!("{:.1}%", diagnostics.performance_metrics.network_score)}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="diagnostic-card">
                                <div class="card-icon security-icon">"🛡️"</div>
                                <div class="card-content">
                                    <h3>"Security Status"</h3>
                                    <div class="security-overview">
                                        <div class="security-metric">
                                            <span class="metric-label">"Security Score:"</span>
                                            <span class={format!("metric-value {}",
                                                if diagnostics.security_status.score > 80.0 { "text-green-600" }
                                                else if diagnostics.security_status.score > 60.0 { "text-yellow-600" }
                                                else { "text-red-600" }
                                            )}>
                                                {format!("{:.0}%", diagnostics.security_status.score)}
                                            </span>
                                        </div>
                                        <div class="security-metric">
                                            <span class="metric-label">"Vulnerabilities:"</span>
                                            <span class="metric-value text-red-600">
                                                {diagnostics.security_status.vulnerability_count}
                                            </span>
                                        </div>
                                        <div class="security-metric">
                                            <span class="metric-label">"Compliance:"</span>
                                            <span class={format!("metric-value {}",
                                                if diagnostics.security_status.compliance_level > 80.0 { "text-green-600" }
                                                else { "text-yellow-600" }
                                            )}>
                                                {format!("{:.0}%", diagnostics.security_status.compliance_level)}
                                            </span>
                                        </div>
                                    </div>
                                </div>
                            </div>
                        </div>

                        // Diagnostic Tests Section
                        <div class="diagnostic-tests-section">
                            <div class="section-header">
                                <h2>"Diagnostic Tests"</h2>
                                {move || if !selected_tests.get().is_empty() {
                                    view! {
                                        <div class="selected-actions">
                                            <span class="selected-count">
                                                {selected_tests.get().len()}" tests selected"
                                            </span>
                                            <button
                                                class="btn btn-xs btn-primary"
                                                on:click=move |_| run_selected_tests_action.dispatch(())
                                                disabled=loading
                                            >
                                                "Run Selected"
                                            </button>
                                            <button
                                                class="btn btn-xs btn-secondary"
                                                on:click=move |_| set_selected_tests.set(std::collections::HashSet::new())
                                            >
                                                "Clear Selection"
                                            </button>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}
                            </div>

                            <div class="tests-table-container">
                                <table class="tests-table">
                                    <thead>
                                        <tr>
                                            <th class="select-col">
                                                {
                                                    let tests_for_select_all = diagnostics.available_tests.clone();
                                                    view! {
                                                        <input
                                                            type="checkbox"
                                                            on:change=move |ev| {
                                                                if event_target_checked(&ev) {
                                                                    let all_test_names: std::collections::HashSet<String> =
                                                                        tests_for_select_all.iter().map(|t| t.name.clone()).collect();
                                                                    set_selected_tests.set(all_test_names);
                                                                } else {
                                                                    set_selected_tests.set(std::collections::HashSet::new());
                                                                }
                                                            }
                                                        />
                                                    }
                                                }
                                            </th>
                                            <th>"Test Name"</th>
                                            <th>"Category"</th>
                                            <th>"Description"</th>
                                            <th>"Status"</th>
                                            <th>"Duration"</th>
                                            <th>"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        {diagnostics.available_tests.clone().into_iter().map(|test| {
                                            let test_clone1 = test.clone();
                                            let test_clone2 = test.clone();
                                            let test_name = test.name.clone();
                                            let test_name2 = test.name.clone();
                                            let test_name3 = test.name.clone();
                                            let test_name4 = test.name.clone();
                                            let test_name_display = test.name.clone();
                                            let test_category = test.category.clone();
                                            let test_description = test.description.clone();

                                            view! {
                                                <tr class="test-row">
                                                    <td class="test-select">
                                                        <input
                                                            type="checkbox"
                                                            prop:checked=move || selected_tests.get().contains(&test_name)
                                                            on:change=move |_| toggle_test_selection(test_clone2.name.clone())
                                                        />
                                                    </td>
                                                    <td class="test-name">
                                                        <strong>{test_name_display}</strong>
                                                    </td>
                                                    <td class="test-category">
                                                        <span class="category-badge">{test_category}</span>
                                                    </td>
                                                    <td class="test-description">
                                                        {test_description}
                                                    </td>
                                                    <td class="test-status">
                                                        {move || {
                                                            if running_tests.get().contains(&test_name2) {
                                                                view! {
                                                                    <span class="status-badge text-blue-600 bg-blue-50">
                                                                        "🔄 Running"
                                                                    </span>
                                                                }.into_view()
                                                            } else if let Some(result) = test_results.get().get(&test_name2) {
                                                                let status_class = get_status_color(result.passed);
                                                                view! {
                                                                    <span class={format!("status-badge {}", status_class)}>
                                                                        {if result.passed { "✅ Passed" } else { "❌ Failed" }}
                                                                    </span>
                                                                }.into_view()
                                                            } else {
                                                                view! {
                                                                    <span class="status-badge text-gray-600 bg-gray-50">
                                                                        "⏸️ Not Run"
                                                                    </span>
                                                                }.into_view()
                                                            }
                                                        }}
                                                    </td>
                                                    <td class="test-duration">
                                                        {move || {
                                                            if let Some(result) = test_results.get().get(&test_name3) {
                                                                format_duration(result.duration_ms)
                                                            } else {
                                                                "-".to_string()
                                                            }
                                                        }}
                                                    </td>
                                                    <td class="test-actions">
                                                        <button
                                                            class="btn btn-xs btn-primary"
                                                            disabled=move || running_tests.get().contains(&test_name4) || loading.get()
                                                            on:click=move |_| run_test_action.dispatch(test_clone1.name.clone())
                                                        >
                                                            "Run Test"
                                                        </button>
                                                    </td>
                                                </tr>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </tbody>
                                </table>
                            </div>
                        </div>

                        // Test Results Details
                        {move || if !test_results.get().is_empty() {
                            view! {
                                <div class="test-results-section">
                                    <h2>"Test Results"</h2>
                                    <div class="results-grid">
                                        {test_results.get().into_iter().collect::<Vec<_>>().into_iter().map(|(test_name, result)| {
                                            let status_class = get_status_color(result.passed);
                                            let passed = result.passed;
                                            let duration_ms = result.duration_ms;
                                            let message = result.message.clone();
                                            let error_details = result.error_details.clone();

                                            view! {
                                                <div class="result-card">
                                                    <div class="result-header">
                                                        <h4>{test_name}</h4>
                                                        <span class={format!("result-status {}", status_class)}>
                                                            {if passed { "✅ PASSED" } else { "❌ FAILED" }}
                                                        </span>
                                                    </div>
                                                    <div class="result-details">
                                                        <div class="detail-row">
                                                            <span class="label">"Duration:"</span>
                                                            <span class="value">{format_duration(duration_ms)}</span>
                                                        </div>
                                                        <div class="detail-row">
                                                            <span class="label">"Message:"</span>
                                                            <span class="value">{message}</span>
                                                        </div>
                                                        {error_details.map(|details| view! {
                                                            <div class="detail-row">
                                                                <span class="label">"Details:"</span>
                                                                <span class="value error-details">{details}</span>
                                                            </div>
                                                        })}
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            }.into_view()
                        } else {
                            view! { <div></div> }.into_view()
                        }}

                        // System Issues Section
                        {move || if !diagnostics.system_issues.is_empty() {
                            view! {
                                <div class="system-issues-section">
                                    <h2>"System Issues"</h2>
                                    <div class="issues-list">
                                        {diagnostics.system_issues.clone().into_iter().map(|issue| {
                                            let severity_class = get_severity_color(&issue.severity);
                                            let severity = issue.severity.to_uppercase();
                                            let category = issue.category.clone();
                                            let description = issue.description.clone();
                                            let recommendation = issue.recommendation.clone();

                                            view! {
                                                <div class="issue-card">
                                                    <div class="issue-header">
                                                        <span class={format!("severity-badge {}", severity_class)}>
                                                            {severity}
                                                        </span>
                                                        <span class="issue-category">{category}</span>
                                                    </div>
                                                    <div class="issue-content">
                                                        <p class="issue-description">{description}</p>
                                                        {recommendation.map(|rec| view! {
                                                            <div class="issue-recommendation">
                                                                <strong>"Recommendation: "</strong>
                                                                {rec}
                                                            </div>
                                                        })}
                                                    </div>
                                                </div>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="no-issues">
                                    <div class="success-message">
                                        "🎉 No system issues detected. Your system is running optimally!"
                                    </div>
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view()
            } else {
                view! { <div class="empty-state">"No diagnostic data available"</div> }.into_view()
            }}
        </div>
    }
}