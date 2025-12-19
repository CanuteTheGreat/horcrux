use leptos::*;
use crate::api::{
    BackupValidation, RestoreTest, ValidationSchedule,
    get_backup_validations, get_restore_tests, get_validation_schedules,
    start_backup_validation, start_restore_test,
};

#[component]
pub fn BackupValidationPage() -> impl IntoView {
    let (validations, set_validations) = create_signal(Vec::<BackupValidation>::new());
    let (restore_tests, set_restore_tests) = create_signal(Vec::<RestoreTest>::new());
    let (schedules, set_schedules) = create_signal(Vec::<ValidationSchedule>::new());
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);
    let (active_tab, set_active_tab) = create_signal("validations".to_string());

    // Modal states
    let (show_new_validation, set_show_new_validation) = create_signal(false);
    let (show_new_restore_test, set_show_new_restore_test) = create_signal(false);
    let (show_validation_detail, set_show_validation_detail) = create_signal(false);
    let (selected_validation, set_selected_validation) = create_signal(None::<BackupValidation>);
    let (selected_restore_test, set_selected_restore_test) = create_signal(None::<RestoreTest>);

    // Form states
    let (selected_backup_id, set_selected_backup_id) = create_signal(String::new());
    let (validation_type, set_validation_type) = create_signal("integrity".to_string());
    let (test_type, set_test_type) = create_signal("full_restore".to_string());
    let (target_env, set_target_env) = create_signal("sandbox".to_string());

    // Load data
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);

            match get_backup_validations().await {
                Ok(list) => set_validations.set(list),
                Err(e) => set_error.set(Some(format!("Failed to load validations: {}", e))),
            }

            match get_restore_tests().await {
                Ok(list) => set_restore_tests.set(list),
                Err(_) => {}
            }

            match get_validation_schedules().await {
                Ok(list) => set_schedules.set(list),
                Err(_) => {}
            }

            set_loading.set(false);
        });
    });

    // Statistics
    let validation_stats = move || {
        let all = validations.get();
        let passed = all.iter().filter(|v| v.status == "passed").count();
        let failed = all.iter().filter(|v| v.status == "failed").count();
        let pending = all.iter().filter(|v| v.status == "pending" || v.status == "running").count();
        (all.len(), passed, failed, pending)
    };

    let start_validation = move |_| {
        let backup_id = selected_backup_id.get();
        let vtype = validation_type.get();

        spawn_local(async move {
            match start_backup_validation(&backup_id, &vtype).await {
                Ok(_) => {
                    set_show_new_validation.set(false);
                    if let Ok(list) = get_backup_validations().await {
                        set_validations.set(list);
                    }
                }
                Err(e) => set_error.set(Some(format!("Failed to start validation: {}", e))),
            }
        });
    };

    let start_restore_test_fn = move |_| {
        let backup_id = selected_backup_id.get();
        let ttype = test_type.get();
        let target = target_env.get();

        spawn_local(async move {
            match start_restore_test(&backup_id, &ttype, &target).await {
                Ok(_) => {
                    set_show_new_restore_test.set(false);
                    if let Ok(list) = get_restore_tests().await {
                        set_restore_tests.set(list);
                    }
                }
                Err(e) => set_error.set(Some(format!("Failed to start restore test: {}", e))),
            }
        });
    };

    view! {
        <div class="backup-validation-page">
            <div class="page-header">
                <h1 class="page-title">"Backup Validation and Testing"</h1>
                <p class="page-description">
                    "Validate backup integrity and test restore procedures"
                </p>
                <div class="header-actions">
                    <button
                        class="btn btn-primary"
                        on:click=move |_| set_show_new_validation.set(true)
                    >
                        "Validate Backup"
                    </button>
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| set_show_new_restore_test.set(true)
                    >
                        "Test Restore"
                    </button>
                </div>
            </div>

            // Statistics Overview
            <div class="stats-grid">
                <div class="stat-card">
                    <span class="stat-label">"Total Validations"</span>
                    <span class="stat-value">{move || validation_stats().0}</span>
                </div>
                <div class="stat-card success">
                    <span class="stat-label">"Passed"</span>
                    <span class="stat-value">{move || validation_stats().1}</span>
                </div>
                <div class="stat-card danger">
                    <span class="stat-label">"Failed"</span>
                    <span class="stat-value">{move || validation_stats().2}</span>
                </div>
                <div class="stat-card warning">
                    <span class="stat-label">"In Progress"</span>
                    <span class="stat-value">{move || validation_stats().3}</span>
                </div>
            </div>

            // Tab Navigation
            <div class="tab-nav">
                <button
                    class={move || if active_tab.get() == "validations" { "tab-btn active" } else { "tab-btn" }}
                    on:click=move |_| set_active_tab.set("validations".to_string())
                >
                    "Validations"
                </button>
                <button
                    class={move || if active_tab.get() == "restore_tests" { "tab-btn active" } else { "tab-btn" }}
                    on:click=move |_| set_active_tab.set("restore_tests".to_string())
                >
                    "Restore Tests"
                </button>
                <button
                    class={move || if active_tab.get() == "schedules" { "tab-btn active" } else { "tab-btn" }}
                    on:click=move |_| set_active_tab.set("schedules".to_string())
                >
                    "Schedules"
                </button>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>"Loading validation data..."</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>"Error"</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                match active_tab.get().as_str() {
                    "validations" => view! {
                        <div class="validations-section">
                            <table class="data-table">
                                <thead>
                                    <tr>
                                        <th>"Backup"</th>
                                        <th>"Type"</th>
                                        <th>"Status"</th>
                                        <th>"Files Checked"</th>
                                        <th>"Issues"</th>
                                        <th>"Duration"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {validations.get().into_iter().map(|validation| {
                                        let v_clone = validation.clone();
                                        view! {
                                            <tr class={format!("status-{}", validation.status)}>
                                                <td>
                                                    <div class="backup-info">
                                                        <span class="backup-name">{validation.backup_name.clone()}</span>
                                                        <span class="vm-name">{validation.vm_name.clone()}</span>
                                                    </div>
                                                </td>
                                                <td>
                                                    <span class="type-badge">{validation.validation_type.to_uppercase()}</span>
                                                </td>
                                                <td>
                                                    <span class={format!("status-badge status-{}", validation.status)}>
                                                        {validation.status.to_uppercase()}
                                                    </span>
                                                </td>
                                                <td>{validation.results.files_checked}</td>
                                                <td>
                                                    {if validation.results.issues.is_empty() {
                                                        view! { <span class="no-issues">"None"</span> }.into_view()
                                                    } else {
                                                        view! {
                                                            <span class="issue-count">
                                                                {validation.results.issues.len()}
                                                            </span>
                                                        }.into_view()
                                                    }}
                                                </td>
                                                <td>
                                                    {validation.duration_seconds.map(|d| format!("{}s", d)).unwrap_or_else(|| "-".to_string())}
                                                </td>
                                                <td>
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        on:click=move |_| {
                                                            set_selected_validation.set(Some(v_clone.clone()));
                                                            set_show_validation_detail.set(true);
                                                        }
                                                    >
                                                        "Details"
                                                    </button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_view(),

                    "restore_tests" => view! {
                        <div class="restore-tests-section">
                            <table class="data-table">
                                <thead>
                                    <tr>
                                        <th>"Backup"</th>
                                        <th>"Test Type"</th>
                                        <th>"Environment"</th>
                                        <th>"Status"</th>
                                        <th>"Progress"</th>
                                        <th>"Result"</th>
                                        <th>"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {restore_tests.get().into_iter().map(|test| {
                                        let test_clone = test.clone();
                                        view! {
                                            <tr class={format!("status-{}", test.status)}>
                                                <td>{test.backup_name.clone()}</td>
                                                <td>{test.test_type.clone()}</td>
                                                <td>{test.target_environment.clone()}</td>
                                                <td>
                                                    <span class={format!("status-badge status-{}", test.status)}>
                                                        {test.status.to_uppercase()}
                                                    </span>
                                                </td>
                                                <td>
                                                    <div class="progress-bar">
                                                        <div
                                                            class="progress-fill"
                                                            style={format!("width: {}%", test.progress)}
                                                        ></div>
                                                    </div>
                                                    <span class="progress-text">{format!("{:.0}%", test.progress)}</span>
                                                </td>
                                                <td>
                                                    {if test.test_results.restore_successful {
                                                        view! { <span class="result-pass">"PASS"</span> }.into_view()
                                                    } else if test.status == "failed" {
                                                        view! { <span class="result-fail">"FAIL"</span> }.into_view()
                                                    } else {
                                                        view! { <span class="result-pending">"-"</span> }.into_view()
                                                    }}
                                                </td>
                                                <td>
                                                    <button
                                                        class="btn btn-sm btn-secondary"
                                                        on:click=move |_| {
                                                            set_selected_restore_test.set(Some(test_clone.clone()));
                                                        }
                                                    >
                                                        "View"
                                                    </button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_view(),

                    "schedules" => view! {
                        <div class="schedules-section">
                            <div class="schedules-grid">
                                {schedules.get().into_iter().map(|schedule| view! {
                                    <div class="schedule-card">
                                        <div class="schedule-header">
                                            <h3>{schedule.name.clone()}</h3>
                                            <span class={if schedule.enabled { "badge badge-success" } else { "badge badge-secondary" }}>
                                                {if schedule.enabled { "Enabled" } else { "Disabled" }}
                                            </span>
                                        </div>
                                        <div class="schedule-info">
                                            <div class="info-row">
                                                <span class="label">"Schedule:"</span>
                                                <span class="value">{schedule.schedule.clone()}</span>
                                            </div>
                                            <div class="info-row">
                                                <span class="label">"Selection:"</span>
                                                <span class="value">{schedule.backup_selection.clone()}</span>
                                            </div>
                                            <div class="info-row">
                                                <span class="label">"Types:"</span>
                                                <span class="value">{schedule.validation_types.join(", ")}</span>
                                            </div>
                                        </div>
                                    </div>
                                }).collect::<Vec<_>>()}
                            </div>
                        </div>
                    }.into_view(),

                    _ => view! { <div></div> }.into_view()
                }
            }}

            // New Validation Modal
            {move || if show_new_validation.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_new_validation.set(false)>
                        <div class="modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>"Validate Backup"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_new_validation.set(false)
                                >
                                    "x"
                                </button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label>"Select Backup"</label>
                                    <select
                                        class="form-control"
                                        on:change=move |ev| set_selected_backup_id.set(event_target_value(&ev))
                                    >
                                        <option value="">"-- Select a backup --"</option>
                                        // In a real implementation, this would be populated from API
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Validation Type"</label>
                                    <div class="radio-group">
                                        <label class="radio-option">
                                            <input
                                                type="radio"
                                                name="validation_type"
                                                value="checksum"
                                                checked=move || validation_type.get() == "checksum"
                                                on:change=move |_| set_validation_type.set("checksum".to_string())
                                            />
                                            <div>
                                                <span class="option-title">"Checksum Verification"</span>
                                                <span class="option-desc">"Verify file checksums match"</span>
                                            </div>
                                        </label>
                                        <label class="radio-option">
                                            <input
                                                type="radio"
                                                name="validation_type"
                                                value="integrity"
                                                checked=move || validation_type.get() == "integrity"
                                                on:change=move |_| set_validation_type.set("integrity".to_string())
                                            />
                                            <div>
                                                <span class="option-title">"Integrity Check"</span>
                                                <span class="option-desc">"Full structural integrity validation"</span>
                                            </div>
                                        </label>
                                        <label class="radio-option">
                                            <input
                                                type="radio"
                                                name="validation_type"
                                                value="restore_test"
                                                checked=move || validation_type.get() == "restore_test"
                                                on:change=move |_| set_validation_type.set("restore_test".to_string())
                                            />
                                            <div>
                                                <span class="option-title">"Restore Test"</span>
                                                <span class="option-desc">"Test actual restore process"</span>
                                            </div>
                                        </label>
                                    </div>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_new_validation.set(false)
                                >
                                    "Cancel"
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=start_validation
                                >
                                    "Start Validation"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // New Restore Test Modal
            {move || if show_new_restore_test.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_new_restore_test.set(false)>
                        <div class="modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>"Test Restore"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_new_restore_test.set(false)
                                >
                                    "x"
                                </button>
                            </div>
                            <div class="modal-body">
                                <div class="form-group">
                                    <label>"Select Backup"</label>
                                    <select
                                        class="form-control"
                                        on:change=move |ev| set_selected_backup_id.set(event_target_value(&ev))
                                    >
                                        <option value="">"-- Select a backup --"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Test Type"</label>
                                    <select
                                        class="form-control"
                                        on:change=move |ev| set_test_type.set(event_target_value(&ev))
                                    >
                                        <option value="full_restore">"Full Restore Test"</option>
                                        <option value="partial_restore">"Partial Restore Test"</option>
                                        <option value="boot_test">"Boot Test Only"</option>
                                    </select>
                                </div>
                                <div class="form-group">
                                    <label>"Target Environment"</label>
                                    <select
                                        class="form-control"
                                        on:change=move |ev| set_target_env.set(event_target_value(&ev))
                                    >
                                        <option value="sandbox">"Sandbox (Isolated)"</option>
                                        <option value="isolated_vm">"Isolated VM"</option>
                                        <option value="staging">"Staging Environment"</option>
                                    </select>
                                </div>
                                <div class="form-note">
                                    <p>"Note: Restore tests create temporary resources that are automatically cleaned up after testing."</p>
                                </div>
                            </div>
                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_new_restore_test.set(false)
                                >
                                    "Cancel"
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=start_restore_test_fn
                                >
                                    "Start Test"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! {}.into_view()
            }}

            // Validation Detail Modal
            {move || if show_validation_detail.get() {
                if let Some(validation) = selected_validation.get() {
                    view! {
                        <div class="modal-overlay" on:click=move |_| set_show_validation_detail.set(false)>
                            <div class="modal modal-lg" on:click=|e| e.stop_propagation()>
                                <div class="modal-header">
                                    <h2>"Validation Details"</h2>
                                    <button
                                        class="modal-close"
                                        on:click=move |_| set_show_validation_detail.set(false)
                                    >
                                        "x"
                                    </button>
                                </div>
                                <div class="modal-body">
                                    <div class="detail-grid">
                                        <div class="detail-section">
                                            <h3>"Summary"</h3>
                                            <div class="info-grid">
                                                <div class="info-item">
                                                    <span class="label">"Backup:"</span>
                                                    <span class="value">{validation.backup_name.clone()}</span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"Status:"</span>
                                                    <span class={format!("status-badge status-{}", validation.status)}>
                                                        {validation.status.to_uppercase()}
                                                    </span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"Files Checked:"</span>
                                                    <span class="value">{validation.results.files_checked}</span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"Files Passed:"</span>
                                                    <span class="value">{validation.results.files_passed}</span>
                                                </div>
                                                <div class="info-item">
                                                    <span class="label">"Files Failed:"</span>
                                                    <span class="value">{validation.results.files_failed}</span>
                                                </div>
                                            </div>
                                        </div>

                                        {if !validation.results.issues.is_empty() {
                                            view! {
                                                <div class="detail-section">
                                                    <h3>"Issues"</h3>
                                                    <div class="issues-list">
                                                        {validation.results.issues.iter().map(|issue| view! {
                                                            <div class={format!("issue-item severity-{}", issue.severity)}>
                                                                <span class="issue-severity">{issue.severity.to_uppercase()}</span>
                                                                <span class="issue-category">{issue.category.clone()}</span>
                                                                <span class="issue-path">{issue.path.clone()}</span>
                                                                <p class="issue-message">{issue.message.clone()}</p>
                                                            </div>
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {}.into_view()
                                        }}
                                    </div>
                                </div>
                                <div class="modal-footer">
                                    <button
                                        class="btn btn-secondary"
                                        on:click=move |_| set_show_validation_detail.set(false)
                                    >
                                        "Close"
                                    </button>
                                </div>
                            </div>
                        </div>
                    }.into_view()
                } else {
                    view! {}.into_view()
                }
            } else {
                view! {}.into_view()
            }}
        </div>
    }
}
