# Horcrux API Documentation

## Interactive Documentation

Horcrux provides interactive API documentation through Swagger UI.

### Accessing the Documentation

Once the Horcrux API server is running, you can access the interactive documentation at:

**http://localhost:8006/api/docs**

### Features

- **Try It Out**: Test API endpoints directly from your browser
- **Authentication**: Configure authentication tokens in the UI
- **Request/Response Examples**: See example payloads for all endpoints
- **Schema Browser**: Explore data models and response structures
- **Filter**: Search for specific endpoints
- **Deep Linking**: Share direct links to specific endpoints

### OpenAPI Specification

The machine-readable OpenAPI 3.0 specification is available at:

- **YAML Format**: http://localhost:8006/api/openapi.yaml
- **JSON Info**: http://localhost:8006/api/openapi.json

### Using the Swagger UI

#### 1. Authentication

Before making API calls, you need to authenticate:

1. Click the **Authorize** button (🔒) at the top of the page
2. Choose your authentication method:
   - **Bearer Token (JWT)**: Login via `/api/auth/login` and use the returned token
   - **API Key**: Use an API key from `/api/users/{username}/api-keys`

**Example login to get Bearer token:**
```bash
curl -X POST http://localhost:8006/api/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"admin","password":"admin","realm":"local"}'
```

Copy the `token` from the response and paste it into the Bearer Token field.

#### 2. Testing Endpoints

1. Find the endpoint you want to test (e.g., `POST /api/vms`)
2. Click **Try it out**
3. Fill in the request body (example provided)
4. Click **Execute**
5. View the response below

#### 3. Exploring Schemas

Scroll down to the **Schemas** section to view all data models:
- `VmConfig` - Virtual machine configuration
- `Container` - Container definition
- `StoragePool` - Storage pool configuration
- `Error` - Error response format
- And 20+ more schemas...

### API Organization

The API is organized into logical groups:

| Tag | Description | Endpoint Count |
|-----|-------------|----------------|
| **Health** | API health check | 1 |
| **Authentication** | User login/logout | 5 |
| **VMs** | Virtual machine management | 6 |
| **VM Snapshots** | Snapshot operations | 6 |
| **VM Cloning** | Clone and template | 3 |
| **Containers** | Container management | 2 |
| **Storage** | Storage pools | 2 |
| **Backup & Restore** | Backup operations | 5 |
| **Replication** | ZFS replication | 6 |
| **Monitoring** | Metrics and stats | 3 |
| **Alerts** | Alert rules | 6 |
| **Firewall** | Security rules | 6 |
| **Networking** | SDN and CNI | 10 |
| **Clustering** | Multi-node management | 3 |
| **High Availability** | HA resources | 5 |
| **Migration** | Live migration | 2 |
| **Console** | VNC/SPICE access | 3 |
| **GPU** | GPU passthrough | 7 |
| **WebSocket** | Real-time events | 1 |
| **And more...** | | |

### Generating Client SDKs

You can generate client libraries in various languages from the OpenAPI spec:

#### Using OpenAPI Generator

```bash
# Install openapi-generator
npm install -g @openapitools/openapi-generator-cli

# Generate Python client
openapi-generator-cli generate \
  -i http://localhost:8006/api/openapi.yaml \
  -g python \
  -o ./python-client

# Generate Go client
openapi-generator-cli generate \
  -i http://localhost:8006/api/openapi.yaml \
  -g go \
  -o ./go-client

# Generate TypeScript/JavaScript client
openapi-generator-cli generate \
  -i http://localhost:8006/api/openapi.yaml \
  -g typescript-axios \
  -o ./ts-client
```

### API Versioning

The current API version is **v0.2.0**. The API follows semantic versioning:

- **Major version** (0.x.0): Breaking changes
- **Minor version** (x.2.x): New features, backward compatible
- **Patch version** (x.x.0): Bug fixes

All API endpoints are prefixed with `/api/` for clarity.

### Rate Limiting

The API includes rate limiting to prevent abuse:

- **Auth endpoints**: 5 requests per minute per IP
- **Other endpoints**: 100 requests per minute per user

Rate limit information is returned in response headers:
- `X-RateLimit-Limit`: Maximum requests allowed
- `X-RateLimit-Remaining`: Requests remaining in window
- `X-RateLimit-Reset`: Time when limit resets (Unix timestamp)

### Error Responses

All API errors follow a consistent format:

```json
{
  "status": 404,
  "error": "NOT_FOUND",
  "message": "Virtual machine 'vm-100' not found",
  "details": null,
  "request_id": "req-123456",
  "timestamp": "2025-10-11T10:30:00Z"
}
```

**Common HTTP Status Codes:**
- `200 OK` - Success
- `201 Created` - Resource created
- `202 Accepted` - Async operation started
- `400 Bad Request` - Invalid input
- `401 Unauthorized` - Authentication required
- `403 Forbidden` - Insufficient permissions
- `404 Not Found` - Resource doesn't exist
- `409 Conflict` - Resource already exists
- `422 Unprocessable Entity` - Validation failed
- `429 Too Many Requests` - Rate limit exceeded
- `500 Internal Server Error` - Server error
- `503 Service Unavailable` - Service temporarily unavailable

### WebSocket Events

The API supports WebSocket connections for real-time updates at:

**ws://localhost:8006/api/ws**

After connecting, send a subscription message:

```json
{
  "topics": ["vm:status", "vm:metrics", "alerts"]
}
```

**Available Topics:**
- `vm:status` - VM state changes
- `vm:metrics` - Real-time resource usage
- `vm:events` - VM lifecycle events
- `node:metrics` - Node-level statistics
- `backups` - Backup progress
- `migrations` - Migration status
- `alerts` - Alert notifications
- `notifications` - General system notifications

**Example Event:**
```json
{
  "type": "VmStatusChanged",
  "data": {
    "vm_id": "vm-100",
    "old_status": "stopped",
    "new_status": "running",
    "timestamp": "2025-10-11T10:30:00Z"
  }
}
```

### Examples

See the following resources for code examples:

- **Python Client**: [`docs/examples/python/`](../examples/python/)
- **Shell Scripts**: [`docs/examples/shell/`](../examples/shell/)
- **API Reference**: [`docs/API.md`](API.md) - 150+ endpoint details

### Support

- **Interactive Docs**: http://localhost:8006/api/docs
- **GitHub Issues**: https://github.com/CanuteTheGreat/horcrux/issues
- **Discussions**: https://github.com/CanuteTheGreat/horcrux/discussions

### Development

If you're developing against the API locally:

1. Start the server: `cargo run -p horcrux-api`
2. Open Swagger UI: http://localhost:8006/api/docs
3. Test endpoints interactively
4. Generate client code from `/api/openapi.yaml`

The OpenAPI spec is automatically served from the `docs/openapi.yaml` file, so any updates to that file will be reflected immediately (after server restart).

---

**Made with ❤️ for the Gentoo community**
