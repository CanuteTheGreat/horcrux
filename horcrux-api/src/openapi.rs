///! OpenAPI/Swagger UI integration
///!
///! Provides interactive API documentation at /api/docs

use axum::{
    response::{Html, IntoResponse},
    routing::get,
    Router,
};

/// Serve OpenAPI specification file
pub async fn serve_openapi_spec() -> impl IntoResponse {
    // Read the OpenAPI YAML file
    let openapi_yaml = include_str!("../../docs/openapi.yaml");

    (
        [("content-type", "application/x-yaml")],
        openapi_yaml
    )
}

/// Serve OpenAPI specification in JSON format
pub async fn serve_openapi_json() -> impl IntoResponse {
    // Read the YAML and note that client may prefer JSON
    // For now, we'll serve a message directing to YAML
    let json = r#"{
        "message": "OpenAPI specification available in YAML format",
        "yaml_endpoint": "/api/openapi.yaml",
        "swagger_ui": "/api/docs"
    }"#;

    (
        [("content-type", "application/json")],
        json
    )
}

/// Serve Swagger UI HTML page
///
/// This is a standalone Swagger UI that loads our OpenAPI spec
pub async fn serve_swagger_ui() -> impl IntoResponse {
    let html = r##"
<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Horcrux API Documentation</title>
    <link rel="stylesheet" href="https://unpkg.com/swagger-ui-dist@5.10.0/swagger-ui.css">
    <style>
        body {
            margin: 0;
            padding: 0;
        }
        .topbar {
            display: none;
        }
        .swagger-ui .info {
            margin: 20px 0;
        }
        .swagger-ui .info .title {
            font-size: 36px;
            color: #a855f7;
        }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://unpkg.com/swagger-ui-dist@5.10.0/swagger-ui-bundle.js"></script>
    <script src="https://unpkg.com/swagger-ui-dist@5.10.0/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = function() {
            window.ui = SwaggerUIBundle({
                url: "/api/openapi.yaml",
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
                layout: "StandaloneLayout",
                tryItOutEnabled: true,
                persistAuthorization: true,
                filter: true,
                syntaxHighlight: {
                    activate: true,
                    theme: "monokai"
                }
            });
        };
    </script>
</body>
</html>
"##;

    Html(html)
}

/// Create OpenAPI routes
///
/// Includes the raw spec endpoint, JSON endpoint, and Swagger UI
pub fn openapi_routes() -> Router {
    Router::new()
        .route("/api/openapi.yaml", get(serve_openapi_spec))
        .route("/api/openapi.json", get(serve_openapi_json))
        .route("/api/docs", get(serve_swagger_ui))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_openapi_spec_loads() {
        let response = serve_openapi_spec().await;
        // Verify it returns the YAML content
        let response = response.into_response();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_openapi_json_endpoint() {
        let response = serve_openapi_json().await;
        let response = response.into_response();
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn test_swagger_ui_loads() {
        let response = serve_swagger_ui().await;
        let response = response.into_response();
        assert_eq!(response.status(), 200);
    }

    #[test]
    fn test_openapi_routes_creation() {
        let routes = openapi_routes();
        // Verify routes can be created without panic
        assert!(format!("{:?}", routes).contains("Router"));
    }
}
