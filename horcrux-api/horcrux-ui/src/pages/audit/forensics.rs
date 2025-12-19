use leptos::*;
use crate::api::*;

// All types imported from crate::api::* (Investigation, TimelineEntry, Finding, Artifact, CreateInvestigationRequest)

#[component]
pub fn ForensicsPage() -> impl IntoView {
    let (investigations, set_investigations) = create_signal(Vec::<Investigation>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Filter states
    let (search_term, set_search_term) = create_signal(String::new());
    let (filter_status, set_filter_status) = create_signal("all".to_string());
    let (filter_severity, set_filter_severity) = create_signal("all".to_string());

    // View states
    let (selected_investigation, set_selected_investigation) = create_signal(None::<Investigation>);
    let (current_view, set_current_view) = create_signal("list".to_string());

    // Modal states
    let (show_create_investigation, set_show_create_investigation) = create_signal(false);
    let (show_add_finding, set_show_add_finding) = create_signal(false);
    let (show_collect_artifact, set_show_collect_artifact) = create_signal(false);

    // Investigation form states
    let (inv_title, set_inv_title) = create_signal(String::new());
    let (inv_description, set_inv_description) = create_signal(String::new());
    let (inv_severity, set_inv_severity) = create_signal("medium".to_string());
    let (inv_tags, set_inv_tags) = create_signal(String::new());

    // Finding form states
    let (finding_title, set_finding_title) = create_signal(String::new());
    let (finding_description, set_finding_description) = create_signal(String::new());
    let (finding_type, set_finding_type) = create_signal("contributing_factor".to_string());
    let (finding_severity, set_finding_severity) = create_signal("medium".to_string());

    // Artifact collection states
    let (artifact_type, set_artifact_type) = create_signal("log".to_string());
    let (artifact_source, set_artifact_source) = create_signal(String::new());
    let (artifact_time_start, set_artifact_time_start) = create_signal(String::new());
    let (artifact_time_end, set_artifact_time_end) = create_signal(String::new());

    // Load data on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match get_investigations().await {
                Ok(invs) => {
                    set_investigations.set(invs);
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load investigations: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Filtered investigations
    let filtered_investigations = create_memo(move |_| {
        investigations.get()
            .into_iter()
            .filter(|inv| {
                let search_match = if search_term.get().is_empty() {
                    true
                } else {
                    let term = search_term.get().to_lowercase();
                    inv.title.to_lowercase().contains(&term) ||
                    inv.description.to_lowercase().contains(&term) ||
                    inv.tags.iter().any(|t| t.to_lowercase().contains(&term))
                };

                let status_match = filter_status.get() == "all" || inv.status == filter_status.get();
                let severity_match = filter_severity.get() == "all" || inv.severity == filter_severity.get();

                search_match && status_match && severity_match
            })
            .collect::<Vec<_>>()
    });

    // Create investigation
    let create_investigation_action = create_action(move |_: &()| async move {
        let request = CreateInvestigationRequest {
            title: inv_title.get(),
            description: inv_description.get(),
            severity: inv_severity.get(),
            related_events: vec![],
            tags: inv_tags.get().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        };

        match create_investigation(request).await {
            Ok(inv) => {
                set_show_create_investigation.set(false);
                set_inv_title.set(String::new());
                set_inv_description.set(String::new());
                set_inv_severity.set("medium".to_string());
                set_inv_tags.set(String::new());

                // Reload investigations
                if let Ok(invs) = get_investigations().await {
                    set_investigations.set(invs);
                }

                // Open the new investigation
                set_selected_investigation.set(Some(inv));
                set_current_view.set("detail".to_string());
                true
            }
            Err(_) => false
        }
    });

    // Add finding to investigation
    let add_finding = move || {
        if let Some(inv) = selected_investigation.get() {
            let finding = Finding {
                id: uuid::Uuid::new_v4().to_string(),
                title: finding_title.get(),
                description: finding_description.get(),
                finding_type: finding_type.get(),
                severity: finding_severity.get(),
                evidence: vec![],
                created_at: chrono::Utc::now().to_rfc3339(),
                created_by: "current_user".to_string(),
            };

            spawn_local(async move {
                if let Ok(_) = add_investigation_finding(inv.id.clone(), finding).await {
                    set_show_add_finding.set(false);
                    set_finding_title.set(String::new());
                    set_finding_description.set(String::new());

                    // Reload investigation
                    if let Ok(updated_inv) = get_investigation(inv.id).await {
                        set_selected_investigation.set(Some(updated_inv));
                    }
                }
            });
        }
    };

    // Collect artifact
    let collect_artifact = move || {
        if let Some(inv) = selected_investigation.get() {
            spawn_local(async move {
                let request = serde_json::json!({
                    "artifact_type": artifact_type.get(),
                    "source": artifact_source.get(),
                    "time_start": artifact_time_start.get(),
                    "time_end": artifact_time_end.get(),
                });

                if let Ok(_) = collect_investigation_artifact(inv.id.clone(), request).await {
                    set_show_collect_artifact.set(false);
                    set_artifact_source.set(String::new());
                    set_artifact_time_start.set(String::new());
                    set_artifact_time_end.set(String::new());

                    // Reload investigation
                    if let Ok(updated_inv) = get_investigation(inv.id).await {
                        set_selected_investigation.set(Some(updated_inv));
                    }
                }
            });
        }
    };

    // Update investigation status
    let update_status = move |new_status: String| {
        if let Some(inv) = selected_investigation.get() {
            spawn_local(async move {
                if let Ok(_) = update_investigation_status(inv.id.clone(), new_status).await {
                    // Reload investigation
                    if let Ok(updated_inv) = get_investigation(inv.id).await {
                        set_selected_investigation.set(Some(updated_inv));
                    }
                    // Reload list
                    if let Ok(invs) = get_investigations().await {
                        set_investigations.set(invs);
                    }
                }
            });
        }
    };

    view! {
        <div class="forensics-page">
            <div class="page-header">
                <h1 class="page-title">Security Forensics</h1>
                <p class="page-description">
                    Investigate security incidents and collect forensic evidence
                </p>

                <div class="page-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_create_investigation.set(true)
                    >
                        New Investigation
                    </button>
                </div>
            </div>

            {move || match current_view.get().as_str() {
                "list" => view! {
                    <div class="investigations-list-view">
                        // Filters
                        <div class="investigations-filters">
                            <div class="filter-row">
                                <div class="search-box">
                                    <input
                                        type="text"
                                        placeholder="Search investigations..."
                                        class="search-input"
                                        prop:value=search_term
                                        on:input=move |ev| {
                                            set_search_term.set(event_target_value(&ev));
                                        }
                                    />
                                </div>

                                <div class="filter-selects">
                                    <select
                                        class="filter-select"
                                        prop:value=filter_status
                                        on:change=move |ev| {
                                            set_filter_status.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Status</option>
                                        <option value="open">Open</option>
                                        <option value="in_progress">In Progress</option>
                                        <option value="closed">Closed</option>
                                        <option value="archived">Archived</option>
                                    </select>

                                    <select
                                        class="filter-select"
                                        prop:value=filter_severity
                                        on:change=move |ev| {
                                            set_filter_severity.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="all">All Severities</option>
                                        <option value="critical">Critical</option>
                                        <option value="high">High</option>
                                        <option value="medium">Medium</option>
                                        <option value="low">Low</option>
                                    </select>
                                </div>
                            </div>
                        </div>

                        // Investigations list
                        {move || if loading.get() {
                            view! {
                                <div class="loading-container">
                                    <div class="spinner"></div>
                                    <p>Loading investigations...</p>
                                </div>
                            }.into_view()
                        } else if let Some(err) = error.get() {
                            view! {
                                <div class="error-container">
                                    <div class="error-message">
                                        <h3>Error Loading Investigations</h3>
                                        <p>{err}</p>
                                    </div>
                                </div>
                            }.into_view()
                        } else if filtered_investigations.get().is_empty() {
                            view! {
                                <div class="empty-state">
                                    <h3>No Investigations Found</h3>
                                    <p>Start a new investigation to begin forensic analysis</p>
                                    <button
                                        class="btn btn-primary"
                                        on:click=move |_| set_show_create_investigation.set(true)
                                    >
                                        New Investigation
                                    </button>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="investigations-grid">
                                    {filtered_investigations.get().into_iter().map(|inv| {
                                        let inv_clone = inv.clone();
                                        view! {
                                            <div class={format!("investigation-card severity-{}", inv.severity)}>
                                                <div class="investigation-header">
                                                    <h3>{inv.title.clone()}</h3>
                                                    <div class="investigation-badges">
                                                        <span class={format!("severity-badge severity-{}", inv.severity)}>
                                                            {inv.severity.to_uppercase()}
                                                        </span>
                                                        <span class={format!("status-badge status-{}", inv.status)}>
                                                            {inv.status.replace("_", " ").to_uppercase()}
                                                        </span>
                                                    </div>
                                                </div>

                                                <p class="investigation-description">
                                                    {inv.description.clone()}
                                                </p>

                                                <div class="investigation-stats">
                                                    <div class="stat-item">
                                                        <span class="stat-value">{inv.timeline.len().to_string()}</span>
                                                        <span class="stat-label">Timeline Events</span>
                                                    </div>
                                                    <div class="stat-item">
                                                        <span class="stat-value">{inv.findings.len().to_string()}</span>
                                                        <span class="stat-label">Findings</span>
                                                    </div>
                                                    <div class="stat-item">
                                                        <span class="stat-value">{inv.artifacts.len().to_string()}</span>
                                                        <span class="stat-label">Artifacts</span>
                                                    </div>
                                                </div>

                                                <div class="investigation-meta">
                                                    <span>Created: {inv.created_at.clone()}</span>
                                                    {inv.assigned_to.as_ref().map(|assignee| view! {
                                                        <span>Assigned to: {assignee.clone()}</span>
                                                    })}
                                                </div>

                                                {if !inv.tags.is_empty() {
                                                    view! {
                                                        <div class="investigation-tags">
                                                            {inv.tags.iter().map(|tag| view! {
                                                                <span class="tag">{tag.clone()}</span>
                                                            }).collect::<Vec<_>>()}
                                                        </div>
                                                    }.into_view()
                                                } else {
                                                    view! { <div></div> }.into_view()
                                                }}

                                                <div class="investigation-actions">
                                                    <button
                                                        class="btn btn-primary"
                                                        on:click=move |_| {
                                                            set_selected_investigation.set(Some(inv_clone.clone()));
                                                            set_current_view.set("detail".to_string());
                                                        }
                                                    >
                                                        Open Investigation
                                                    </button>
                                                </div>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_view()
                        }}
                    </div>
                }.into_view(),

                "detail" => view! {
                    <div class="investigation-detail-view">
                        {selected_investigation.get().map(|inv| {
                            let inv_id = inv.id.clone();
                            view! {
                                <div class="investigation-detail">
                                    <div class="detail-header">
                                        <button
                                            class="btn btn-link back-btn"
                                            on:click=move |_| {
                                                set_current_view.set("list".to_string());
                                                set_selected_investigation.set(None);
                                            }
                                        >
                                            "<-" Back to List
                                        </button>

                                        <div class="detail-title">
                                            <h2>{inv.title.clone()}</h2>
                                            <div class="detail-badges">
                                                <span class={format!("severity-badge severity-{}", inv.severity)}>
                                                    {inv.severity.to_uppercase()}
                                                </span>
                                                <span class={format!("status-badge status-{}", inv.status)}>
                                                    {inv.status.replace("_", " ").to_uppercase()}
                                                </span>
                                            </div>
                                        </div>

                                        <div class="detail-actions">
                                            {if inv.status == "open" {
                                                view! {
                                                    <button
                                                        class="btn btn-primary"
                                                        on:click=move |_| update_status("in_progress".to_string())
                                                    >
                                                        Start Investigation
                                                    </button>
                                                }.into_view()
                                            } else if inv.status == "in_progress" {
                                                view! {
                                                    <button
                                                        class="btn btn-success"
                                                        on:click=move |_| update_status("closed".to_string())
                                                    >
                                                        Close Investigation
                                                    </button>
                                                }.into_view()
                                            } else {
                                                view! { <div></div> }.into_view()
                                            }}
                                        </div>
                                    </div>

                                    <p class="detail-description">{inv.description.clone()}</p>

                                    // Timeline Section
                                    <div class="detail-section">
                                        <div class="section-header">
                                            <h3>Timeline ({inv.timeline.len()} events)</h3>
                                        </div>

                                        {if inv.timeline.is_empty() {
                                            view! {
                                                <div class="empty-section">
                                                    <p>No timeline events yet</p>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="timeline">
                                                    {inv.timeline.iter().map(|entry| view! {
                                                        <div class={format!("timeline-entry {}", if entry.important { "important" } else { "" })}>
                                                            <div class="timeline-marker"></div>
                                                            <div class="timeline-content">
                                                                <div class="timeline-header">
                                                                    <span class="timeline-time">{entry.timestamp.clone()}</span>
                                                                    <span class="timeline-type">{entry.event_type.replace("_", " ")}</span>
                                                                    <span class="timeline-source">{entry.source.clone()}</span>
                                                                </div>
                                                                <p class="timeline-description">{entry.description.clone()}</p>
                                                            </div>
                                                        </div>
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            }.into_view()
                                        }}
                                    </div>

                                    // Findings Section
                                    <div class="detail-section">
                                        <div class="section-header">
                                            <h3>Findings ({inv.findings.len()})</h3>
                                            <button
                                                class="btn btn-sm btn-primary"
                                                on:click=move |_| set_show_add_finding.set(true)
                                            >
                                                Add Finding
                                            </button>
                                        </div>

                                        {if inv.findings.is_empty() {
                                            view! {
                                                <div class="empty-section">
                                                    <p>No findings documented yet</p>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="findings-list">
                                                    {inv.findings.iter().map(|finding| view! {
                                                        <div class={format!("finding-card finding-type-{}", finding.finding_type)}>
                                                            <div class="finding-header">
                                                                <h4>{finding.title.clone()}</h4>
                                                                <div class="finding-badges">
                                                                    <span class="finding-type-badge">
                                                                        {finding.finding_type.replace("_", " ")}
                                                                    </span>
                                                                    <span class={format!("severity-badge severity-{}", finding.severity)}>
                                                                        {finding.severity.to_uppercase()}
                                                                    </span>
                                                                </div>
                                                            </div>
                                                            <p class="finding-description">{finding.description.clone()}</p>
                                                            <div class="finding-meta">
                                                                <span>Added by {finding.created_by.clone()} on {finding.created_at.clone()}</span>
                                                            </div>
                                                        </div>
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            }.into_view()
                                        }}
                                    </div>

                                    // Artifacts Section
                                    <div class="detail-section">
                                        <div class="section-header">
                                            <h3>Artifacts ({inv.artifacts.len()})</h3>
                                            <button
                                                class="btn btn-sm btn-primary"
                                                on:click=move |_| set_show_collect_artifact.set(true)
                                            >
                                                Collect Artifact
                                            </button>
                                        </div>

                                        {if inv.artifacts.is_empty() {
                                            view! {
                                                <div class="empty-section">
                                                    <p>No artifacts collected yet</p>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="artifacts-list">
                                                    <table class="artifacts-table">
                                                        <thead>
                                                            <tr>
                                                                <th>Name</th>
                                                                <th>Type</th>
                                                                <th>Size</th>
                                                                <th>Hash (SHA256)</th>
                                                                <th>Collected</th>
                                                                <th>Actions</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {inv.artifacts.iter().map(|artifact| view! {
                                                                <tr>
                                                                    <td>
                                                                        <div class="artifact-name">
                                                                            {artifact.name.clone()}
                                                                            <small>{artifact.description.clone()}</small>
                                                                        </div>
                                                                    </td>
                                                                    <td>{artifact.artifact_type.replace("_", " ")}</td>
                                                                    <td>{format_bytes(artifact.size_bytes)}</td>
                                                                    <td class="hash-cell">
                                                                        <code>{artifact.hash.chars().take(16).collect::<String>()}...</code>
                                                                    </td>
                                                                    <td>
                                                                        <div>{artifact.collected_at.clone()}</div>
                                                                        <small>by {artifact.collected_by.clone()}</small>
                                                                    </td>
                                                                    <td>
                                                                        {artifact.download_url.as_ref().map(|url| view! {
                                                                            <a
                                                                                href={url.clone()}
                                                                                class="btn btn-sm btn-secondary"
                                                                                target="_blank"
                                                                            >
                                                                                Download
                                                                            </a>
                                                                        })}
                                                                    </td>
                                                                </tr>
                                                            }).collect::<Vec<_>>()}
                                                        </tbody>
                                                    </table>
                                                </div>
                                            }.into_view()
                                        }}
                                    </div>
                                </div>
                            }.into_view()
                        }).unwrap_or_else(|| view! {
                            <div class="empty-state">
                                <p>No investigation selected</p>
                            </div>
                        }.into_view())}
                    </div>
                }.into_view(),

                _ => view! { <div></div> }.into_view()
            }}

            // Create Investigation Modal
            {move || if show_create_investigation.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_create_investigation.set(false)>
                        <div class="modal-content create-investigation-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>New Investigation</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_create_investigation.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <form on:submit=move |ev| {
                                    ev.prevent_default();
                                    create_investigation_action.dispatch(());
                                }>
                                    <div class="form-group">
                                        <label for="inv-title">Title</label>
                                        <input
                                            type="text"
                                            id="inv-title"
                                            class="form-control"
                                            placeholder="Investigation title"
                                            prop:value=inv_title
                                            on:input=move |ev| {
                                                set_inv_title.set(event_target_value(&ev));
                                            }
                                            required
                                        />
                                    </div>

                                    <div class="form-group">
                                        <label for="inv-description">Description</label>
                                        <textarea
                                            id="inv-description"
                                            class="form-control"
                                            rows="4"
                                            placeholder="Describe the security incident being investigated..."
                                            prop:value=inv_description
                                            on:input=move |ev| {
                                                set_inv_description.set(event_target_value(&ev));
                                            }
                                            required
                                        ></textarea>
                                    </div>

                                    <div class="form-group">
                                        <label for="inv-severity">Severity</label>
                                        <select
                                            id="inv-severity"
                                            class="form-control"
                                            prop:value=inv_severity
                                            on:change=move |ev| {
                                                set_inv_severity.set(event_target_value(&ev));
                                            }
                                        >
                                            <option value="low">Low</option>
                                            <option value="medium">Medium</option>
                                            <option value="high">High</option>
                                            <option value="critical">Critical</option>
                                        </select>
                                    </div>

                                    <div class="form-group">
                                        <label for="inv-tags">Tags (comma-separated)</label>
                                        <input
                                            type="text"
                                            id="inv-tags"
                                            class="form-control"
                                            placeholder="malware, ransomware, phishing"
                                            prop:value=inv_tags
                                            on:input=move |ev| {
                                                set_inv_tags.set(event_target_value(&ev));
                                            }
                                        />
                                    </div>

                                    <div class="form-actions">
                                        <button
                                            type="button"
                                            class="btn btn-secondary"
                                            on:click=move |_| set_show_create_investigation.set(false)
                                        >
                                            Cancel
                                        </button>
                                        <button
                                            type="submit"
                                            class="btn btn-primary"
                                            disabled=move || create_investigation_action.pending().get()
                                        >
                                            {move || if create_investigation_action.pending().get() {
                                                "Creating..."
                                            } else {
                                                "Create Investigation"
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

            // Add Finding Modal
            {move || if show_add_finding.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_add_finding.set(false)>
                        <div class="modal-content add-finding-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Add Finding</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_add_finding.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="form-group">
                                    <label for="finding-title">Title</label>
                                    <input
                                        type="text"
                                        id="finding-title"
                                        class="form-control"
                                        prop:value=finding_title
                                        on:input=move |ev| {
                                            set_finding_title.set(event_target_value(&ev));
                                        }
                                        required
                                    />
                                </div>

                                <div class="form-group">
                                    <label for="finding-type">Finding Type</label>
                                    <select
                                        id="finding-type"
                                        class="form-control"
                                        prop:value=finding_type
                                        on:change=move |ev| {
                                            set_finding_type.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="root_cause">Root Cause</option>
                                        <option value="contributing_factor">Contributing Factor</option>
                                        <option value="impact">Impact</option>
                                        <option value="recommendation">Recommendation</option>
                                    </select>
                                </div>

                                <div class="form-group">
                                    <label for="finding-severity">Severity</label>
                                    <select
                                        id="finding-severity"
                                        class="form-control"
                                        prop:value=finding_severity
                                        on:change=move |ev| {
                                            set_finding_severity.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="low">Low</option>
                                        <option value="medium">Medium</option>
                                        <option value="high">High</option>
                                        <option value="critical">Critical</option>
                                    </select>
                                </div>

                                <div class="form-group">
                                    <label for="finding-description">Description</label>
                                    <textarea
                                        id="finding-description"
                                        class="form-control"
                                        rows="4"
                                        prop:value=finding_description
                                        on:input=move |ev| {
                                            set_finding_description.set(event_target_value(&ev));
                                        }
                                        required
                                    ></textarea>
                                </div>

                                <div class="form-actions">
                                    <button
                                        type="button"
                                        class="btn btn-secondary"
                                        on:click=move |_| set_show_add_finding.set(false)
                                    >
                                        Cancel
                                    </button>
                                    <button
                                        type="button"
                                        class="btn btn-primary"
                                        on:click=move |_| add_finding()
                                        disabled=move || finding_title.get().is_empty() || finding_description.get().is_empty()
                                    >
                                        Add Finding
                                    </button>
                                </div>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Collect Artifact Modal
            {move || if show_collect_artifact.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_collect_artifact.set(false)>
                        <div class="modal-content collect-artifact-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Collect Artifact</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_collect_artifact.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="form-group">
                                    <label for="artifact-type">Artifact Type</label>
                                    <select
                                        id="artifact-type"
                                        class="form-control"
                                        prop:value=artifact_type
                                        on:change=move |ev| {
                                            set_artifact_type.set(event_target_value(&ev));
                                        }
                                    >
                                        <option value="log">System Logs</option>
                                        <option value="config">Configuration Files</option>
                                        <option value="network_capture">Network Capture</option>
                                        <option value="memory_dump">Memory Dump</option>
                                        <option value="disk_image">Disk Image</option>
                                    </select>
                                </div>

                                <div class="form-group">
                                    <label for="artifact-source">Source</label>
                                    <input
                                        type="text"
                                        id="artifact-source"
                                        class="form-control"
                                        placeholder="Hostname, IP, or path"
                                        prop:value=artifact_source
                                        on:input=move |ev| {
                                            set_artifact_source.set(event_target_value(&ev));
                                        }
                                        required
                                    />
                                </div>

                                <div class="form-row">
                                    <div class="form-group">
                                        <label for="artifact-time-start">Time Range Start</label>
                                        <input
                                            type="datetime-local"
                                            id="artifact-time-start"
                                            class="form-control"
                                            prop:value=artifact_time_start
                                            on:input=move |ev| {
                                                set_artifact_time_start.set(event_target_value(&ev));
                                            }
                                        />
                                    </div>
                                    <div class="form-group">
                                        <label for="artifact-time-end">Time Range End</label>
                                        <input
                                            type="datetime-local"
                                            id="artifact-time-end"
                                            class="form-control"
                                            prop:value=artifact_time_end
                                            on:input=move |ev| {
                                                set_artifact_time_end.set(event_target_value(&ev));
                                            }
                                        />
                                    </div>
                                </div>

                                <div class="form-actions">
                                    <button
                                        type="button"
                                        class="btn btn-secondary"
                                        on:click=move |_| set_show_collect_artifact.set(false)
                                    >
                                        Cancel
                                    </button>
                                    <button
                                        type="button"
                                        class="btn btn-primary"
                                        on:click=move |_| collect_artifact()
                                        disabled=move || artifact_source.get().is_empty()
                                    >
                                        Collect Artifact
                                    </button>
                                </div>
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

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.2} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.2} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.2} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}