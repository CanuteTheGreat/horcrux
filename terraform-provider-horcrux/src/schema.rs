//! Terraform Schema Types
//!
//! Defines the schema types used for Terraform Plugin Protocol v6.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Attribute type for schema
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributeType {
    String,
    Number,
    Bool,
    List(Box<AttributeType>),
    Set(Box<AttributeType>),
    Map(Box<AttributeType>),
    Object(HashMap<String, AttributeType>),
}

/// Schema attribute
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaAttribute {
    #[serde(rename = "type")]
    pub attr_type: AttributeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub computed: bool,
    #[serde(default)]
    pub sensitive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
}

impl SchemaAttribute {
    pub fn string() -> Self {
        Self {
            attr_type: AttributeType::String,
            description: None,
            required: false,
            optional: false,
            computed: false,
            sensitive: false,
            default: None,
        }
    }

    pub fn number() -> Self {
        Self {
            attr_type: AttributeType::Number,
            description: None,
            required: false,
            optional: false,
            computed: false,
            sensitive: false,
            default: None,
        }
    }

    pub fn bool() -> Self {
        Self {
            attr_type: AttributeType::Bool,
            description: None,
            required: false,
            optional: false,
            computed: false,
            sensitive: false,
            default: None,
        }
    }

    pub fn list(element_type: AttributeType) -> Self {
        Self {
            attr_type: AttributeType::List(Box::new(element_type)),
            description: None,
            required: false,
            optional: false,
            computed: false,
            sensitive: false,
            default: None,
        }
    }

    pub fn map(element_type: AttributeType) -> Self {
        Self {
            attr_type: AttributeType::Map(Box::new(element_type)),
            description: None,
            required: false,
            optional: false,
            computed: false,
            sensitive: false,
            default: None,
        }
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self.optional = false;
        self
    }

    pub fn optional(mut self) -> Self {
        self.optional = true;
        self.required = false;
        self
    }

    pub fn computed(mut self) -> Self {
        self.computed = true;
        self
    }

    pub fn sensitive(mut self) -> Self {
        self.sensitive = true;
        self
    }

    pub fn with_default(mut self, value: serde_json::Value) -> Self {
        self.default = Some(value);
        self
    }
}

/// Block type for nested blocks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaBlock {
    pub attributes: HashMap<String, SchemaAttribute>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub blocks: HashMap<String, NestedBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl SchemaBlock {
    pub fn new() -> Self {
        Self {
            attributes: HashMap::new(),
            blocks: HashMap::new(),
            description: None,
        }
    }

    pub fn with_attribute(mut self, name: &str, attr: SchemaAttribute) -> Self {
        self.attributes.insert(name.to_string(), attr);
        self
    }

    pub fn with_block(mut self, name: &str, block: NestedBlock) -> Self {
        self.blocks.insert(name.to_string(), block);
        self
    }

    pub fn with_description(mut self, desc: &str) -> Self {
        self.description = Some(desc.to_string());
        self
    }
}

impl Default for SchemaBlock {
    fn default() -> Self {
        Self::new()
    }
}

/// Nested block type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NestedBlock {
    pub nesting_mode: NestingMode,
    pub block: SchemaBlock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_items: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_items: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NestingMode {
    Single,
    List,
    Set,
    Map,
}

/// Resource schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSchema {
    pub version: i64,
    pub block: SchemaBlock,
}

impl ResourceSchema {
    pub fn new(version: i64, block: SchemaBlock) -> Self {
        Self { version, block }
    }
}

/// Provider schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSchema {
    pub provider: SchemaBlock,
    pub resource_schemas: HashMap<String, ResourceSchema>,
    pub data_source_schemas: HashMap<String, ResourceSchema>,
}

impl ProviderSchema {
    pub fn new(provider: SchemaBlock) -> Self {
        Self {
            provider,
            resource_schemas: HashMap::new(),
            data_source_schemas: HashMap::new(),
        }
    }

    pub fn with_resource(mut self, name: &str, schema: ResourceSchema) -> Self {
        self.resource_schemas.insert(name.to_string(), schema);
        self
    }

    #[allow(dead_code)]
    pub fn with_data_source(mut self, name: &str, schema: ResourceSchema) -> Self {
        self.data_source_schemas.insert(name.to_string(), schema);
        self
    }
}

// ============================================================================
// Terraform Plugin Protocol Messages
// ============================================================================

/// JSON-RPC request
#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: i64,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// JSON-RPC error
#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl RpcResponse {
    pub fn success(id: i64, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: i64, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }
}

/// Diagnostic severity
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DiagnosticSeverity {
    Invalid,
    Error,
    Warning,
}

/// Diagnostic message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attribute: Option<Vec<String>>,
}

impl Diagnostic {
    pub fn error(summary: &str) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            summary: summary.to_string(),
            detail: None,
            attribute: None,
        }
    }

    #[allow(dead_code)]
    pub fn warning(summary: &str) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            summary: summary.to_string(),
            detail: None,
            attribute: None,
        }
    }

    #[allow(dead_code)]
    pub fn with_detail(mut self, detail: &str) -> Self {
        self.detail = Some(detail.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn with_attribute(mut self, path: Vec<String>) -> Self {
        self.attribute = Some(path);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schema_attribute_builder() {
        let attr = SchemaAttribute::string()
            .with_description("Test attribute")
            .required()
            .sensitive();

        assert!(attr.required);
        assert!(attr.sensitive);
        assert_eq!(attr.description, Some("Test attribute".to_string()));
    }

    #[test]
    fn test_schema_block_builder() {
        let block = SchemaBlock::new()
            .with_attribute("name", SchemaAttribute::string().required())
            .with_attribute("count", SchemaAttribute::number().optional())
            .with_description("Test block");

        assert!(block.attributes.contains_key("name"));
        assert!(block.attributes.contains_key("count"));
        assert_eq!(block.description, Some("Test block".to_string()));
    }

    #[test]
    fn test_rpc_response_success() {
        let response = RpcResponse::success(1, serde_json::json!({"status": "ok"}));
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[test]
    fn test_rpc_response_error() {
        let response = RpcResponse::error(1, -32600, "Invalid request");
        assert!(response.result.is_none());
        assert!(response.error.is_some());
        assert_eq!(response.error.unwrap().code, -32600);
    }
}
