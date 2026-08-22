use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use serde_json::Value;
use ys_agent_core::{CoreError, CoreResult, Sensitivity, SideEffect, Tool, ToolRisk, ToolSpec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceToolPolicy {
    pub allowed_tool_names: Option<BTreeSet<String>>,
    pub max_risk: ToolRisk,
    pub max_input_sensitivity: Sensitivity,
    pub max_output_sensitivity: Sensitivity,
    pub max_preview_sensitivity: Sensitivity,
}

impl Default for WorkspaceToolPolicy {
    fn default() -> Self {
        Self {
            allowed_tool_names: None,
            max_risk: ToolRisk::High,
            max_input_sensitivity: Sensitivity::Restricted,
            max_output_sensitivity: Sensitivity::Restricted,
            max_preview_sensitivity: Sensitivity::Internal,
        }
    }
}

impl WorkspaceToolPolicy {
    pub fn allows(&self, spec: &ToolSpec) -> bool {
        let name_allowed = self
            .allowed_tool_names
            .as_ref()
            .is_none_or(|names| names.contains(&spec.name));

        name_allowed
            && risk_rank(spec.risk) <= risk_rank(self.max_risk)
            && spec.input_sensitivity <= self.max_input_sensitivity
            && spec.output_sensitivity <= self.max_output_sensitivity
    }
}

fn risk_rank(risk: ToolRisk) -> u8 {
    match risk {
        ToolRisk::Low => 0,
        ToolRisk::Medium => 1,
        ToolRisk::High => 2,
    }
}

pub struct ToolCatalog {
    tools: BTreeMap<String, Arc<dyn Tool>>,
    policy: WorkspaceToolPolicy,
}

impl Default for ToolCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCatalog {
    pub fn new() -> Self {
        Self::with_policy(WorkspaceToolPolicy::default())
    }

    pub fn with_policy(policy: WorkspaceToolPolicy) -> Self {
        Self {
            tools: BTreeMap::new(),
            policy,
        }
    }

    pub fn policy(&self) -> &WorkspaceToolPolicy {
        &self.policy
    }

    pub fn register<T>(&mut self, tool: T) -> CoreResult<()>
    where
        T: Tool + 'static,
    {
        self.register_arc(Arc::new(tool))
    }

    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) -> CoreResult<()> {
        let spec = tool.spec();
        self.validate_registration(&spec)?;
        self.tools.insert(spec.name, tool);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    fn validate_registration(&self, spec: &ToolSpec) -> CoreResult<()> {
        if self.tools.contains_key(&spec.name) {
            return Err(CoreError::DuplicateTool(spec.name.clone()));
        }
        if !is_stable_name(&spec.name) {
            return Err(CoreError::validation(
                "invalid_tool_name",
                "tool name must use lower_snake_case ASCII characters",
            ));
        }
        if !is_three_part_version(&spec.version) {
            return Err(CoreError::validation(
                "invalid_tool_version",
                "tool version must contain three numeric parts",
            ));
        }
        if spec.timeout_ms == 0 {
            return Err(CoreError::validation(
                "invalid_tool_timeout",
                "tool timeout must be greater than zero",
            ));
        }
        if spec.side_effect != SideEffect::None {
            return Err(CoreError::UnsupportedCapability(
                "v0.2 registers read-only tools only".to_owned(),
            ));
        }
        if spec.required_permissions.len() != 1 || spec.required_permissions[0] != "data_query" {
            return Err(CoreError::validation(
                "invalid_tool_permissions",
                "v0.2 tools require exactly data_query",
            ));
        }
        if !self.policy.allows(spec) {
            return Err(CoreError::validation(
                "tool_policy_incompatible",
                "tool metadata exceeds Workspace policy",
            ));
        }

        validate_schema_shape(&spec.input_schema, "input")?;
        validate_schema_shape(&spec.output_schema, "output")?;
        Ok(())
    }
}

fn is_stable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn is_three_part_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(crate) fn validate_schema_shape(schema: &Value, label: &str) -> CoreResult<()> {
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return Err(CoreError::validation(
            "invalid_tool_schema",
            format!("tool {label} schema root must have type object"),
        ));
    }
    validate_schema_node(schema, "$", label)
}

fn validate_schema_node(schema: &Value, path: &str, label: &str) -> CoreResult<()> {
    let object = schema.as_object().ok_or_else(|| {
        CoreError::validation(
            "invalid_tool_schema",
            format!("tool {label} schema at {path} must be an object"),
        )
    })?;

    if let Some(values) = object.get("enum")
        && values.as_array().is_none_or(Vec::is_empty)
    {
        return Err(CoreError::validation(
            "invalid_tool_schema",
            format!("tool {label} schema enum at {path} must be a non-empty array"),
        ));
    }

    let Some(schema_type) = object.get("type") else {
        if object.contains_key("const") || object.contains_key("enum") {
            return Ok(());
        }
        return Err(CoreError::validation(
            "invalid_tool_schema",
            format!("tool {label} schema at {path} needs type, const, or enum"),
        ));
    };
    let schema_type = schema_type.as_str().ok_or_else(|| {
        CoreError::validation(
            "invalid_tool_schema",
            format!("tool {label} schema type at {path} must be a string"),
        )
    })?;

    match schema_type {
        "object" => {
            let properties = match object.get("properties") {
                Some(value) => value.as_object().cloned().ok_or_else(|| {
                    CoreError::validation(
                        "invalid_tool_schema",
                        format!("tool {label} properties at {path} must be an object"),
                    )
                })?,
                None => serde_json::Map::new(),
            };
            for (name, child) in &properties {
                validate_schema_node(child, &format!("{path}.{name}"), label)?;
            }

            if let Some(required) = object.get("required") {
                let required = required.as_array().ok_or_else(|| {
                    CoreError::validation(
                        "invalid_tool_schema",
                        format!("tool {label} required at {path} must be an array"),
                    )
                })?;
                for name in required {
                    let name = name.as_str().ok_or_else(|| {
                        CoreError::validation(
                            "invalid_tool_schema",
                            format!("tool {label} required names at {path} must be strings"),
                        )
                    })?;
                    if !properties.contains_key(name) {
                        return Err(CoreError::validation(
                            "invalid_tool_schema",
                            format!(
                                "tool {label} required field {path}.{name} has no property schema"
                            ),
                        ));
                    }
                }
            }

            if let Some(additional) = object.get("additionalProperties")
                && additional.as_bool().is_none()
            {
                return Err(CoreError::validation(
                    "invalid_tool_schema",
                    format!("tool {label} additionalProperties at {path} must be a boolean"),
                ));
            }
        }

        "array" => {
            if let Some(items) = object.get("items") {
                validate_schema_node(items, &format!("{path}[]"), label)?;
            }
        }
        "string" | "boolean" | "number" | "integer" | "null" => {}
        _ => {
            return Err(CoreError::validation(
                "invalid_tool_schema",
                format!("tool {label} schema type {schema_type} at {path} is unsupported"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_instance(schema: &Value, value: &Value) -> Result<(), String> {
    validate_instance_at(schema, value, "$")
}

fn validate_instance_at(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("schema at {path} is not an object"))?;

    if let Some(expected) = object.get("const")
        && value != expected
    {
        return Err(format!("value at {path} does not match const"));
    }
    if let Some(values) = object.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        return Err(format!("value at {path} is outside enum"));
    }

    let Some(schema_type) = object.get("type").and_then(Value::as_str) else {
        return Ok(());
    };

    match schema_type {
        "object" => {
            let value_object = value
                .as_object()
                .ok_or_else(|| format!("value at {path} must be an object"))?;
            let properties = object.get("properties").and_then(Value::as_object);

            if let Some(required) = object.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !value_object.contains_key(name) {
                        return Err(format!("required value {path}.{name} is missing"));
                    }
                }
            }

            if let Some(properties) = properties {
                for (name, child_value) in value_object {
                    if let Some(child_schema) = properties.get(name) {
                        validate_instance_at(child_schema, child_value, &format!("{path}.{name}"))?;
                    } else if object.get("additionalProperties") == Some(&Value::Bool(false)) {
                        return Err(format!("unexpected value {path}.{name}"));
                    }
                }
            } else if !value_object.is_empty()
                && object.get("additionalProperties") == Some(&Value::Bool(false))
            {
                return Err(format!("value at {path} does not allow properties"));
            }
        }
        "array" => {
            let values = value
                .as_array()
                .ok_or_else(|| format!("value at {path} must be an array"))?;
            if let Some(items) = object.get("items") {
                for (index, item) in values.iter().enumerate() {
                    validate_instance_at(items, item, &format!("{path}[{index}]"))?;
                }
            }
        }
        "string" if !value.is_string() => {
            return Err(format!("value at {path} must be a string"));
        }
        "boolean" if !value.is_boolean() => {
            return Err(format!("value at {path} must be a boolean"));
        }
        "number" if !value.is_number() => {
            return Err(format!("value at {path} must be a number"));
        }
        "integer" if !(value.is_i64() || value.is_u64()) => {
            return Err(format!("value at {path} must be an integer"));
        }
        "null" if !value.is_null() => {
            return Err(format!("value at {path} must be null"));
        }
        "string" | "boolean" | "number" | "integer" | "null" => {}
        _ => return Err(format!("unsupported schema type at {path}")),
    }

    Ok(())
}
