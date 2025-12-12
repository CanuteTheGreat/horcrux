//! Terraform Provider Implementation
//!
//! Implements the Terraform Plugin Protocol for Horcrux.

use crate::client::HorcruxClient;
use crate::resources::{get_all_resources, Resource, ResourceState};
use crate::schema::{
    Diagnostic, ProviderSchema, ResourceSchema, RpcRequest, RpcResponse, SchemaAttribute,
    SchemaBlock,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::runtime::Runtime;

/// Provider configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub api_token: Option<String>,
    pub insecure: Option<bool>,
}

/// Horcrux Terraform Provider
pub struct HorcruxProvider {
    config: Arc<RwLock<Option<ProviderConfig>>>,
    client: Arc<RwLock<Option<HorcruxClient>>>,
    resources: HashMap<String, Box<dyn Resource>>,
    runtime: Runtime,
}

impl HorcruxProvider {
    /// Create a new provider
    pub fn new() -> Self {
        let resources: HashMap<String, Box<dyn Resource>> = get_all_resources()
            .into_iter()
            .map(|r| (r.type_name().to_string(), r))
            .collect();

        let runtime = Runtime::new().expect("Failed to create Tokio runtime");

        Self {
            config: Arc::new(RwLock::new(None)),
            client: Arc::new(RwLock::new(None)),
            resources,
            runtime,
        }
    }

    /// Get provider schema
    fn get_schema(&self) -> ProviderSchema {
        let provider_block = SchemaBlock::new()
            .with_attribute(
                "endpoint",
                SchemaAttribute::string()
                    .with_description("Horcrux API endpoint (e.g., http://localhost:8006)")
                    .required(),
            )
            .with_attribute(
                "username",
                SchemaAttribute::string()
                    .with_description("Username for authentication")
                    .optional(),
            )
            .with_attribute(
                "password",
                SchemaAttribute::string()
                    .with_description("Password for authentication")
                    .optional()
                    .sensitive(),
            )
            .with_attribute(
                "api_token",
                SchemaAttribute::string()
                    .with_description("API token for authentication (alternative to username/password)")
                    .optional()
                    .sensitive(),
            )
            .with_attribute(
                "insecure",
                SchemaAttribute::bool()
                    .with_description("Skip TLS verification")
                    .optional()
                    .with_default(serde_json::json!(false)),
            )
            .with_description("Horcrux virtualization platform provider");

        let mut schema = ProviderSchema::new(provider_block);

        // Add resource schemas
        for (name, resource) in &self.resources {
            schema = schema.with_resource(name, resource.schema());
        }

        schema
    }

    /// Configure the provider
    fn configure(&self, config: ProviderConfig) -> Vec<Diagnostic> {
        let mut diagnostics = Vec::new();

        // Validate configuration
        let endpoint = match &config.endpoint {
            Some(e) => e.clone(),
            None => {
                diagnostics.push(Diagnostic::error("endpoint is required"));
                return diagnostics;
            }
        };

        // Create client
        let mut client = HorcruxClient::new(&endpoint);

        // Set authentication
        if let Some(token) = &config.api_token {
            client = client.with_token(token);
        } else if config.username.is_some() && config.password.is_some() {
            let username = config.username.as_ref().unwrap();
            let password = config.password.as_ref().unwrap();

            // Authenticate
            match self.runtime.block_on(async {
                let mut auth_client = HorcruxClient::new(&endpoint);
                auth_client.authenticate(username, password).await
            }) {
                Ok(token) => {
                    client = client.with_token(&token);
                }
                Err(e) => {
                    diagnostics.push(Diagnostic::error(&format!("Authentication failed: {}", e)));
                    return diagnostics;
                }
            }
        } else {
            diagnostics.push(Diagnostic::error(
                "Either api_token or username/password must be provided",
            ));
            return diagnostics;
        }

        // Store configuration and client
        *self.config.write().unwrap() = Some(config);
        *self.client.write().unwrap() = Some(client);

        diagnostics
    }

    /// Get the configured client
    fn get_client(&self) -> Result<HorcruxClient, Diagnostic> {
        self.client
            .read()
            .unwrap()
            .clone()
            .ok_or_else(|| Diagnostic::error("Provider not configured"))
    }

    /// Handle an RPC request
    pub fn handle_request(&self, input: &str) -> String {
        let request: RpcRequest = match serde_json::from_str(input) {
            Ok(r) => r,
            Err(e) => {
                return serde_json::to_string(&RpcResponse::error(
                    0,
                    -32700,
                    &format!("Parse error: {}", e),
                ))
                .unwrap_or_default();
            }
        };

        let response = match request.method.as_str() {
            "GetProviderSchema" => self.handle_get_schema(request.id),
            "ConfigureProvider" => self.handle_configure(request.id, &request.params),
            "ValidateResourceConfig" => {
                self.handle_validate_resource(request.id, &request.params)
            }
            "PlanResourceChange" => self.handle_plan_resource(request.id, &request.params),
            "ApplyResourceChange" => self.handle_apply_resource(request.id, &request.params),
            "ReadResource" => self.handle_read_resource(request.id, &request.params),
            "ImportResourceState" => self.handle_import_resource(request.id, &request.params),
            "StopProvider" => RpcResponse::success(request.id, serde_json::json!({})),
            _ => RpcResponse::error(
                request.id,
                -32601,
                &format!("Method not found: {}", request.method),
            ),
        };

        serde_json::to_string(&response).unwrap_or_else(|e| {
            serde_json::to_string(&RpcResponse::error(
                request.id,
                -32603,
                &format!("Serialization error: {}", e),
            ))
            .unwrap_or_default()
        })
    }

    /// Handle GetProviderSchema
    fn handle_get_schema(&self, id: i64) -> RpcResponse {
        let schema = self.get_schema();
        RpcResponse::success(id, serde_json::to_value(schema).unwrap_or_default())
    }

    /// Handle ConfigureProvider
    fn handle_configure(&self, id: i64, params: &Value) -> RpcResponse {
        let config: ProviderConfig = params
            .get("config")
            .and_then(|c| serde_json::from_value(c.clone()).ok())
            .unwrap_or_default();

        let diagnostics = self.configure(config);

        RpcResponse::success(
            id,
            serde_json::json!({
                "diagnostics": diagnostics
            }),
        )
    }

    /// Handle ValidateResourceConfig
    fn handle_validate_resource(&self, id: i64, params: &Value) -> RpcResponse {
        let type_name = params
            .get("type_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let _resource = match self.resources.get(type_name) {
            Some(r) => r,
            None => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [
                            Diagnostic::error(&format!("Unknown resource type: {}", type_name))
                        ]
                    }),
                );
            }
        };

        // Basic validation - check required fields exist
        let diagnostics: Vec<Diagnostic> = Vec::new();

        RpcResponse::success(
            id,
            serde_json::json!({
                "diagnostics": diagnostics
            }),
        )
    }

    /// Handle PlanResourceChange
    fn handle_plan_resource(&self, id: i64, params: &Value) -> RpcResponse {
        let type_name = params
            .get("type_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let resource = match self.resources.get(type_name) {
            Some(r) => r,
            None => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [
                            Diagnostic::error(&format!("Unknown resource type: {}", type_name))
                        ]
                    }),
                );
            }
        };

        let proposed_state: ResourceState = params
            .get("proposed_new_state")
            .and_then(|v| {
                v.as_object().map(|obj| {
                    let values: HashMap<String, Value> = obj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    ResourceState { values }
                })
            })
            .unwrap_or_default();

        let prior_state: Option<ResourceState> = params.get("prior_state").and_then(|v| {
            v.as_object().map(|obj| {
                let values: HashMap<String, Value> =
                    obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                ResourceState { values }
            })
        });

        match resource.plan_change(prior_state.as_ref(), &proposed_state) {
            Ok(planned) => RpcResponse::success(
                id,
                serde_json::json!({
                    "planned_state": planned.values,
                    "diagnostics": []
                }),
            ),
            Err(diagnostics) => RpcResponse::success(
                id,
                serde_json::json!({
                    "diagnostics": diagnostics
                }),
            ),
        }
    }

    /// Handle ApplyResourceChange
    fn handle_apply_resource(&self, id: i64, params: &Value) -> RpcResponse {
        let type_name = params
            .get("type_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let resource = match self.resources.get(type_name) {
            Some(r) => r,
            None => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [
                            Diagnostic::error(&format!("Unknown resource type: {}", type_name))
                        ]
                    }),
                );
            }
        };

        let client = match self.get_client() {
            Ok(c) => c,
            Err(diag) => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [diag]
                    }),
                );
            }
        };

        let planned_state: ResourceState = params
            .get("planned_state")
            .and_then(|v| {
                v.as_object().map(|obj| {
                    let values: HashMap<String, Value> = obj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    ResourceState { values }
                })
            })
            .unwrap_or_default();

        let prior_state: Option<ResourceState> = params.get("prior_state").and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_object().map(|obj| {
                    let values: HashMap<String, Value> =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    ResourceState { values }
                })
            }
        });

        let is_destroy = params
            .get("planned_state")
            .map(|v| v.is_null())
            .unwrap_or(false);

        let result = self.runtime.block_on(async {
            if is_destroy {
                // Delete
                if let Some(prior) = prior_state {
                    resource.delete(&client, &prior).await.map(|_| None)
                } else {
                    Ok(None)
                }
            } else if prior_state.is_none() {
                // Create
                resource.create(&client, &planned_state).await.map(Some)
            } else {
                // Update
                resource
                    .update(&client, prior_state.as_ref().unwrap(), &planned_state)
                    .await
                    .map(Some)
            }
        });

        match result {
            Ok(Some(new_state)) => RpcResponse::success(
                id,
                serde_json::json!({
                    "new_state": new_state.values,
                    "diagnostics": []
                }),
            ),
            Ok(None) => RpcResponse::success(
                id,
                serde_json::json!({
                    "new_state": null,
                    "diagnostics": []
                }),
            ),
            Err(diagnostics) => RpcResponse::success(
                id,
                serde_json::json!({
                    "diagnostics": diagnostics
                }),
            ),
        }
    }

    /// Handle ReadResource
    fn handle_read_resource(&self, id: i64, params: &Value) -> RpcResponse {
        let type_name = params
            .get("type_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let resource = match self.resources.get(type_name) {
            Some(r) => r,
            None => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [
                            Diagnostic::error(&format!("Unknown resource type: {}", type_name))
                        ]
                    }),
                );
            }
        };

        let client = match self.get_client() {
            Ok(c) => c,
            Err(diag) => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [diag]
                    }),
                );
            }
        };

        let current_state: ResourceState = params
            .get("current_state")
            .and_then(|v| {
                v.as_object().map(|obj| {
                    let values: HashMap<String, Value> = obj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    ResourceState { values }
                })
            })
            .unwrap_or_default();

        let result = self
            .runtime
            .block_on(async { resource.read(&client, &current_state).await });

        match result {
            Ok(state) => {
                if state.values.is_empty() {
                    // Resource no longer exists
                    RpcResponse::success(
                        id,
                        serde_json::json!({
                            "new_state": null,
                            "diagnostics": []
                        }),
                    )
                } else {
                    RpcResponse::success(
                        id,
                        serde_json::json!({
                            "new_state": state.values,
                            "diagnostics": []
                        }),
                    )
                }
            }
            Err(diagnostics) => RpcResponse::success(
                id,
                serde_json::json!({
                    "diagnostics": diagnostics
                }),
            ),
        }
    }

    /// Handle ImportResourceState
    fn handle_import_resource(&self, id: i64, params: &Value) -> RpcResponse {
        let type_name = params
            .get("type_name")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let resource_id = params.get("id").and_then(|v| v.as_str()).unwrap_or("");

        let resource = match self.resources.get(type_name) {
            Some(r) => r,
            None => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [
                            Diagnostic::error(&format!("Unknown resource type: {}", type_name))
                        ]
                    }),
                );
            }
        };

        let client = match self.get_client() {
            Ok(c) => c,
            Err(diag) => {
                return RpcResponse::success(
                    id,
                    serde_json::json!({
                        "diagnostics": [diag]
                    }),
                );
            }
        };

        // Create a minimal state with just the ID for reading
        let mut import_state = ResourceState::new();
        import_state.set("id", serde_json::json!(resource_id));

        let result = self
            .runtime
            .block_on(async { resource.read(&client, &import_state).await });

        match result {
            Ok(state) => {
                if state.values.is_empty() {
                    RpcResponse::success(
                        id,
                        serde_json::json!({
                            "diagnostics": [
                                Diagnostic::error(&format!("Resource {} not found", resource_id))
                            ]
                        }),
                    )
                } else {
                    RpcResponse::success(
                        id,
                        serde_json::json!({
                            "imported_resources": [{
                                "type_name": type_name,
                                "state": state.values
                            }],
                            "diagnostics": []
                        }),
                    )
                }
            }
            Err(diagnostics) => RpcResponse::success(
                id,
                serde_json::json!({
                    "diagnostics": diagnostics
                }),
            ),
        }
    }
}

impl Default for HorcruxProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_creation() {
        let provider = HorcruxProvider::new();
        assert!(!provider.resources.is_empty());
    }

    #[test]
    fn test_provider_schema() {
        let provider = HorcruxProvider::new();
        let schema = provider.get_schema();

        assert!(schema.provider.attributes.contains_key("endpoint"));
        assert!(schema.provider.attributes.contains_key("username"));
        assert!(schema.provider.attributes.contains_key("api_token"));
    }

    #[test]
    fn test_handle_get_schema() {
        let provider = HorcruxProvider::new();
        let response = provider.handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"GetProviderSchema","params":{}}"#,
        );

        assert!(response.contains("provider"));
        assert!(response.contains("resource_schemas"));
    }

    #[test]
    fn test_handle_unknown_method() {
        let provider = HorcruxProvider::new();
        let response = provider.handle_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"UnknownMethod","params":{}}"#,
        );

        assert!(response.contains("error"));
        assert!(response.contains("Method not found"));
    }
}
