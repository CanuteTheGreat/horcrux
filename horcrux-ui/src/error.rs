///! User-friendly error handling for the UI
///!
///! Provides error message formatting and display components

use leptos::*;
use serde::{Deserialize, Serialize};

/// API error response format (matches backend)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub status: u16,
    pub error: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub timestamp: String,
}

impl ApiError {
    /// Get user-friendly error message
    pub fn user_message(&self) -> String {
        match self.error.as_str() {
            "NOT_FOUND" => self.format_not_found(),
            "AUTHENTICATION_FAILED" => "Your session has expired. Please log in again.".to_string(),
            "FORBIDDEN" => self.format_forbidden(),
            "VALIDATION_ERROR" => self.format_validation(),
            "CONFLICT" => self.format_conflict(),
            "RATE_LIMITED" => "Too many requests. Please wait a moment and try again.".to_string(),
            "SERVICE_UNAVAILABLE" => "The service is temporarily unavailable. Please try again later.".to_string(),
            "INTERNAL_ERROR" => "An unexpected error occurred. Please try again or contact support.".to_string(),
            _ => self.message.clone(),
        }
    }

    fn format_not_found(&self) -> String {
        // Make NOT_FOUND errors more user-friendly
        if self.message.contains("Virtual machine") {
            "The virtual machine you're looking for doesn't exist.".to_string()
        } else if self.message.contains("Container") {
            "The container you're looking for doesn't exist.".to_string()
        } else {
            "The requested resource was not found.".to_string()
        }
    }

    fn format_forbidden(&self) -> String {
        "You don't have permission to perform this action.".to_string()
    }

    fn format_validation(&self) -> String {
        // Improve validation error messages
        let msg = &self.message;
        if msg.contains("memory") {
            format!("Invalid memory configuration: {}", msg)
        } else if msg.contains("cpus") || msg.contains("CPU") {
            format!("Invalid CPU configuration: {}", msg)
        } else if msg.contains("disk") {
            format!("Invalid disk configuration: {}", msg)
        } else {
            format!("Invalid input: {}", msg)
        }
    }

    fn format_conflict(&self) -> String {
        let msg = &self.message;
        if msg.contains("already exists") {
            format!("A resource with this name already exists. Please choose a different name.")
        } else if msg.contains("already running") {
            "This virtual machine is already running.".to_string()
        } else {
            format!("Operation conflict: {}", msg)
        }
    }

    /// Get severity level for UI styling
    pub fn severity(&self) -> ErrorSeverity {
        match self.status {
            400..=499 => ErrorSeverity::Warning,
            500..=599 => ErrorSeverity::Error,
            _ => ErrorSeverity::Info,
        }
    }

    /// Get icon for error type
    pub fn icon(&self) -> &'static str {
        match self.error.as_str() {
            "NOT_FOUND" => "🔍",
            "AUTHENTICATION_FAILED" => "🔐",
            "FORBIDDEN" => "🚫",
            "VALIDATION_ERROR" => "⚠️",
            "CONFLICT" => "⚡",
            "RATE_LIMITED" => "⏱️",
            "SERVICE_UNAVAILABLE" => "🔧",
            _ => "❌",
        }
    }

    /// Should show retry button
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.error.as_str(),
            "RATE_LIMITED" | "SERVICE_UNAVAILABLE" | "INTERNAL_ERROR"
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ErrorSeverity {
    Info,
    Warning,
    Error,
}

impl ErrorSeverity {
    pub fn class(&self) -> &'static str {
        match self {
            ErrorSeverity::Info => "alert-info",
            ErrorSeverity::Warning => "alert-warning",
            ErrorSeverity::Error => "alert-error",
        }
    }
}

/// Error display component
#[component]
pub fn ErrorAlert(
    /// Error to display
    error: ApiError,
    /// Callback for retry button
    #[prop(optional)]
    on_retry: Option<Callback<()>>,
    /// Callback for dismiss button
    #[prop(optional)]
    on_dismiss: Option<Callback<()>>,
) -> impl IntoView {
    let severity = error.severity();
    let icon = error.icon();
    let message = error.user_message();
    let show_retry = error.is_retryable() && on_retry.is_some();

    view! {
        <div class={format!("alert {}", severity.class())}>
            <div class="alert-icon">{icon}</div>
            <div class="alert-content">
                <div class="alert-message">{message}</div>
                {error.details.as_ref().map(|details| view! {
                    <details class="alert-details">
                        <summary>"Technical details"</summary>
                        <code>{details.clone()}</code>
                    </details>
                })}
            </div>
            <div class="alert-actions">
                {show_retry.then(|| {
                    let on_retry = on_retry.unwrap();
                    view! {
                        <button
                            class="btn-secondary btn-sm"
                            on:click=move |_| on_retry.call(())
                        >
                            "Retry"
                        </button>
                    }
                })}
                {on_dismiss.map(|on_dismiss| view! {
                    <button
                        class="btn-ghost btn-sm"
                        on:click=move |_| on_dismiss.call(())
                    >
                        "✕"
                    </button>
                })}
            </div>
        </div>
    }
}

/// Toast notification component for temporary errors
#[component]
pub fn ErrorToast(
    /// Error to display
    error: ApiError,
    /// Auto-dismiss after milliseconds (default: 5000)
    #[prop(default = 5000)]
    duration_ms: u32,
) -> impl IntoView {
    let (visible, set_visible) = create_signal(true);
    let severity = error.severity();
    let icon = error.icon();
    let message = error.user_message();

    // Auto-dismiss after duration
    if duration_ms > 0 {
        set_timeout(
            move || set_visible.set(false),
            std::time::Duration::from_millis(duration_ms as u64),
        );
    }

    view! {
        <Show when=move || visible.get()>
            <div class={format!("toast {}", severity.class())}>
                <div class="toast-icon">{icon}</div>
                <div class="toast-message">{message}</div>
                <button
                    class="toast-close"
                    on:click=move |_| set_visible.set(false)
                >
                    "✕"
                </button>
            </div>
        </Show>
    }
}

/// Inline error message for form fields
#[component]
pub fn FieldError(
    /// Error message
    message: String,
) -> impl IntoView {
    view! {
        <div class="field-error">
            <span class="field-error-icon">"⚠️"</span>
            <span class="field-error-message">{message}</span>
        </div>
    }
}

/// Loading error state component
#[component]
pub fn LoadingError(
    /// Error that occurred
    error: ApiError,
    /// Callback for retry button
    #[prop(optional)]
    on_retry: Option<Callback<()>>,
) -> impl IntoView {
    let message = error.user_message();
    let icon = error.icon();

    view! {
        <div class="loading-error">
            <div class="loading-error-icon">{icon}</div>
            <h3>"Failed to Load"</h3>
            <p>{message}</p>
            {on_retry.map(|on_retry| view! {
                <button
                    class="btn-primary"
                    on:click=move |_| on_retry.call(())
                >
                    "Try Again"
                </button>
            })}
        </div>
    }
}

/// Empty state component (when no data available)
#[component]
pub fn EmptyState(
    /// Icon to display
    #[prop(default = "📦")]
    icon: &'static str,
    /// Title
    title: String,
    /// Description
    description: String,
    /// Action button text
    #[prop(optional)]
    action_text: Option<String>,
    /// Action callback
    #[prop(optional)]
    on_action: Option<Callback<()>>,
) -> impl IntoView {
    view! {
        <div class="empty-state">
            <div class="empty-state-icon">{icon}</div>
            <h3>{title}</h3>
            <p>{description}</p>
            {action_text.zip(on_action).map(|(text, on_action)| view! {
                <button
                    class="btn-primary"
                    on:click=move |_| on_action.call(())
                >
                    {text}
                </button>
            })}
        </div>
    }
}

/// Helper to extract error from reqwasm response
pub async fn extract_api_error(response: reqwasm::http::Response) -> ApiError {
    // Try to parse as ApiError JSON
    if let Ok(error) = response.json::<ApiError>().await {
        return error;
    }

    // Fallback to generic error
    ApiError {
        status: response.status(),
        error: "UNKNOWN_ERROR".to_string(),
        message: format!("Request failed with status {}", response.status()),
        details: None,
        request_id: None,
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_message_not_found() {
        let error = ApiError {
            status: 404,
            error: "NOT_FOUND".to_string(),
            message: "Virtual machine 'vm-100' not found".to_string(),
            details: None,
            request_id: None,
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        assert_eq!(
            error.user_message(),
            "The virtual machine you're looking for doesn't exist."
        );
    }

    #[test]
    fn test_user_message_auth_failed() {
        let error = ApiError {
            status: 401,
            error: "AUTHENTICATION_FAILED".to_string(),
            message: "Authentication failed".to_string(),
            details: None,
            request_id: None,
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        assert_eq!(
            error.user_message(),
            "Your session has expired. Please log in again."
        );
    }

    #[test]
    fn test_severity() {
        let error_400 = ApiError {
            status: 400,
            error: "BAD_REQUEST".to_string(),
            message: "Bad request".to_string(),
            details: None,
            request_id: None,
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        assert_eq!(error_400.severity(), ErrorSeverity::Warning);

        let error_500 = ApiError {
            status: 500,
            error: "INTERNAL_ERROR".to_string(),
            message: "Internal error".to_string(),
            details: None,
            request_id: None,
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        assert_eq!(error_500.severity(), ErrorSeverity::Error);
    }

    #[test]
    fn test_is_retryable() {
        let retryable = ApiError {
            status: 503,
            error: "SERVICE_UNAVAILABLE".to_string(),
            message: "Service unavailable".to_string(),
            details: None,
            request_id: None,
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        assert!(retryable.is_retryable());

        let not_retryable = ApiError {
            status: 404,
            error: "NOT_FOUND".to_string(),
            message: "Not found".to_string(),
            details: None,
            request_id: None,
            timestamp: "2025-10-09T10:30:45Z".to_string(),
        };

        assert!(!not_retryable.is_retryable());
    }
}
