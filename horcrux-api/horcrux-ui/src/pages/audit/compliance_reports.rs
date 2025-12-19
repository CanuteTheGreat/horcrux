use leptos::*;
use crate::api::*;

// All types imported from crate::api::* (ComplianceFramework, ComplianceControl, ComplianceEvidence, ManualOverride, ComplianceReport, GenerateReportRequest)

#[component]
pub fn ComplianceReportsPage() -> impl IntoView {
    let (frameworks, set_frameworks) = create_signal(Vec::<ComplianceFramework>::new());
    let (controls, set_controls) = create_signal(Vec::<ComplianceControl>::new());
    let (reports, set_reports) = create_signal(Vec::<ComplianceReport>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // View states
    let (current_tab, set_current_tab) = create_signal("overview".to_string());
    let (selected_framework, set_selected_framework) = create_signal(None::<String>);
    let (filter_status, set_filter_status) = create_signal("all".to_string());
    let (filter_category, set_filter_category) = create_signal("all".to_string());

    // Modal states
    let (show_control_detail, set_show_control_detail) = create_signal(false);
    let (selected_control, set_selected_control) = create_signal(None::<ComplianceControl>);
    let (show_generate_report, set_show_generate_report) = create_signal(false);
    let (show_override_modal, set_show_override_modal) = create_signal(false);

    // Report generation states
    let (report_framework, set_report_framework) = create_signal(String::new());
    let (report_type, set_report_type) = create_signal("full".to_string());
    let (report_period_start, set_report_period_start) = create_signal(String::new());
    let (report_period_end, set_report_period_end) = create_signal(String::new());
    let (report_include_evidence, set_report_include_evidence) = create_signal(true);
    let (report_format, set_report_format) = create_signal("pdf".to_string());
    let (generating_report, set_generating_report) = create_signal(false);

    // Override states
    let (override_status, set_override_status) = create_signal("pass".to_string());
    let (override_reason, set_override_reason) = create_signal(String::new());
    let (override_expires, set_override_expires) = create_signal(String::new());

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            let frameworks_result = get_compliance_frameworks().await;
            let reports_result = get_compliance_reports().await;

            match (frameworks_result, reports_result) {
                (Ok(fw), Ok(rpts)) => {
                    set_frameworks.set(fw);
                    set_reports.set(rpts);
                    set_error.set(None);
                }
                (Err(e), _) | (_, Err(e)) => {
                    set_error.set(Some(format!("Failed to load compliance data: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Load controls when framework is selected
    create_effect(move |_| {
        if let Some(framework_id) = selected_framework.get() {
            spawn_local(async move {
                if let Ok(ctrl) = get_compliance_controls(framework_id).await {
                    set_controls.set(ctrl);
                }
            });
        }
    });

    // Filtered controls
    let filtered_controls = create_memo(move |_| {
        controls.get()
            .into_iter()
            .filter(|control| {
                let status_match = filter_status.get() == "all" || control.status == filter_status.get();
                let category_match = filter_category.get() == "all" || control.category == filter_category.get();
                status_match && category_match
            })
            .collect::<Vec<_>>()
    });

    // Get unique categories
    let categories = create_memo(move |_| {
        let mut cats: Vec<String> = controls.get()
            .iter()
            .map(|c| c.category.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        cats.sort();
        cats
    });

    // Generate compliance report
    let generate_report = move || {
        set_generating_report.set(true);

        let request = GenerateReportRequest {
            framework_id: report_framework.get(),
            report_type: report_type.get(),
            period_start: report_period_start.get(),
            period_end: report_period_end.get(),
            include_evidence: report_include_evidence.get(),
            format: report_format.get(),
        };

        spawn_local(async move {
            match generate_compliance_report(request).await {
                Ok(report) => {
                    // Reload reports
                    if let Ok(rpts) = get_compliance_reports().await {
                        set_reports.set(rpts);
                    }
                    set_show_generate_report.set(false);

                    // Optionally trigger download
                    if let Some(url) = report.download_url {
                        let _ = web_sys::window().unwrap().location().set_href(&url);
                    }
                }
                Err(_) => {
                    // Show error
                }
            }
            set_generating_report.set(false);
        });
    };

    // Run compliance assessment
    let run_assessment = move |framework_id: String| {
        spawn_local(async move {
            if let Ok(_) = run_compliance_assessment(framework_id).await {
                // Reload frameworks
                if let Ok(fw) = get_compliance_frameworks().await {
                    set_frameworks.set(fw);
                }
            }
        });
    };

    // Apply manual override
    let apply_override = move || {
        if let Some(control) = selected_control.get() {
            let override_data = ManualOverride {
                status: override_status.get(),
                reason: override_reason.get(),
                user: "current_user".to_string(), // Would come from auth context
                timestamp: chrono::Utc::now().to_rfc3339(),
                expires: if override_expires.get().is_empty() { None } else { Some(override_expires.get()) },
            };

            spawn_local(async move {
                if let Ok(_) = set_control_override(control.id.clone(), override_data).await {
                    set_show_override_modal.set(false);
                    set_override_reason.set(String::new());
                    set_override_expires.set(String::new());

                    // Reload controls
                    if let Some(framework_id) = selected_framework.get() {
                        if let Ok(ctrl) = get_compliance_controls(framework_id).await {
                            set_controls.set(ctrl);
                        }
                    }
                }
            });
        }
    };

    view! {
        <div class="compliance-reports-page">
            <div class="page-header">
                <h1 class="page-title">Compliance Reports</h1>
                <p class="page-description">
                    Monitor compliance with regulatory frameworks and generate audit-ready reports
                </p>

                <div class="page-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_generate_report.set(true)
                    >
                        Generate Report
                    </button>
                </div>
            </div>

            <div class="page-tabs">
                <button
                    class={move || if current_tab.get() == "overview" { "tab-btn active" } else { "tab-btn" }}
                    on:click=move |_| set_current_tab.set("overview".to_string())
                >
                    Overview
                </button>
                <button
                    class={move || if current_tab.get() == "controls" { "tab-btn active" } else { "tab-btn" }}
                    on:click=move |_| set_current_tab.set("controls".to_string())
                >
                    Controls
                </button>
                <button
                    class={move || if current_tab.get() == "reports" { "tab-btn active" } else { "tab-btn" }}
                    on:click=move |_| set_current_tab.set("reports".to_string())
                >
                    Reports
                </button>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading compliance data...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading Compliance Data</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                match current_tab.get().as_str() {
                    "overview" => view! {
                        <div class="compliance-overview">
                            <div class="frameworks-grid">
                                {move || frameworks.get().into_iter().map(|framework| {
                                    let framework_id = framework.id.clone();
                                    let framework_id2 = framework.id.clone();
                                    view! {
                                        <div class={format!("framework-card {}",
                                            if framework.score >= 90.0 { "compliant" }
                                            else if framework.score >= 70.0 { "partial" }
                                            else { "non-compliant" }
                                        )}>
                                            <div class="framework-header">
                                                <h3>{framework.name.clone()}</h3>
                                                <span class="version-badge">v{framework.version.clone()}</span>
                                            </div>

                                            <p class="framework-description">{framework.description.clone()}</p>

                                            <div class="compliance-score">
                                                <div class="score-circle">
                                                    <span class="score-value">{format!("{:.0}", framework.score)}%</span>
                                                </div>
                                            </div>

                                            <div class="control-stats">
                                                <div class="stat-item pass">
                                                    <span class="stat-value">{framework.passing_controls.to_string()}</span>
                                                    <span class="stat-label">Passing</span>
                                                </div>
                                                <div class="stat-item fail">
                                                    <span class="stat-value">{framework.failing_controls.to_string()}</span>
                                                    <span class="stat-label">Failing</span>
                                                </div>
                                                <div class="stat-item na">
                                                    <span class="stat-value">{framework.not_applicable_controls.to_string()}</span>
                                                    <span class="stat-label">N/A</span>
                                                </div>
                                            </div>

                                            <div class="framework-meta">
                                                <span>Last Assessment: {framework.last_assessment.clone()}</span>
                                            </div>

                                            <div class="framework-actions">
                                                <button
                                                    class="btn btn-secondary"
                                                    on:click=move |_| {
                                                        set_selected_framework.set(Some(framework_id.clone()));
                                                        set_current_tab.set("controls".to_string());
                                                    }
                                                >
                                                    View Controls
                                                </button>
                                                <button
                                                    class="btn btn-primary"
                                                    on:click=move |_| {
                                                        run_assessment(framework_id2.clone());
                                                    }
                                                >
                                                    Run Assessment
                                                </button>
                                            </div>
                                        </div>
                                    }
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    }.into_view(),

                    "controls" => view! {
                        <div class="controls-view">
                            // Framework selector
                            <div class="controls-filters">
                                <div class="filter-row">
                                    <select
                                        class="filter-select framework-select"
                                        prop:value=selected_framework.get().unwrap_or_default()
                                        on:change=move |ev| {
                                            let value = event_target_value(&ev);
                                            if value.is_empty() {
                                                set_selected_framework.set(None);
                                            } else {
                                                set_selected_framework.set(Some(value));
                                            }
                                        }
                                    >
                                        <option value="">Select Framework</option>
                                        {move || frameworks.get().into_iter().map(|fw| view! {
                                            <option value={fw.id.clone()}>{fw.name}</option>
                                        }).collect::<Vec<_>>()}
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_status
                                        on:change=move |ev| {
                                            set_filter_status.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Status</option>
                                        <option value="pass">Passing</option>
                                        <option value="fail">Failing</option>
                                        <option value="warning">Warning</option>
                                        <option value="not_applicable">Not Applicable</option>
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_category
                                        on:change=move |ev| {
                                            set_filter_category.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Categories</option>
                                        {move || categories.get().into_iter().map(|cat| view! {
                                            <option value={cat.clone()}>{cat}</option>
                                        }).collect::<Vec<_>>()}
                                    </select>
                                </div>
                            </div>

                            {move || if selected_framework.get().is_none() {
                                view! {
                                    <div class="empty-state">
                                        <h3>Select a Framework</h3>
                                        <p>Choose a compliance framework to view its controls</p>
                                    </div>
                                }.into_view()
                            } else if filtered_controls.get().is_empty() {
                                view! {
                                    <div class="empty-state">
                                        <h3>No Controls Found</h3>
                                        <p>No controls match your current filters</p>
                                    </div>
                                }.into_view()
                            } else {
                                view! {
                                    <div class="controls-list">
                                        <table class="controls-table">
                                            <thead>
                                                <tr>
                                                    <th>Control ID</th>
                                                    <th>Title</th>
                                                    <th>Category</th>
                                                    <th>Severity</th>
                                                    <th>Status</th>
                                                    <th>Last Checked</th>
                                                    <th>Actions</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {move || filtered_controls.get().into_iter().map(|control| {
                                                    let control_clone = control.clone();
                                                    let control_clone2 = control.clone();
                                                    view! {
                                                        <tr class={format!("control-row status-{}", control.status)}>
                                                            <td class="control-id">{control.control_id.clone()}</td>
                                                            <td class="control-title">{control.title.clone()}</td>
                                                            <td class="control-category">{control.category.clone()}</td>
                                                            <td class="control-severity">
                                                                <span class={format!("severity-badge severity-{}", control.severity)}>
                                                                    {control.severity.to_uppercase()}
                                                                </span>
                                                            </td>
                                                            <td class="control-status">
                                                                <span class={format!("status-badge status-{}", control.status)}>
                                                                    {match control.status.as_str() {
                                                                        "pass" => "PASS".to_string(),
                                                                        "fail" => "FAIL".to_string(),
                                                                        "warning" => "WARNING".to_string(),
                                                                        "not_applicable" => "N/A".to_string(),
                                                                        _ => control.status.clone()
                                                                    }}
                                                                </span>
                                                                {control.manual_override.as_ref().map(|_| view! {
                                                                    <span class="override-indicator" title="Manual override applied">
                                                                        (Override)
                                                                    </span>
                                                                })}
                                                            </td>
                                                            <td class="control-last-checked">{control.last_checked.clone()}</td>
                                                            <td class="control-actions">
                                                                <button
                                                                    class="btn btn-sm btn-secondary"
                                                                    on:click=move |_| {
                                                                        set_selected_control.set(Some(control_clone.clone()));
                                                                        set_show_control_detail.set(true);
                                                                    }
                                                                >
                                                                    Details
                                                                </button>
                                                                <button
                                                                    class="btn btn-sm btn-outline"
                                                                    on:click=move |_| {
                                                                        set_selected_control.set(Some(control_clone2.clone()));
                                                                        set_show_override_modal.set(true);
                                                                    }
                                                                >
                                                                    Override
                                                                </button>
                                                            </td>
                                                        </tr>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </tbody>
                                        </table>
                                    </div>
                                }.into_view()
                            }}
                        </div>
                    }.into_view(),

                    "reports" => view! {
                        <div class="reports-view">
                            <div class="reports-list">
                                {move || if reports.get().is_empty() {
                                    view! {
                                        <div class="empty-state">
                                            <h3>No Reports Generated</h3>
                                            <p>Generate your first compliance report to get started</p>
                                            <button
                                                class="btn btn-primary"
                                                on:click=move |_| set_show_generate_report.set(true)
                                            >
                                                Generate Report
                                            </button>
                                        </div>
                                    }.into_view()
                                } else {
                                    view! {
                                        <table class="reports-table">
                                            <thead>
                                                <tr>
                                                    <th>Framework</th>
                                                    <th>Report Type</th>
                                                    <th>Period</th>
                                                    <th>Score</th>
                                                    <th>Status</th>
                                                    <th>Generated</th>
                                                    <th>Actions</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                {reports.get().into_iter().map(|report| {
                                                    view! {
                                                        <tr>
                                                            <td>{report.framework_name.clone()}</td>
                                                            <td>{report.report_type.clone()}</td>
                                                            <td>
                                                                {format!("{} - {}", report.period_start, report.period_end)}
                                                            </td>
                                                            <td>
                                                                <span class={format!("score-badge {}",
                                                                    if report.overall_score >= 90.0 { "compliant" }
                                                                    else if report.overall_score >= 70.0 { "partial" }
                                                                    else { "non-compliant" }
                                                                )}>
                                                                    {format!("{:.0}%", report.overall_score)}
                                                                </span>
                                                            </td>
                                                            <td>
                                                                <span class={format!("status-badge status-{}", report.status)}>
                                                                    {report.status.replace("_", " ").to_uppercase()}
                                                                </span>
                                                            </td>
                                                            <td>
                                                                <div>{report.generated_at.clone()}</div>
                                                                <small>by {report.generated_by.clone()}</small>
                                                            </td>
                                                            <td>
                                                                {report.download_url.as_ref().map(|url| view! {
                                                                    <a
                                                                        href={url.clone()}
                                                                        class="btn btn-sm btn-primary"
                                                                        target="_blank"
                                                                    >
                                                                        Download
                                                                    </a>
                                                                })}
                                                            </td>
                                                        </tr>
                                                    }
                                                }).collect::<Vec<_>>()}
                                            </tbody>
                                        </table>
                                    }.into_view()
                                }}
                            </div>
                        </div>
                    }.into_view(),

                    _ => view! { <div></div> }.into_view()
                }
            }}

            // Control Detail Modal
            {move || if show_control_detail.get() {
                selected_control.get().map(|control| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_control_detail.set(false)>
                        <div class="modal-content control-detail-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Control Details: {control.control_id.clone()}</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_control_detail.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="control-full-details">
                                    <div class="detail-section">
                                        <h3>{control.title.clone()}</h3>
                                        <p>{control.description.clone()}</p>
                                    </div>

                                    <div class="detail-section">
                                        <h4>Status</h4>
                                        <div class="status-display">
                                            <span class={format!("status-badge status-{}", control.status)}>
                                                {control.status.to_uppercase()}
                                            </span>
                                            {control.manual_override.as_ref().map(|override_info| view! {
                                                <div class="override-info">
                                                    <span class="override-label">Manual Override:</span>
                                                    <span>{override_info.reason.clone()}</span>
                                                    <small>by {override_info.user.clone()} on {override_info.timestamp.clone()}</small>
                                                </div>
                                            })}
                                        </div>
                                    </div>

                                    {if !control.recommendations.is_empty() {
                                        view! {
                                            <div class="detail-section">
                                                <h4>Recommendations</h4>
                                                <ul class="recommendations-list">
                                                    {control.recommendations.iter().map(|rec| view! {
                                                        <li>{rec.clone()}</li>
                                                    }).collect::<Vec<_>>()}
                                                </ul>
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! { <div></div> }.into_view()
                                    }}

                                    {if !control.evidence.is_empty() {
                                        view! {
                                            <div class="detail-section">
                                                <h4>Evidence ({control.evidence.len()} items)</h4>
                                                <div class="evidence-list">
                                                    {control.evidence.iter().map(|evidence| view! {
                                                        <div class="evidence-item">
                                                            <div class="evidence-header">
                                                                <span class="evidence-type">{evidence.evidence_type.clone()}</span>
                                                                <span class="evidence-source">{evidence.source.clone()}</span>
                                                                <span class={format!("verified-badge {}", if evidence.verified { "verified" } else { "unverified" })}>
                                                                    {if evidence.verified { "Verified" } else { "Unverified" }}
                                                                </span>
                                                            </div>
                                                            <div class="evidence-timestamp">{evidence.timestamp.clone()}</div>
                                                        </div>
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            </div>
                                        }.into_view()
                                    } else {
                                        view! { <div></div> }.into_view()
                                    }}
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_control_detail.set(false)
                                >
                                    Close
                                </button>
                            </div>
                        </div>
                    </div>
                })
            } else {
                None
            }}

            // Generate Report Modal
            {move || if show_generate_report.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_generate_report.set(false)>
                        <div class="modal-content generate-report-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Generate Compliance Report</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_generate_report.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <form on:submit=move |ev| {
                                    ev.prevent_default();
                                    generate_report();
                                }>
                                    <div class="form-group">
                                        <label for="report-framework">Framework</label>
                                        <select
                                            id="report-framework"
                                            class="form-control"
                                            prop:value=report_framework
                                            on:change=move |ev| {
                                                set_report_framework.set(event_target_value(&ev));
                                            }
                                            required
                                        >
                                            <option value="">Select Framework</option>
                                            {move || frameworks.get().into_iter().map(|fw| view! {
                                                <option value={fw.id.clone()}>{fw.name}</option>
                                            }).collect::<Vec<_>>()}
                                        </select>
                                    </div>

                                    <div class="form-group">
                                        <label for="report-type">Report Type</label>
                                        <select
                                            id="report-type"
                                            class="form-control"
                                            prop:value=report_type
                                            on:change=move |ev| {
                                                set_report_type.set(event_target_value(&ev));
                                            }
                                        >
                                            <option value="full">Full Report</option>
                                            <option value="summary">Summary Report</option>
                                            <option value="executive">Executive Summary</option>
                                        </select>
                                    </div>

                                    <div class="form-row">
                                        <div class="form-group">
                                            <label for="period-start">Period Start</label>
                                            <input
                                                type="date"
                                                id="period-start"
                                                class="form-control"
                                                prop:value=report_period_start
                                                on:input=move |ev| {
                                                    set_report_period_start.set(event_target_value(&ev));
                                                }
                                                required
                                            />
                                        </div>
                                        <div class="form-group">
                                            <label for="period-end">Period End</label>
                                            <input
                                                type="date"
                                                id="period-end"
                                                class="form-control"
                                                prop:value=report_period_end
                                                on:input=move |ev| {
                                                    set_report_period_end.set(event_target_value(&ev));
                                                }
                                                required
                                            />
                                        </div>
                                    </div>

                                    <div class="form-group">
                                        <label for="report-format">Format</label>
                                        <select
                                            id="report-format"
                                            class="form-control"
                                            prop:value=report_format
                                            on:change=move |ev| {
                                                set_report_format.set(event_target_value(&ev));
                                            }
                                        >
                                            <option value="pdf">PDF</option>
                                            <option value="docx">Word Document</option>
                                            <option value="html">HTML</option>
                                        </select>
                                    </div>

                                    <div class="form-group">
                                        <label class="checkbox-label">
                                            <input
                                                type="checkbox"
                                                prop:checked=report_include_evidence
                                                on:change=move |ev| {
                                                    set_report_include_evidence.set(event_target_checked(&ev));
                                                }
                                            />
                                            Include evidence attachments
                                        </label>
                                    </div>

                                    <div class="form-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| set_show_generate_report.set(false)
                                        >
                                            Cancel
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || generating_report.get()
                                        >
                                            {move || if generating_report.get() {
                                                "Generating..."
                                            } else {
                                                "Generate Report"
                                            }}
                                        </button>
                                    </div>
                                </form>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Override Modal
            {move || if show_override_modal.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_override_modal.set(false)>
                        <div class="modal-content override-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Manual Override</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_override_modal.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="form-group">
                                    <label for="override-status">Override Status</label>
                                    <select
                                        id="override-status"
                                        class="form-control"
                                        prop:value=override_status
                                        on:change=move |ev| {
                                            set_override_status.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="pass">Pass</option>
                                        <option value="not_applicable">Not Applicable</option>
                                    </select>
                                </div>

                                <div class="form-group">
                                    <label for="override-reason">Reason</label>
                                    <textarea
                                        id="override-reason"
                                        class="form-control"
                                        rows="4"
                                        placeholder="Explain why this control status is being overridden..."
                                        prop:value=override_reason
                                        on:input=move |ev| {
                                            set_override_reason.set(event_target_value(&ev));
                                        }
                                        required
                                    ></textarea>
                                </div>

                                <div class="form-group">
                                    <label for="override-expires">Expires (Optional)</label>
                                    <input
                                        type="date"
                                        id="override-expires"
                                        class="form-control"
                                        prop:value=override_expires
                                        on:input=move |ev| {
                                            set_override_expires.set(event_target_value(&ev));
                                        }
                                    />
                                    <small class="form-text">
                                        Leave empty for a permanent override
                                    </small>
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_override_modal.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| apply_override()
                                    disabled=move || override_reason.get().is_empty()
                                >
                                    Apply Override
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}
        </div>
    }
}