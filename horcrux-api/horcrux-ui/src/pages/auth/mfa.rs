use leptos::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::JsCast;
use crate::api::*;

// WebAuthnChallenge is the only type not in api.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebAuthnChallenge {
    pub challenge: String,
    pub rp_id: String,
    pub rp_name: String,
    pub user_id: String,
    pub user_name: String,
    pub user_display_name: String,
}

#[component]
pub fn MfaManagementPage() -> impl IntoView {
    let (mfa_status, set_mfa_status) = create_signal(None::<MfaStatus>);
    let (loading, set_loading) = create_signal(true);
    let (error, set_error) = create_signal(None::<String>);

    // Setup states
    let (show_totp_setup, set_show_totp_setup) = create_signal(false);
    let (totp_setup_data, set_totp_setup_data) = create_signal(None::<TotpSetup>);
    let (totp_verification_code, set_totp_verification_code) = create_signal(String::new());
    let (show_backup_codes, set_show_backup_codes) = create_signal(false);
    let (backup_codes, set_backup_codes) = create_signal(Vec::<String>::new());

    // WebAuthn states
    let (show_webauthn_setup, set_show_webauthn_setup) = create_signal(false);
    let (webauthn_name, set_webauthn_name) = create_signal(String::new());
    let (webauthn_registering, set_webauthn_registering) = create_signal(false);

    // Admin view states
    let (current_tab, set_current_tab) = create_signal("personal".to_string()); // personal, policy, users
    let (enforcement_policy, set_enforcement_policy) = create_signal(None::<MfaEnforcementPolicy>);
    let (users_mfa_status, set_users_mfa_status) = create_signal(Vec::<UserMfaStatus>::new());

    // Modal states
    let (show_revoke_device, set_show_revoke_device) = create_signal(false);
    let (device_to_revoke, set_device_to_revoke) = create_signal(None::<TrustedDevice>);
    let (show_disable_mfa, set_show_disable_mfa) = create_signal(false);
    let (disable_mfa_code, set_disable_mfa_code) = create_signal(String::new());

    // Load MFA status on mount
    create_effect(move |_| {
        spawn_local(async move {
            set_loading.set(true);
            match get_mfa_status().await {
                Ok(status) => {
                    set_mfa_status.set(Some(status));
                    set_error.set(None);
                }
                Err(e) => {
                    set_error.set(Some(format!("Failed to load MFA status: {}", e)));
                }
            }
            set_loading.set(false);
        });
    });

    // Start TOTP setup
    let start_totp_setup = move || {
        spawn_local(async move {
            match initiate_totp_setup().await {
                Ok(setup) => {
                    set_totp_setup_data.set(Some(setup));
                    set_show_totp_setup.set(true);
                }
                Err(_) => {
                    // Show error
                }
            }
        });
    };

    // Verify and complete TOTP setup
    let complete_totp_setup = move || {
        let code = totp_verification_code.get();

        spawn_local(async move {
            match verify_totp_setup(code).await {
                Ok(codes) => {
                    set_backup_codes.set(codes);
                    set_show_totp_setup.set(false);
                    set_show_backup_codes.set(true);
                    set_totp_verification_code.set(String::new());

                    // Reload MFA status
                    if let Ok(status) = get_mfa_status().await {
                        set_mfa_status.set(Some(status));
                    }
                }
                Err(_) => {
                    // Show error
                }
            }
        });
    };

    // Start WebAuthn registration
    let start_webauthn_registration = move || {
        set_webauthn_registering.set(true);

        let name = webauthn_name.get();

        spawn_local(async move {
            match register_webauthn_credential(name).await {
                Ok(_) => {
                    set_show_webauthn_setup.set(false);
                    set_webauthn_name.set(String::new());

                    // Reload MFA status
                    if let Ok(status) = get_mfa_status().await {
                        set_mfa_status.set(Some(status));
                    }
                }
                Err(_) => {
                    // Show error
                }
            }
            set_webauthn_registering.set(false);
        });
    };

    // Regenerate backup codes
    let regenerate_backup_codes = move || {
        spawn_local(async move {
            match regenerate_mfa_backup_codes().await {
                Ok(codes) => {
                    set_backup_codes.set(codes);
                    set_show_backup_codes.set(true);

                    // Reload MFA status
                    if let Ok(status) = get_mfa_status().await {
                        set_mfa_status.set(Some(status));
                    }
                }
                Err(_) => {
                    // Show error
                }
            }
        });
    };

    // Revoke trusted device
    let revoke_device = move || {
        if let Some(device) = device_to_revoke.get() {
            spawn_local(async move {
                if let Ok(_) = revoke_trusted_device(device.id).await {
                    set_show_revoke_device.set(false);
                    set_device_to_revoke.set(None);

                    // Reload MFA status
                    if let Ok(status) = get_mfa_status().await {
                        set_mfa_status.set(Some(status));
                    }
                }
            });
        }
    };

    // Disable MFA method
    let disable_mfa_method = move |method_id: String| {
        spawn_local(async move {
            if let Ok(_) = disable_mfa(method_id, disable_mfa_code.get()).await {
                set_show_disable_mfa.set(false);
                set_disable_mfa_code.set(String::new());

                // Reload MFA status
                if let Ok(status) = get_mfa_status().await {
                    set_mfa_status.set(Some(status));
                }
            }
        });
    };

    // Set method as primary
    let set_primary_method = move |method_id: String| {
        spawn_local(async move {
            if let Ok(_) = set_primary_mfa_method(method_id).await {
                // Reload MFA status
                if let Ok(status) = get_mfa_status().await {
                    set_mfa_status.set(Some(status));
                }
            }
        });
    };

    view! {
        <div class="mfa-management-page">
            <div class="page-header">
                <h1 class="page-title">Multi-Factor Authentication</h1>
                <p class="page-description">
                    Secure your account with additional authentication methods
                </p>

                <div class="page-tabs">
                    <button
                        class={move || if current_tab.get() == "personal" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| set_current_tab.set("personal".to_string())
                    >
                        My MFA
                    </button>
                    <button
                        class={move || if current_tab.get() == "policy" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| {
                            set_current_tab.set("policy".to_string());
                            spawn_local(async move {
                                if let Ok(policy) = get_mfa_enforcement_policy().await {
                                    set_enforcement_policy.set(Some(policy));
                                }
                            });
                        }
                    >
                        Enforcement Policy
                    </button>
                    <button
                        class={move || if current_tab.get() == "users" { "tab-btn active" } else { "tab-btn" }}
                        on:click=move |_| {
                            set_current_tab.set("users".to_string());
                            spawn_local(async move {
                                if let Ok(users) = get_users_mfa_status().await {
                                    set_users_mfa_status.set(users);
                                }
                            });
                        }
                    >
                        User MFA Status
                    </button>
                </div>
            </div>

            {move || if loading.get() {
                view! {
                    <div class="loading-container">
                        <div class="spinner"></div>
                        <p>Loading MFA status...</p>
                    </div>
                }.into_view()
            } else if let Some(err) = error.get() {
                view! {
                    <div class="error-container">
                        <div class="error-message">
                            <h3>Error Loading MFA Status</h3>
                            <p>{err}</p>
                        </div>
                    </div>
                }.into_view()
            } else {
                match current_tab.get().as_str() {
                    "personal" => view! {
                        <div class="personal-mfa-view">
                            {mfa_status.get().map(|status| view! {
                                <div class="mfa-overview">
                                    // MFA Status Banner
                                    <div class={format!("mfa-status-banner {}", if status.enabled { "enabled" } else { "disabled" })}>
                                        <div class="status-icon">
                                            {if status.enabled { "🔐" } else { "🔓" }}
                                        </div>
                                        <div class="status-text">
                                            <h3>
                                                {if status.enabled { "MFA is Enabled" } else { "MFA is Not Enabled" }}
                                            </h3>
                                            <p>
                                                {if status.enabled {
                                                    format!("{} authentication method(s) configured", status.methods.len())
                                                } else {
                                                    "Enable MFA to add an extra layer of security to your account".to_string()
                                                }}
                                            </p>
                                        </div>
                                    </div>

                                    // Authentication Methods
                                    <div class="mfa-section">
                                        <div class="section-header">
                                            <h3>Authentication Methods</h3>
                                            <div class="add-method-buttons">
                                                <button
                                                    class="btn btn-primary"
                                                    on:click=move |_| start_totp_setup()
                                                >
                                                    Add Authenticator App
                                                </button>
                                                <button
                                                    class="btn btn-secondary"
                                                    on:click=move |_| set_show_webauthn_setup.set(true)
                                                >
                                                    Add Security Key
                                                </button>
                                            </div>
                                        </div>

                                        {if status.methods.is_empty() {
                                            view! {
                                                <div class="empty-methods">
                                                    <p>No authentication methods configured yet.</p>
                                                    <p>Add an authenticator app or security key to enable MFA.</p>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="methods-list">
                                                    {status.methods.iter().map(|method| {
                                                        let method_id = method.id.clone();
                                                        let method_id2 = method.id.clone();
                                                        view! {
                                                            <div class={format!("method-card method-type-{}", method.method_type)}>
                                                                <div class="method-icon">
                                                                    {match method.method_type.as_str() {
                                                                        "totp" => "📱",
                                                                        "webauthn" => "🔑",
                                                                        "sms" => "📞",
                                                                        "email" => "📧",
                                                                        _ => "🔐"
                                                                    }}
                                                                </div>
                                                                <div class="method-details">
                                                                    <h4>
                                                                        {method.name.clone()}
                                                                        {if method.is_primary {
                                                                            view! { <span class="primary-badge">Primary</span> }.into_view()
                                                                        } else {
                                                                            view! { <span></span> }.into_view()
                                                                        }}
                                                                    </h4>
                                                                    <div class="method-meta">
                                                                        <span>Added: {method.registered_at.clone()}</span>
                                                                        {method.last_used.as_ref().map(|last| view! {
                                                                            <span>Last used: {last.clone()}</span>
                                                                        })}
                                                                    </div>
                                                                </div>
                                                                <div class="method-actions">
                                                                    {if !method.is_primary {
                                                                        view! {
                                                                            <button
                                                                                class="btn btn-sm btn-secondary"
                                                                                on:click=move |_| {
                                                                                    set_primary_method(method_id.clone());
                                                                                }
                                                                            >
                                                                                Set Primary
                                                                            </button>
                                                                        }.into_view()
                                                                    } else {
                                                                        view! { <span></span> }.into_view()
                                                                    }}
                                                                    <button
                                                                        class="btn btn-sm btn-danger"
                                                                        on:click=move |_| {
                                                                            set_show_disable_mfa.set(true);
                                                                        }
                                                                    >
                                                                        Remove
                                                                    </button>
                                                                </div>
                                                            </div>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            }.into_view()
                                        }}
                                    </div>

                                    // Backup Codes
                                    <div class="mfa-section">
                                        <div class="section-header">
                                            <h3>Backup Codes</h3>
                                        </div>

                                        <div class="backup-codes-status">
                                            <div class="backup-info">
                                                <p>
                                                    <strong>{status.backup_codes_remaining.to_string()}</strong> backup codes remaining
                                                </p>
                                                <p class="help-text">
                                                    Use backup codes if you lose access to your authentication methods.
                                                </p>
                                            </div>
                                            <button
                                                class="btn btn-secondary"
                                                on:click=move |_| regenerate_backup_codes()
                                            >
                                                Regenerate Codes
                                            </button>
                                        </div>
                                    </div>

                                    // Trusted Devices
                                    <div class="mfa-section">
                                        <div class="section-header">
                                            <h3>Trusted Devices ({status.trusted_devices.len()})</h3>
                                        </div>

                                        {if status.trusted_devices.is_empty() {
                                            view! {
                                                <div class="empty-devices">
                                                    <p>No trusted devices.</p>
                                                    <p class="help-text">When you verify with MFA, you can mark a device as trusted to skip verification for 30 days.</p>
                                                </div>
                                            }.into_view()
                                        } else {
                                            view! {
                                                <div class="trusted-devices-list">
                                                    <table class="devices-table">
                                                        <thead>
                                                            <tr>
                                                                <th>Device</th>
                                                                <th>Browser / OS</th>
                                                                <th>IP Address</th>
                                                                <th>Trusted On</th>
                                                                <th>Expires</th>
                                                                <th>Actions</th>
                                                            </tr>
                                                        </thead>
                                                        <tbody>
                                                            {status.trusted_devices.iter().map(|device| {
                                                                let device_clone = device.clone();
                                                                view! {
                                                                    <tr>
                                                                        <td>
                                                                            <div class="device-info">
                                                                                <span class="device-icon">{
                                                                                    match device.device_type.as_str() {
                                                                                        "desktop" => "🖥️",
                                                                                        "mobile" => "📱",
                                                                                        "tablet" => "📱",
                                                                                        _ => "💻"
                                                                                    }
                                                                                }</span>
                                                                                <span class="device-name">{device.name.clone()}</span>
                                                                            </div>
                                                                        </td>
                                                                        <td>{format!("{} / {}", device.browser, device.os)}</td>
                                                                        <td>
                                                                            {device.ip_address.clone()}
                                                                            {device.location.as_ref().map(|loc| view! {
                                                                                <small class="location">{format!(" ({})", loc)}</small>
                                                                            })}
                                                                        </td>
                                                                        <td>{device.trusted_at.clone()}</td>
                                                                        <td>{device.expires_at.clone()}</td>
                                                                        <td>
                                                                            <button
                                                                                class="btn btn-sm btn-danger"
                                                                                on:click=move |_| {
                                                                                    set_device_to_revoke.set(Some(device_clone.clone()));
                                                                                    set_show_revoke_device.set(true);
                                                                                }
                                                                            >
                                                                                Revoke
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
                                </div>
                            })}
                        </div>
                    }.into_view(),

                    "policy" => view! {
                        <div class="policy-view">
                            {enforcement_policy.get().map(|policy| view! {
                                <MfaPolicyEditor policy=policy on_save=move |updated_policy| {
                                    spawn_local(async move {
                                        if let Ok(_) = update_mfa_enforcement_policy(updated_policy).await {
                                            if let Ok(policy) = get_mfa_enforcement_policy().await {
                                                set_enforcement_policy.set(Some(policy));
                                            }
                                        }
                                    });
                                }/>
                            }).unwrap_or_else(|| view! {
                                <div class="loading-container">
                                    <div class="spinner"></div>
                                    <p>Loading policy...</p>
                                </div>
                            }.into_view())}
                        </div>
                    }.into_view(),

                    "users" => view! {
                        <div class="users-mfa-view">
                            <UsersMfaStatusTable users=users_mfa_status.get() on_action=move |action, user_id| {
                                spawn_local(async move {
                                    match action.as_str() {
                                        "reset" => { let _ = reset_user_mfa(user_id).await; }
                                        "enforce" => { let _ = enforce_user_mfa(user_id).await; }
                                        _ => {}
                                    }
                                    // Reload users
                                    if let Ok(users) = get_users_mfa_status().await {
                                        set_users_mfa_status.set(users);
                                    }
                                });
                            }/>
                        </div>
                    }.into_view(),

                    _ => view! { <div></div> }.into_view()
                }
            }}

            // TOTP Setup Modal
            {move || if show_totp_setup.get() {
                totp_setup_data.get().map(|setup| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_totp_setup.set(false)>
                        <div class="modal-content totp-setup-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>Set Up Authenticator App</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_totp_setup.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="totp-setup-steps">
                                    <div class="setup-step">
                                        <h3>"Step 1: Scan QR Code"</h3>
                                        <p>"Scan this QR code with your authenticator app (Google Authenticator, Authy, etc.)"</p>
                                        <div class="qr-code">
                                            <img src={setup.qr_code_url.clone()} alt="QR Code" />
                                        </div>
                                        <details>
                                            <summary>"Cannot scan? Enter code manually"</summary>
                                            <div class="manual-code">
                                                <code>{setup.secret.clone()}</code>
                                            </div>
                                        </details>
                                    </div>

                                    <div class="setup-step">
                                        <h3>Step 2: Verify Code</h3>
                                        <p>Enter the 6-digit code from your authenticator app to verify setup</p>
                                        <div class="verification-input">
                                            <input
                                                type="text"
                                                placeholder="000000"
                                                maxlength="6"
                                                class="code-input"
                                                prop:value=totp_verification_code
                                                on:input=move |ev| {
                                                    set_totp_verification_code.set(event_target_value(&ev));
                                                }
                                            />
                                        </div>
                                    </div>
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_totp_setup.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| complete_totp_setup()
                                    disabled=move || totp_verification_code.get().len() != 6
                                >
                                    Verify and Enable
                                </button>
                            </div>
                        </div>
                    </div>
                })
            } else {
                None
            }}

            // Backup Codes Modal
            {move || if show_backup_codes.get() {
                view! {
                    <div class="modal-overlay">
                        <div class="modal-content backup-codes-modal">
                            <div class="modal-header">
                                <h2>Save Your Backup Codes</h2>
                            </div>

                            <div class="modal-body">
                                <div class="backup-codes-warning">
                                    <strong>Important:</strong> Save these codes in a secure location.
                                    Each code can only be used once. If you lose your authenticator,
                                    these codes are the only way to access your account.
                                </div>

                                <div class="backup-codes-grid">
                                    {backup_codes.get().iter().enumerate().map(|(i, code)| view! {
                                        <div class="backup-code">
                                            <span class="code-number">{(i + 1).to_string()}.</span>
                                            <code>{code.clone()}</code>
                                        </div>
                                    }).collect::<Vec<_>>()}
                                </div>

                                <div class="backup-codes-actions">
                                    <button
                                        class="btn btn-secondary"
                                        on:click=move |_| {
                                            // Copy to clipboard
                                            let codes_text = backup_codes.get().join("\n");
                                            let _ = web_sys::window()
                                                .unwrap()
                                                .navigator()
                                                .clipboard()
                                                .write_text(&codes_text);
                                        }
                                    >
                                        Copy to Clipboard
                                    </button>
                                    <button
                                        class="btn btn-secondary"
                                        on:click=move |_| {
                                            // Download as file
                                            let codes_text = backup_codes.get().join("\n");
                                            let blob = web_sys::Blob::new_with_str_sequence(&js_sys::Array::of1(&codes_text.into())).unwrap();
                                            let url = web_sys::Url::create_object_url_with_blob(&blob).unwrap();
                                            let a = web_sys::window().unwrap().document().unwrap().create_element("a").unwrap();
                                            a.set_attribute("href", &url).unwrap();
                                            a.set_attribute("download", "backup-codes.txt").unwrap();
                                            a.dyn_ref::<web_sys::HtmlElement>().unwrap().click();
                                        }
                                    >
                                        Download
                                    </button>
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| set_show_backup_codes.set(false)
                                >
                                    "I have saved these codes"
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // WebAuthn Setup Modal
            {move || if show_webauthn_setup.get() {
                view! {
                    <div class="modal-overlay" on:click=move |_| set_show_webauthn_setup.set(false)>
                        <div class="modal-content webauthn-setup-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>"Add Security Key"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_webauthn_setup.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <div class="webauthn-info">
                                    <p>"Use a physical security key (like YubiKey) or your device built-in authenticator for passwordless authentication."</p>
                                </div>

                                <div class="form-group">
                                    <label for="key-name">Key Name</label>
                                    <input
                                        type="text"
                                        id="key-name"
                                        class="form-control"
                                        placeholder="My YubiKey"
                                        prop:value=webauthn_name
                                        on:input=move |ev| {
                                            set_webauthn_name.set(event_target_value(&ev));
                                        }
                                    />
                                    <small class="form-text">A friendly name to identify this security key</small>
                                </div>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_webauthn_setup.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-primary"
                                    on:click=move |_| start_webauthn_registration()
                                    disabled=move || webauthn_name.get().is_empty() || webauthn_registering.get()
                                >
                                    {move || if webauthn_registering.get() {
                                        "Waiting for key..."
                                    } else {
                                        "Register Key"
                                    }}
                                </button>
                            </div>
                        </div>
                    </div>
                }.into_view()
            } else {
                view! { <div></div> }.into_view()
            }}

            // Revoke Device Modal
            {move || if show_revoke_device.get() {
                device_to_revoke.get().map(|device| view! {
                    <div class="modal-overlay" on:click=move |_| set_show_revoke_device.set(false)>
                        <div class="modal-content revoke-device-modal" on:click=|e| e.stop_propagation()>
                            <div class="modal-header">
                                <h2>"Revoke Trusted Device"</h2>
                                <button
                                    class="modal-close"
                                    on:click=move |_| set_show_revoke_device.set(false)
                                >
                                    "x"
                                </button>
                            </div>

                            <div class="modal-body">
                                <p>Are you sure you want to revoke trust for this device?</p>
                                <div class="device-summary">
                                    <strong>{device.name.clone()}</strong>
                                    <span>{format!("{} / {}", device.browser, device.os)}</span>
                                    <span>{device.ip_address.clone()}</span>
                                </div>
                                <p class="warning-text">
                                    You will need to verify with MFA the next time you sign in from this device.
                                </p>
                            </div>

                            <div class="modal-footer">
                                <button
                                    class="btn btn-secondary"
                                    on:click=move |_| set_show_revoke_device.set(false)
                                >
                                    Cancel
                                </button>
                                <button
                                    class="btn btn-danger"
                                    on:click=move |_| revoke_device()
                                >
                                    Revoke Trust
                                </button>
                            </div>
                        </div>
                    </div>
                })
            } else {
                None
            }}
        </div>
    }
}

// Helper component for MFA policy editing
#[component]
fn MfaPolicyEditor(policy: MfaEnforcementPolicy, on_save: impl Fn(MfaEnforcementPolicy) + 'static) -> impl IntoView {
    let (global_enforcement, set_global_enforcement) = create_signal(policy.global_enforcement);
    let (grace_period, set_grace_period) = create_signal(policy.grace_period_days);
    let (require_backup_codes, set_require_backup_codes) = create_signal(policy.require_backup_codes);
    let (max_trusted_devices, set_max_trusted_devices) = create_signal(policy.max_trusted_devices);
    let (trusted_device_expiry, set_trusted_device_expiry) = create_signal(policy.trusted_device_expiry_days);

    view! {
        <div class="mfa-policy-editor">
            <div class="policy-section">
                <h3>Enforcement Settings</h3>

                <div class="form-group">
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=global_enforcement
                            on:change=move |ev| {
                                set_global_enforcement.set(event_target_checked(&ev));
                            }
                        />
                        Enable global MFA enforcement
                    </label>
                    <small class="form-text">
                        When enabled, all users will be required to set up MFA
                    </small>
                </div>

                <div class="form-group">
                    <label for="grace-period">Grace Period (days)</label>
                    <input
                        type="number"
                        id="grace-period"
                        class="form-control"
                        min="0"
                        max="30"
                        prop:value=grace_period
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                set_grace_period.set(val);
                            }
                        }
                    />
                    <small class="form-text">
                        Number of days users have to set up MFA after enforcement is enabled
                    </small>
                </div>
            </div>

            <div class="policy-section">
                <h3>Security Settings</h3>

                <div class="form-group">
                    <label class="checkbox-label">
                        <input
                            type="checkbox"
                            prop:checked=require_backup_codes
                            on:change=move |ev| {
                                set_require_backup_codes.set(event_target_checked(&ev));
                            }
                        />
                        Require backup codes
                    </label>
                    <small class="form-text">
                        Users must generate and save backup codes when setting up MFA
                    </small>
                </div>

                <div class="form-group">
                    <label for="max-devices">Maximum trusted devices per user</label>
                    <input
                        type="number"
                        id="max-devices"
                        class="form-control"
                        min="0"
                        max="10"
                        prop:value=max_trusted_devices
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                set_max_trusted_devices.set(val);
                            }
                        }
                    />
                </div>

                <div class="form-group">
                    <label for="device-expiry">Trusted device expiry (days)</label>
                    <input
                        type="number"
                        id="device-expiry"
                        class="form-control"
                        min="1"
                        max="90"
                        prop:value=trusted_device_expiry
                        on:input=move |ev| {
                            if let Ok(val) = event_target_value(&ev).parse::<u32>() {
                                set_trusted_device_expiry.set(val);
                            }
                        }
                    />
                </div>
            </div>

            <div class="policy-actions">
                <button
                    class="btn btn-primary"
                    on:click=move |_| {
                        let updated = MfaEnforcementPolicy {
                            global_enforcement: global_enforcement.get(),
                            grace_period_days: grace_period.get(),
                            required_for_roles: vec![],
                            allowed_methods: vec!["totp".to_string(), "webauthn".to_string()],
                            require_backup_codes: require_backup_codes.get(),
                            max_trusted_devices: max_trusted_devices.get(),
                            trusted_device_expiry_days: trusted_device_expiry.get(),
                        };
                        on_save(updated);
                    }
                >
                    Save Policy
                </button>
            </div>
        </div>
    }
}

// Helper component for users MFA status table
// Note: UserMfaStatus is imported from crate::api::*
#[component]
fn UsersMfaStatusTable(
    users: Vec<UserMfaStatus>,
    on_action: impl Fn(String, String) + 'static + Clone
) -> impl IntoView {
    let (search_term, set_search_term) = create_signal(String::new());
    let (filter_status, set_filter_status) = create_signal("all".to_string());

    let filtered_users = create_memo(move |_| {
        users.clone()
            .into_iter()
            .filter(|user| {
                let search_match = if search_term.get().is_empty() {
                    true
                } else {
                    let term = search_term.get().to_lowercase();
                    user.username.to_lowercase().contains(&term) ||
                    user.email.to_lowercase().contains(&term)
                };

                let status_match = filter_status.get() == "all" || user.enforcement_status == filter_status.get();

                search_match && status_match
            })
            .collect::<Vec<_>>()
    });

    view! {
        <div class="users-mfa-status">
            <div class="table-filters">
                <input
                    type="text"
                    placeholder="Search users..."
                    class="search-input"
                    prop:value=search_term
                    on:input=move |ev| {
                        set_search_term.set(event_target_value(&ev));
                    }
                />
                <select
                    class="filter-select"
                    prop:value=filter_status
                    on:change=move |ev| {
                        set_filter_status.set(event_target_value(&ev));
                    }
                >
                    <option value="all">All Status</option>
                    <option value="compliant">Compliant</option>
                    <option value="grace_period">Grace Period</option>
                    <option value="non_compliant">Non-Compliant</option>
                </select>
            </div>

            <table class="users-table">
                <thead>
                    <tr>
                        <th>User</th>
                        <th>Email</th>
                        <th>MFA Status</th>
                        <th>Methods</th>
                        <th>Compliance</th>
                        <th>Actions</th>
                    </tr>
                </thead>
                <tbody>
                    {move || filtered_users.get().into_iter().map(|user| {
                        let user_id = user.user_id.clone();
                        let user_id2 = user.user_id.clone();
                        let on_action_clone = on_action.clone();
                        let on_action_clone2 = on_action.clone();
                        view! {
                            <tr>
                                <td>{user.username.clone()}</td>
                                <td>{user.email.clone()}</td>
                                <td>
                                    <span class={format!("mfa-status-badge {}", if user.mfa_enabled { "enabled" } else { "disabled" })}>
                                        {if user.mfa_enabled { "Enabled" } else { "Disabled" }}
                                    </span>
                                </td>
                                <td>{user.methods_count.to_string()}</td>
                                <td>
                                    <span class={format!("compliance-badge compliance-{}", user.enforcement_status)}>
                                        {user.enforcement_status.replace("_", " ")}
                                    </span>
                                </td>
                                <td>
                                    <button
                                        class="btn btn-sm btn-secondary"
                                        on:click=move |_| {
                                            on_action_clone("reset".to_string(), user_id.clone());
                                        }
                                    >
                                        Reset MFA
                                    </button>
                                    {if !user.mfa_enabled {
                                        view! {
                                            <button
                                                class="btn btn-sm btn-primary"
                                                on:click=move |_| {
                                                    on_action_clone2("enforce".to_string(), user_id2.clone());
                                                }
                                            >
                                                Enforce
                                            </button>
                                        }.into_view()
                                    } else {
                                        view! { <span></span> }.into_view()
                                    }}
                                </td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>
    }
}