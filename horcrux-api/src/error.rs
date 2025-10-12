///! Standardized error handling for API responses
///!
///! Provides consistent JSON error responses across all API endpoints

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::error;

/// Standard API error response format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// HTTP status code
    pub status: u16,

    /// Error code for programmatic handling
    pub error: String,

    /// Human-readable error message
    pub message: String,

    /// Optional detailed error information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,

    /// Request ID for tracking (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,

    /// Timestamp when error occurred
    pub timestamp: String,
}

impl ErrorResponse {
    pub fn new(status: u16, error: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            status,
            error: error.into(),
            message: message.into(),
            details: None,
            request_id: None,
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    #[allow(dead_code)]
    pub fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }
}

/// API error types with standardized responses
#[derive(Debug)]
#[allow(dead_code)]
pub enum ApiError {
    /// 500 Internal Server Error
    Internal(String),

    /// 404 Not Found
    NotFound(String),

    /// 401 Unauthorized
    AuthenticationFailed,

    /// 403 Forbidden
    Forbidden(String),

    /// 400 Bad Request
    BadRequest(String),

    /// 409 Conflict
    Conflict(String),

    /// 422 Unprocessable Entity
    ValidationError(String),

    /// 503 Service Unavailable
    ServiceUnavailable(String),

    /// 429 Too Many Requests
    RateLimited(String),
}

impl ApiError {
    /// Convert error to ErrorResponse
    pub fn to_error_response(&self) -> ErrorResponse {
        match self {
            ApiError::Internal(msg) => {
                error!("Internal API error: {}", msg);
                ErrorResponse::new(
                    500,
                    "INTERNAL_ERROR",
                    "An internal server error occurred",
                )
                .with_details(msg)
            }
            ApiError::NotFound(msg) => {
                ErrorResponse::new(404, "NOT_FOUND", msg)
            }
            ApiError::AuthenticationFailed => {
                ErrorResponse::new(
                    401,
                    "AUTHENTICATION_FAILED",
                    "Authentication credentials are invalid or missing",
                )
            }
            ApiError::Forbidden(msg) => {
                ErrorResponse::new(403, "FORBIDDEN", msg)
            }
            ApiError::BadRequest(msg) => {
                ErrorResponse::new(400, "BAD_REQUEST", msg)
            }
            ApiError::Conflict(msg) => {
                ErrorResponse::new(409, "CONFLICT", msg)
            }
            ApiError::ValidationError(msg) => {
                ErrorResponse::new(422, "VALIDATION_ERROR", msg)
            }
            ApiError::ServiceUnavailable(msg) => {
                ErrorResponse::new(503, "SERVICE_UNAVAILABLE", msg)
            }
            ApiError::RateLimited(msg) => {
                ErrorResponse::new(429, "RATE_LIMITED", msg)
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let error_response = self.to_error_response();
        let status_code = StatusCode::from_u16(error_response.status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

        (status_code, Json(error_response)).into_response()
    }
}

impl From<horcrux_common::Error> for ApiError {
    fn from(err: horcrux_common::Error) -> Self {
        match err {
            horcrux_common::Error::VmNotFound(id) => {
                ApiError::NotFound(format!("Virtual machine '{}' not found", id))
            }
            horcrux_common::Error::ContainerNotFound(id) => {
                ApiError::NotFound(format!("Container '{}' not found", id))
            }
            horcrux_common::Error::InvalidConfig(msg) => {
                ApiError::ValidationError(msg)
            }
            horcrux_common::Error::Validation(msg) => {
                ApiError::ValidationError(msg)
            }
            horcrux_common::Error::AuthenticationFailed => {
                ApiError::AuthenticationFailed
            }
            horcrux_common::Error::InvalidSession => {
                ApiError::AuthenticationFailed
            }
            horcrux_common::Error::System(msg) => {
                ApiError::Internal(msg)
            }
            horcrux_common::Error::Io(e) => {
                ApiError::Internal(format!("I/O error: {}", e))
            }
        }
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        ApiError::Internal(format!("I/O error: {}", err))
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(err: serde_json::Error) -> Self {
        ApiError::BadRequest(format!("Invalid JSON: {}", err))
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(err: sqlx::Error) -> Self {
        error!("Database error: {}", err);
        ApiError::Internal("Database error occurred".to_string())
    }
}

/// Helper functions for creating common errors
impl ApiError {
    #[allow(dead_code)]
    pub fn vm_not_found(id: impl Into<String>) -> Self {
        ApiError::NotFound(format!("Virtual machine '{}' not found", id.into()))
    }

    #[allow(dead_code)]
    pub fn container_not_found(id: impl Into<String>) -> Self {
        ApiError::NotFound(format!("Container '{}' not found", id.into()))
    }

    #[allow(dead_code)]
    pub fn permission_denied(resource: impl Into<String>) -> Self {
        ApiError::Forbidden(format!("Permission denied for resource: {}", resource.into()))
    }

    #[allow(dead_code)]
    pub fn invalid_input(field: impl Into<String>, reason: impl Into<String>) -> Self {
        ApiError::ValidationError(format!("{}: {}", field.into(), reason.into()))
    }

    #[allow(dead_code)]
    pub fn already_exists(resource: impl Into<String>) -> Self {
        ApiError::Conflict(format!("{} already exists", resource.into()))
    }

    #[allow(dead_code)]
    pub fn service_error(service: impl Into<String>, reason: impl Into<String>) -> Self {
        ApiError::ServiceUnavailable(format!("{} is unavailable: {}", service.into(), reason.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_response_creation() {
        let error = ErrorResponse::new(404, "NOT_FOUND", "Resource not found");
        assert_eq!(error.status, 404);
        assert_eq!(error.error, "NOT_FOUND");
        assert_eq!(error.message, "Resource not found");
        assert!(error.details.is_none());
    }

    #[test]
    fn test_error_response_with_details() {
        let error = ErrorResponse::new(500, "INTERNAL_ERROR", "Something went wrong")
            .with_details("Stack trace here")
            .with_request_id("req-123");

        assert_eq!(error.status, 500);
        assert_eq!(error.details, Some("Stack trace here".to_string()));
        assert_eq!(error.request_id, Some("req-123".to_string()));
    }

    #[test]
    fn test_api_error_conversion() {
        let err = horcrux_common::Error::VmNotFound("vm-100".to_string());
        let api_err: ApiError = err.into();

        let response = api_err.to_error_response();
        assert_eq!(response.status, 404);
        assert_eq!(response.error, "NOT_FOUND");
    }

    #[test]
    fn test_helper_functions() {
        let err = ApiError::vm_not_found("vm-100");
        let response = err.to_error_response();
        assert_eq!(response.status, 404);
        assert!(response.message.contains("vm-100"));

        let err = ApiError::permission_denied("/api/vms/100");
        let response = err.to_error_response();
        assert_eq!(response.status, 403);

        let err = ApiError::invalid_input("memory", "must be greater than 0");
        let response = err.to_error_response();
        assert_eq!(response.status, 422);
    }

    #[test]
    fn test_json_serialization() {
        let error = ErrorResponse::new(400, "BAD_REQUEST", "Invalid input");
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("BAD_REQUEST"));
        assert!(json.contains("Invalid input"));
    }
}
