# API Error Responses

## Overview

All Horcrux API endpoints return standardized JSON error responses to ensure consistent client-side error handling. This document describes the error response format and common error codes.

## Error Response Format

All error responses follow this JSON structure:

```json
{
  "status": 404,
  "error": "NOT_FOUND",
  "message": "Virtual machine 'vm-100' not found",
  "details": "Optional detailed error information",
  "request_id": "req-abc123",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

### Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `status` | number | Yes | HTTP status code (e.g., 404, 500) |
| `error` | string | Yes | Machine-readable error code (e.g., "NOT_FOUND") |
| `message` | string | Yes | Human-readable error message |
| `details` | string | No | Additional error details (stack traces in dev mode) |
| `request_id` | string | No | Unique request identifier for tracking |
| `timestamp` | string | Yes | ISO 8601 timestamp when error occurred |

## HTTP Status Codes

### Success Codes (2xx)
- `200 OK` - Request succeeded
- `201 Created` - Resource created successfully
- `204 No Content` - Request succeeded with no response body

### Client Error Codes (4xx)

#### 400 Bad Request
**Error Code**: `BAD_REQUEST`

Invalid request format or parameters.

**Example**:
```json
{
  "status": 400,
  "error": "BAD_REQUEST",
  "message": "Invalid JSON: expected value at line 1 column 1",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- Malformed JSON
- Missing required parameters
- Invalid parameter types

---

#### 401 Unauthorized
**Error Code**: `AUTHENTICATION_FAILED`

Authentication credentials are invalid or missing.

**Example**:
```json
{
  "status": 401,
  "error": "AUTHENTICATION_FAILED",
  "message": "Authentication credentials are invalid or missing",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- Missing Authorization header
- Invalid JWT token
- Expired session
- Invalid API key

---

#### 403 Forbidden
**Error Code**: `FORBIDDEN`

User lacks permission to access the resource.

**Example**:
```json
{
  "status": 403,
  "error": "FORBIDDEN",
  "message": "Permission denied for resource: /api/vms/100",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- Insufficient RBAC privileges
- Resource belongs to another user
- IP address not whitelisted

---

#### 404 Not Found
**Error Code**: `NOT_FOUND`

Requested resource does not exist.

**Example**:
```json
{
  "status": 404,
  "error": "NOT_FOUND",
  "message": "Virtual machine 'vm-100' not found",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- VM/container ID doesn't exist
- Endpoint path is incorrect
- Resource was deleted

---

#### 409 Conflict
**Error Code**: `CONFLICT`

Request conflicts with current state.

**Example**:
```json
{
  "status": 409,
  "error": "CONFLICT",
  "message": "Virtual machine 'vm-100' already exists",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- Resource already exists
- VM is already running
- Concurrent modification conflict

---

#### 422 Unprocessable Entity
**Error Code**: `VALIDATION_ERROR`

Request is well-formed but semantically invalid.

**Example**:
```json
{
  "status": 422,
  "error": "VALIDATION_ERROR",
  "message": "memory: must be greater than 0",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- Invalid configuration values
- Business logic validation failure
- Constraint violation

---

#### 429 Too Many Requests
**Error Code**: `RATE_LIMITED`

Rate limit exceeded.

**Example**:
```json
{
  "status": 429,
  "error": "RATE_LIMITED",
  "message": "Rate limit exceeded: 100 requests per minute",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Headers**:
- `X-RateLimit-Limit`: Maximum requests allowed
- `X-RateLimit-Remaining`: Requests remaining
- `X-RateLimit-Reset`: Time when limit resets (Unix timestamp)

---

### Server Error Codes (5xx)

#### 500 Internal Server Error
**Error Code**: `INTERNAL_ERROR`

Unexpected server error occurred.

**Example**:
```json
{
  "status": 500,
  "error": "INTERNAL_ERROR",
  "message": "An internal server error occurred",
  "details": "Database connection failed",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- Database connection failure
- Unhandled exception
- System resource exhaustion

---

#### 503 Service Unavailable
**Error Code**: `SERVICE_UNAVAILABLE`

Service is temporarily unavailable.

**Example**:
```json
{
  "status": 503,
  "error": "SERVICE_UNAVAILABLE",
  "message": "QEMU service is unavailable: connection refused",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

**Common Causes**:
- QEMU/KVM unavailable
- Database maintenance
- Cluster node offline

---

## Error Code Reference

| Error Code | HTTP Status | Description |
|------------|-------------|-------------|
| `BAD_REQUEST` | 400 | Invalid request format or parameters |
| `AUTHENTICATION_FAILED` | 401 | Invalid or missing credentials |
| `FORBIDDEN` | 403 | Insufficient permissions |
| `NOT_FOUND` | 404 | Resource does not exist |
| `CONFLICT` | 409 | Request conflicts with current state |
| `VALIDATION_ERROR` | 422 | Semantic validation failure |
| `RATE_LIMITED` | 429 | Rate limit exceeded |
| `INTERNAL_ERROR` | 500 | Unexpected server error |
| `SERVICE_UNAVAILABLE` | 503 | Service temporarily unavailable |

## Client-Side Error Handling

### JavaScript/TypeScript Example

```typescript
interface ApiError {
  status: number;
  error: string;
  message: string;
  details?: string;
  request_id?: string;
  timestamp: string;
}

async function handleApiCall(url: string) {
  try {
    const response = await fetch(url, {
      headers: {
        'Authorization': `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
    });

    if (!response.ok) {
      const error: ApiError = await response.json();

      switch (error.error) {
        case 'NOT_FOUND':
          console.error('Resource not found:', error.message);
          break;
        case 'AUTHENTICATION_FAILED':
          // Redirect to login
          window.location.href = '/login';
          break;
        case 'FORBIDDEN':
          console.error('Permission denied:', error.message);
          break;
        case 'VALIDATION_ERROR':
          console.error('Validation error:', error.message);
          break;
        case 'RATE_LIMITED':
          console.error('Rate limited, retry after reset');
          break;
        default:
          console.error('API error:', error.message);
      }

      throw error;
    }

    return await response.json();
  } catch (err) {
    // Network error or parsing error
    console.error('Request failed:', err);
    throw err;
  }
}
```

### Python Example

```python
import requests
from typing import Optional

class ApiError(Exception):
    def __init__(self, status: int, error: str, message: str,
                 details: Optional[str] = None, request_id: Optional[str] = None):
        self.status = status
        self.error = error
        self.message = message
        self.details = details
        self.request_id = request_id
        super().__init__(message)

def api_request(url: str, token: str):
    headers = {
        'Authorization': f'Bearer {token}',
        'Content-Type': 'application/json',
    }

    response = requests.get(url, headers=headers)

    if not response.ok:
        error_data = response.json()
        raise ApiError(
            status=error_data['status'],
            error=error_data['error'],
            message=error_data['message'],
            details=error_data.get('details'),
            request_id=error_data.get('request_id'),
        )

    return response.json()

# Usage
try:
    vms = api_request('http://localhost:8006/api/vms', token)
except ApiError as e:
    if e.error == 'AUTHENTICATION_FAILED':
        print('Authentication failed, please login')
    elif e.error == 'NOT_FOUND':
        print(f'Resource not found: {e.message}')
    else:
        print(f'API error ({e.status}): {e.message}')
```

### Rust Example

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ApiError {
    pub status: u16,
    pub error: String,
    pub message: String,
    pub details: Option<String>,
    pub request_id: Option<String>,
    pub timestamp: String,
}

async fn api_request(url: &str, token: &str) -> Result<serde_json::Value, ApiError> {
    let client = reqwest::Client::new();

    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await
        .unwrap();

    if !response.status().is_success() {
        let error: ApiError = response.json().await.unwrap();
        return Err(error);
    }

    Ok(response.json().await.unwrap())
}

// Usage
match api_request("http://localhost:8006/api/vms", token).await {
    Ok(data) => println!("Success: {:?}", data),
    Err(e) => match e.error.as_str() {
        "NOT_FOUND" => eprintln!("Resource not found: {}", e.message),
        "AUTHENTICATION_FAILED" => eprintln!("Auth failed, please login"),
        _ => eprintln!("API error ({}): {}", e.status, e.message),
    }
}
```

## Best Practices

### For API Consumers

1. **Always check HTTP status code** before parsing response
2. **Use error.error field** for programmatic error handling
3. **Display error.message** to users
4. **Log error.details** for debugging (if present)
5. **Include error.request_id** in bug reports

### For API Developers

1. **Use appropriate HTTP status codes** (don't use 500 for validation errors)
2. **Provide clear, actionable error messages** (not "Something went wrong")
3. **Include details in development** but hide sensitive info in production
4. **Log all errors** with request ID for tracing
5. **Document all error codes** and when they occur

## Testing Error Responses

### Using cURL

```bash
# Test 404 error
curl -i http://localhost:8006/api/vms/nonexistent \
  -H "Authorization: Bearer $TOKEN"

# Test validation error
curl -X POST http://localhost:8006/api/vms \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"name":"test","memory":-1}'

# Test authentication error
curl -i http://localhost:8006/api/vms \
  -H "Authorization: Bearer invalid-token"
```

### Expected Responses

```bash
# 404 Response
HTTP/1.1 404 Not Found
Content-Type: application/json

{
  "status": 404,
  "error": "NOT_FOUND",
  "message": "Virtual machine 'nonexistent' not found",
  "timestamp": "2025-10-09T10:30:45Z"
}

# 422 Validation Error
HTTP/1.1 422 Unprocessable Entity
Content-Type: application/json

{
  "status": 422,
  "error": "VALIDATION_ERROR",
  "message": "memory: must be greater than 0",
  "timestamp": "2025-10-09T10:30:45Z"
}

# 401 Auth Error
HTTP/1.1 401 Unauthorized
Content-Type: application/json

{
  "status": 401,
  "error": "AUTHENTICATION_FAILED",
  "message": "Authentication credentials are invalid or missing",
  "timestamp": "2025-10-09T10:30:45Z"
}
```

---

**Last Updated**: 2025-10-09
**Version**: 1.0
