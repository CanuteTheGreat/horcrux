use leptos::*;
use leptos_router::*;
use crate::api::*;

#[component]
pub fn PromQLBuilderPage() -> impl IntoView {
    let (available_metrics, set_available_metrics) = create_signal(Vec::<MetricDefinition>::new());
    let (query_components, set_query_components) = create_signal(Vec::<QueryComponent>::new());
    let (built_query, set_built_query) = create_signal(String::new());
    let (loading, set_loading) = create_signal(false);
    let (error_message, set_error_message) = create_signal(None::<String>);
    let (success_message, set_success_message) = create_signal(None::<String>);

    // Builder state
    let (show_functions_panel, set_show_functions_panel) = create_signal(true);
    let (show_operators_panel, set_show_operators_panel) = create_signal(true);
    let (query_validation, set_query_validation) = create_signal(None::<QueryValidationResult>);

    // Load metrics and functions
    let load_data = create_action(move |_: &()| async move {
        set_loading.set(true);
        set_error_message.set(None);

        match get_available_metrics().await {
            Ok(metrics) => set_available_metrics.set(metrics),
            Err(e) => set_error_message.set(Some(format!("Failed to load metrics: {}", e))),
        }

        set_loading.set(false);
    });

    // Validate query
    let validate_query = create_action(move |query: &String| {
        let query = query.clone();
        async move {
            if query.is_empty() {
                set_query_validation.set(None);
                return;
            }

            match validate_promql_query(&query).await {
                Ok(result) => set_query_validation.set(Some(result)),
                Err(e) => set_error_message.set(Some(format!("Validation failed: {}", e))),
            }
        }
    });

    // Add component to query
    let add_component = move |component: QueryComponent| {
        set_query_components.update(|components| {
            components.push(component);
        });
        rebuild_query();
    };

    // Remove component from query
    let remove_component = move |index: usize| {
        set_query_components.update(|components| {
            if index < components.len() {
                components.remove(index);
            }
        });
        rebuild_query();
    };

    // Update component
    let update_component = move |index: usize, component: QueryComponent| {
        set_query_components.update(|components| {
            if index < components.len() {
                components[index] = component;
            }
        });
        rebuild_query();
    };

    // Rebuild query from components
    let rebuild_query = move || {
        let components = query_components.get();
        if components.is_empty() {
            set_built_query.set(String::new());
            return;
        }

        let mut query = String::new();
        for (i, component) in components.iter().enumerate() {
            if i > 0 && !query.is_empty() {
                query.push(' ');
            }
            query.push_str(&component.to_promql());
        }

        set_built_query.set(query.clone());
        validate_query.dispatch(query);
    };

    // Predefined functions and operators
    let promql_functions = vec![
        PromQLFunction {
            name: "rate".to_string(),
            description: "Calculate per-second rate".to_string(),
            syntax: "rate(metric[range])".to_string(),
            category: "Rate Functions".to_string(),
            example: "rate(http_requests_total[5m])".to_string(),
        },
        PromQLFunction {
            name: "increase".to_string(),
            description: "Calculate increase over time range".to_string(),
            syntax: "increase(metric[range])".to_string(),
            category: "Rate Functions".to_string(),
            example: "increase(http_requests_total[1h])".to_string(),
        },
        PromQLFunction {
            name: "sum".to_string(),
            description: "Sum values across dimensions".to_string(),
            syntax: "sum(metric) by (labels)".to_string(),
            category: "Aggregation Functions".to_string(),
            example: "sum(cpu_usage) by (instance)".to_string(),
        },
        PromQLFunction {
            name: "avg".to_string(),
            description: "Average values across dimensions".to_string(),
            syntax: "avg(metric) by (labels)".to_string(),
            category: "Aggregation Functions".to_string(),
            example: "avg(memory_usage) by (job)".to_string(),
        },
        PromQLFunction {
            name: "max".to_string(),
            description: "Maximum value across dimensions".to_string(),
            syntax: "max(metric) by (labels)".to_string(),
            category: "Aggregation Functions".to_string(),
            example: "max(disk_usage) by (instance)".to_string(),
        },
        PromQLFunction {
            name: "min".to_string(),
            description: "Minimum value across dimensions".to_string(),
            syntax: "min(metric) by (labels)".to_string(),
            category: "Aggregation Functions".to_string(),
            example: "min(response_time) by (endpoint)".to_string(),
        },
        PromQLFunction {
            name: "histogram_quantile".to_string(),
            description: "Calculate quantile from histogram".to_string(),
            syntax: "histogram_quantile(quantile, metric)".to_string(),
            category: "Histogram Functions".to_string(),
            example: "histogram_quantile(0.95, http_request_duration_seconds_bucket)".to_string(),
        },
        PromQLFunction {
            name: "delta".to_string(),
            description: "Calculate difference between first and last value".to_string(),
            syntax: "delta(metric[range])".to_string(),
            category: "Rate Functions".to_string(),
            example: "delta(cpu_temp[10m])".to_string(),
        },
    ];

    let promql_operators = vec![
        "+", "-", "*", "/", "%", "^",
        "==", "!=", ">", "<", ">=", "<=",
        "and", "or", "unless",
    ];

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
        load_data.dispatch(());
    });

    view! {
        <div class="promql-builder-page">
            <div class="page-header">
                <div class="header-title">
                    <h1>"PromQL Query Builder"</h1>
                    <p class="header-subtitle">"Visual builder for Prometheus query language"</p>
                </div>
                <div class="header-actions">
                    <button
                        class="btn btn-secondary"
                        on:click=move |_| load_data.dispatch(())
                        disabled=loading
                    >
                        "Refresh"
                    </button>
                    <button
                        class="btn btn-outline"
                        on:click=move |_| {
                            set_query_components.set(Vec::new());
                            set_built_query.set(String::new());
                            set_query_validation.set(None);
                        }
                    >
                        "Clear Query"
                    </button>
                    <button
                        class="btn btn-primary"
                        disabled=built_query.get().is_empty()
                        on:click=move |_| {
                            let query = built_query.get();
                            if !query.is_empty() {
                                // Navigate to metrics explorer with the query
                                let navigate = leptos_router::use_navigate();
                                navigate(&format!("/metrics/explorer?query={}", urlencoding::encode(&query)), Default::default());
                            }
                        }
                    >
                        "Execute in Explorer"
                    </button>
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

            <div class="builder-layout">
                <div class="builder-sidebar">
                    // Metrics Panel
                    <div class="panel metrics-panel">
                        <h3>"Available Metrics"</h3>
                        <div class="metrics-list">
                            {move || if loading.get() {
                                view! { <div class="loading">"Loading metrics..."</div> }.into_view()
                            } else {
                                view! {
                                    {available_metrics.get().into_iter().map(|metric| {
                                        let metric_clone = metric.clone();
                                        let metric_name = metric.name.clone();
                                        let metric_type = metric.metric_type.clone();
                                        let metric_unit = metric.unit.clone();
                                        let metric_description = metric.description.clone();
                                        let metric_labels = metric.labels.clone();
                                        view! {
                                            <div class="metric-item">
                                                <div class="metric-header">
                                                    <span class="metric-name">{metric_name}</span>
                                                    <button
                                                        class="btn btn-xs btn-primary"
                                                        on:click=move |_| {
                                                            let component = QueryComponent::Metric {
                                                                name: metric_clone.name.clone(),
                                                                labels: std::collections::HashMap::new(),
                                                            };
                                                            add_component(component);
                                                        }
                                                    >
                                                        "Add"
                                                    </button>
                                                </div>
                                                <div class="metric-details">
                                                    <span class="metric-type">{metric_type}</span>
                                                    <span class="metric-unit">{metric_unit}</span>
                                                </div>
                                                <div class="metric-description">{metric_description}</div>
                                                {if !metric_labels.is_empty() {
                                                    view! {
                                                        <div class="metric-labels">
                                                            <span class="labels-title">"Labels:"</span>
                                                            {metric_labels.into_iter().map(|label| view! {
                                                                <span class="label-tag">{label}</span>
                                                            }).collect::<Vec<_>>()}
                                                        </div>
                                                    }.into_view()
                                                } else {
                                                    view! { <div></div> }.into_view()
                                                }}
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                }.into_view()
                            }}
                        </div>
                    </div>

                    // Functions Panel
                    {move || if show_functions_panel.get() {
                        view! {
                            <div class="panel functions-panel">
                                <h3>"PromQL Functions"</h3>
                                <div class="functions-list">
                                    {
                                        let functions_by_category = promql_functions.into_iter()
                                            .fold(std::collections::HashMap::new(), |mut acc, func| {
                                                acc.entry(func.category.clone()).or_insert_with(Vec::new).push(func);
                                                acc
                                            });

                                        view! {
                                            {functions_by_category.clone().into_iter().collect::<Vec<_>>().into_iter().map(|(category, functions)| {
                                                let cat_name = category.clone();
                                                view! {
                                                    <div class="function-category">
                                                        <h4>{cat_name}</h4>
                                                        {functions.into_iter().map(|func| {
                                                            let func_clone = func.clone();
                                                            let func_name = func.name.clone();
                                                            let func_syntax = func.syntax.clone();
                                                            let func_description = func.description.clone();
                                                            let func_example = func.example.clone();
                                                            view! {
                                                                <div class="function-item">
                                                                    <div class="function-header">
                                                                        <span class="function-name">{func_name}</span>
                                                                        <button
                                                                            class="btn btn-xs btn-secondary"
                                                                            on:click=move |_| {
                                                                                let component = QueryComponent::Function {
                                                                                    name: func_clone.name.clone(),
                                                                                    args: vec![],
                                                                                    modifiers: vec![],
                                                                                };
                                                                                add_component(component);
                                                                            }
                                                                        >
                                                                            "Add"
                                                                        </button>
                                                                    </div>
                                                                    <div class="function-syntax">
                                                                        <code>{func_syntax}</code>
                                                                    </div>
                                                                    <div class="function-description">{func_description}</div>
                                                                    <div class="function-example">
                                                                        <span class="example-label">"Example: "</span>
                                                                        <code>{func_example}</code>
                                                                    </div>
                                                                </div>
                                                            }
                                                        }).collect::<Vec<_>>()}
                                                    </div>
                                                }
                                            }).collect::<Vec<_>>()}
                                        }
                                    }
                                </div>
                            </div>
                        }.into_view()
                    } else {
                        view! { <div></div> }.into_view()
                    }}

                    // Operators Panel
                    {move || if show_operators_panel.get() {
                        view! {
                            <div class="panel operators-panel">
                                <h3>"Operators"</h3>
                                <div class="operators-grid">
                                    <div class="operator-group">
                                        <h4>"Arithmetic"</h4>
                                        {vec!["+", "-", "*", "/", "%", "^"].into_iter().map(|op| {
                                            let op_str = op.to_string();
                                            view! {
                                                <button
                                                    class="operator-btn"
                                                    on:click=move |_| {
                                                        let component = QueryComponent::Operator {
                                                            operator: op_str.clone(),
                                                        };
                                                        add_component(component);
                                                    }
                                                >
                                                    {op}
                                                </button>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>

                                    <div class="operator-group">
                                        <h4>"Comparison"</h4>
                                        {vec!["==", "!=", ">", "<", ">=", "<="].into_iter().map(|op| {
                                            let op_str = op.to_string();
                                            view! {
                                                <button
                                                    class="operator-btn"
                                                    on:click=move |_| {
                                                        let component = QueryComponent::Operator {
                                                            operator: op_str.clone(),
                                                        };
                                                        add_component(component);
                                                    }
                                                >
                                                    {op}
                                                </button>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>

                                    <div class="operator-group">
                                        <h4>"Logical"</h4>
                                        {vec!["and", "or", "unless"].into_iter().map(|op| {
                                            let op_str = op.to_string();
                                            view! {
                                                <button
                                                    class="operator-btn"
                                                    on:click=move |_| {
                                                        let component = QueryComponent::Operator {
                                                            operator: op_str.clone(),
                                                        };
                                                        add_component(component);
                                                    }
                                                >
                                                    {op}
                                                </button>
                                            }
                                        }).collect::<Vec<_>>()}
                                    </div>
                                </div>
                            </div>
                        }.into_view()
                    } else {
                        view! { <div></div> }.into_view()
                    }}
                </div>

                <div class="builder-main">
                    <div class="query-builder-canvas">
                        <div class="canvas-header">
                            <h3>"Query Components"</h3>
                            <div class="canvas-controls">
                                <label class="checkbox-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=show_functions_panel
                                        on:input=move |ev| set_show_functions_panel.set(event_target_checked(&ev))
                                    />
                                    " Show Functions"
                                </label>
                                <label class="checkbox-label">
                                    <input
                                        type="checkbox"
                                        prop:checked=show_operators_panel
                                        on:input=move |ev| set_show_operators_panel.set(event_target_checked(&ev))
                                    />
                                    " Show Operators"
                                </label>
                            </div>
                        </div>

                        {move || if query_components.get().is_empty() {
                            view! {
                                <div class="empty-canvas">
                                    <div class="empty-icon">"🔧"</div>
                                    <h3>"Build Your Query"</h3>
                                    <p>"Add metrics, functions, and operators from the sidebar to build your PromQL query"</p>
                                </div>
                            }.into_view()
                        } else {
                            view! {
                                <div class="query-components">
                                    {query_components.get().into_iter().enumerate().map(|(index, component)| {
                                        view! {
                                            <div class="component-item">
                                                <div class="component-content">
                                                    <QueryComponentEditor
                                                        component=component
                                                        on_update=move |updated_component| {
                                                            update_component(index, updated_component);
                                                        }
                                                    />
                                                </div>
                                                <button
                                                    class="component-remove"
                                                    on:click=move |_| remove_component(index)
                                                    title="Remove component"
                                                >
                                                    "x"
                                                </button>
                                            </div>
                                        }
                                    }).collect::<Vec<_>>()}
                                </div>
                            }.into_view()
                        }}

                        <div class="query-output">
                            <div class="output-header">
                                <h4>"Generated PromQL Query"</h4>
                                {move || if !built_query.get().is_empty() {
                                    view! {
                                        <button
                                            class="btn btn-xs btn-secondary"
                                            on:click=move |_| {
                                                if let Some(navigator) = web_sys::window()
                                                    .and_then(|w| w.navigator().clipboard()) {
                                                    let query = built_query.get();
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        let _ = navigator.write_text(&query);
                                                    });
                                                }
                                            }
                                        >
                                            "Copy"
                                        </button>
                                    }.into_view()
                                } else {
                                    view! { <div></div> }.into_view()
                                }}
                            </div>

                            <div class="query-display">
                                {move || if built_query.get().is_empty() {
                                    view! {
                                        <div class="empty-query">"No query built yet"</div>
                                    }.into_view()
                                } else {
                                    view! {
                                        <div class="query-text">
                                            <code>{built_query.get()}</code>
                                        </div>
                                    }.into_view()
                                }}
                            </div>

                            {move || if let Some(validation) = query_validation.get() {
                                view! {
                                    <div class="query-validation">
                                        <div class={format!("validation-result {}", if validation.valid { "valid" } else { "invalid" })}>
                                            <span class="validation-icon">
                                                {if validation.valid { "✅" } else { "❌" }}
                                            </span>
                                            <span class="validation-message">{&validation.message}</span>
                                        </div>
                                        {validation.suggestions.as_ref().map(|suggestions| {
                                            if !suggestions.is_empty() {
                                                let suggestions_clone = suggestions.clone();
                                                view! {
                                                    <div class="validation-suggestions">
                                                        <h5>"Suggestions:"</h5>
                                                        <ul>
                                                            {suggestions_clone.into_iter().map(|suggestion| view! {
                                                                <li>{suggestion}</li>
                                                            }).collect::<Vec<_>>()}
                                                        </ul>
                                                    </div>
                                                }.into_view()
                                            } else {
                                                view! { <div></div> }.into_view()
                                            }
                                        })}
                                    </div>
                                }.into_view()
                            } else {
                                view! { <div></div> }.into_view()
                            }}
                        </div>
                    </div>
                </div>
            </div>
        </div>
    }
}

#[component]
fn QueryComponentEditor<F>(
    component: QueryComponent,
    on_update: F,
) -> impl IntoView
where
    F: Fn(QueryComponent) + 'static,
{
    let (local_component, set_local_component) = create_signal(component.clone());

    // Update parent when local component changes
    create_effect(move |_| {
        on_update(local_component.get());
    });

    view! {
        <div class="query-component-editor">
            {match component {
                QueryComponent::Metric { name, labels } => {
                    let metric_name = name.clone();
                    let labels_clone = labels.clone();
                    let has_labels = !labels.is_empty();
                    view! {
                        <div class="metric-component">
                            <span class="component-type">"Metric:"</span>
                            <span class="component-name">{metric_name}</span>
                            {if has_labels {
                                view! {
                                    <div class="component-labels">
                                        <span>"{"</span>
                                        {labels_clone.into_iter().map(|(key, value)| {
                                            let k = key.clone();
                                            let v = value.clone();
                                            view! {
                                                <span class="label-pair">{k}"=\""{v}"\""</span>
                                            }
                                        }).collect::<Vec<_>>()}
                                        <span>"}"</span>
                                    </div>
                                }.into_view()
                            } else {
                                view! { <div></div> }.into_view()
                            }}
                        </div>
                    }.into_view()
                },
                QueryComponent::Function { name, args, modifiers: _ } => {
                    let func_name = name.clone();
                    let args_clone = args.clone();
                    let has_args = !args.is_empty();
                    view! {
                        <div class="function-component">
                            <span class="component-type">"Function:"</span>
                            <span class="component-name">{func_name}</span>
                            <span class="function-parens">"("</span>
                            {if has_args {
                                view! {
                                    <span class="function-args">
                                        {args_clone.into_iter().map(|arg| view! {
                                            <span class="function-arg">{arg}</span>
                                        }).collect::<Vec<_>>()}
                                    </span>
                                }.into_view()
                            } else {
                                view! { <span class="function-args-placeholder">"..."</span> }.into_view()
                            }}
                            <span class="function-parens">")"</span>
                        </div>
                    }.into_view()
                },
                QueryComponent::Operator { operator } => {
                    let op = operator.clone();
                    view! {
                        <div class="operator-component">
                            <span class="component-type">"Operator:"</span>
                            <span class="component-name operator-symbol">{op}</span>
                        </div>
                    }.into_view()
                },
            }}
        </div>
    }
}