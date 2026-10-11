use serde_json::{json, Map, Value};

use crate::tools::{is_allowed_tool, is_mcp_only_tool, is_mutating_tool};

pub fn build_openapi(tools: &[Value], public_base_url: &str, auth_type: &str) -> Value {
    let mut paths = Map::new();
    let use_api_key = auth_type == "api_key";

    for tool in tools {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !is_allowed_tool(name) || is_mcp_only_tool(name) {
            continue;
        }

        let input_schema = action_input_schema(
            name,
            tool.get("inputSchema")
                .filter(|schema| schema.is_object())
                .cloned()
                .unwrap_or_else(|| {
                    json!({
                        "type": "object",
                        "additionalProperties": true
                    })
                }),
        );

        let description_raw = tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("Call coding tool");
        let routing_hint = if tool["inputSchema"]["properties"]["attachment_id"].is_object() {
            " Global connection: call chat_open with the supplied chat_id and workspace_folder_id. Every subsequent project operation requires these IDs and the returned attachment_id. Follow the complete skill.text. Projects cannot be switched. "
        } else if name == "list_workspace_folders" {
            " Call this first to discover allowed tool-hub folders."
        } else if workspace_folder_id_optional(name) {
            " Pass workspace_folder_id from list_workspace_folders, unless this control call can be routed from its session_id, output_ref, or resume_id. There is no default folder."
        } else {
            " Pass workspace_folder_id from list_workspace_folders. The request is rejected when no folder is selected; there is no default folder."
        };
        let description: String = format!("{routing_hint}{description_raw}")
            .chars()
            .take(900)
            .collect();
        let summary: String = description_raw.chars().take(300).collect();

        let mut operation = json!({
            "operationId": format!("coding_{name}"),
            "summary": summary,
            "description": description,
            "requestBody": {
                "required": name != "list_workspace_folders",
                "content": {
                    "application/json": {
                        "schema": input_schema
                    }
                }
            },
            "responses": {
                "200": {
                    "description": "Tool execution result",
                    "content": {
                        "application/json": {
                            "schema": { "$ref": "#/components/schemas/ToolExecutionResponse" }
                        }
                    }
                },
                "400": { "description": "Invalid request or policy rejection" },
                "401": { "description": "Invalid API key" },
                "422": { "description": "Tool execution failed" },
                "502": { "description": "MCP backend failure" }
            },
            "x-openai-isConsequential": is_mutating_tool(name)
        });

        if use_api_key {
            operation
                .as_object_mut()
                .expect("operation object")
                .insert("security".to_string(), json!([{ "bearerAuth": [] }]));
        }

        paths.insert(format!("/actions/{name}"), json!({ "post": operation }));
    }

    let mut document = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Coding Tools Actions",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Read, modify and test folders in one shared coding tool hub. There is no default folder: call list_workspace_folders, then pass workspace_folder_id on project tool requests."
        },
        "servers": [{ "url": public_base_url.trim_end_matches('/') }],
        "paths": paths,
        "components": {
            "schemas": {
                "ContentPart": content_part_schema(),
                "ToolError": tool_error_schema(),
                "StructuredContent": structured_content_schema(),
                "ToolExecutionResponse": {
                    "type": "object",
                    "properties": {
                        "ok": { "type": "boolean" },
                        "tool": { "type": "string" },
                        "workspace_folder_id": { "type": "string" },
                        "workspace": { "type": "string" },
                        "structured_content": { "$ref": "#/components/schemas/StructuredContent" },
                        "content": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/ContentPart" }
                        },
                        "is_error": { "type": "boolean" }
                    },
                    "required": ["ok", "tool", "is_error"],
                    "additionalProperties": true
                }
            }
        }
    });

    if use_api_key {
        document
            .as_object_mut()
            .expect("document object")
            .get_mut("components")
            .and_then(Value::as_object_mut)
            .expect("components object")
            .insert(
                "securitySchemes".to_string(),
                json!({
                    "bearerAuth": {
                        "type": "http",
                        "scheme": "bearer"
                    }
                }),
            );
    }

    document
}

fn action_input_schema(tool_name: &str, mut schema: Value) -> Value {
    if tool_name == "list_workspace_folders" {
        return schema;
    }
    let Some(object) = schema.as_object_mut() else {
        return schema;
    };
    object
        .entry("type".to_string())
        .or_insert_with(|| json!("object"));
    let properties = object
        .entry("properties".to_string())
        .or_insert_with(|| json!({}));
    if let Some(properties) = properties.as_object_mut() {
        properties.insert(
            "workspace_folder_id".to_string(),
            json!({
                "type": "string",
                "minLength": 1,
                "description": "Tool-hub folder ID returned by list_workspace_folders. Required unless a session/output/resume control call can be routed from its identifier."
            }),
        );
    }
    if !workspace_folder_id_optional(tool_name) {
        let required = object
            .entry("required".to_string())
            .or_insert_with(|| json!([]));
        if let Some(required) = required.as_array_mut() {
            let field = Value::String("workspace_folder_id".to_string());
            if !required.contains(&field) {
                required.push(field);
            }
        }
    }
    schema
}

fn workspace_folder_id_optional(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "list_workspace_folders"
            | "wait_command"
            | "send_input"
            | "kill_session"
            | "read_output"
            | "request_permissions"
    )
}

fn content_part_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "type": "string" },
            "text": { "type": "string" },
            "mimeType": { "type": "string" },
            "data": { "type": "string" }
        },
        "required": ["type"],
        "additionalProperties": true
    })
}

fn tool_error_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": { "type": "string" },
            "message": { "type": "string" },
            "category": { "type": "string" },
            "retryable": { "type": "boolean" },
            "details": {
                "type": "object",
                "properties": {},
                "additionalProperties": true
            }
        },
        "additionalProperties": true
    })
}

fn structured_content_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ok": { "type": "boolean" },
            "error": tool_error_schema(),
            "diagnostics": {
                "type": "object",
                "properties": {},
                "additionalProperties": true
            },
            "permission_request": {
                "type": "object",
                "properties": {
                    "tool_name": { "type": "string" },
                    "permission": { "type": "string" },
                    "status": { "type": "string" },
                    "retryable": { "type": "boolean" }
                },
                "additionalProperties": true
            }
        },
        "additionalProperties": true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn openapi_without_auth_has_no_security_scheme() {
        let tools = [json!({
            "name": "read_file",
            "description": "Read a file",
            "inputSchema": { "type": "object" }
        })];
        let schema = build_openapi(&tools, "https://actions.example.com", "none");
        assert!(schema["paths"]["/actions/read_file"]["post"]["security"].is_null());
        assert!(schema["components"]["securitySchemes"].is_null());
    }

    #[test]
    fn openapi_api_key_includes_bearer_security() {
        let tools = [json!({
            "name": "read_file",
            "description": "Read a file",
            "inputSchema": { "type": "object" }
        })];
        let schema = build_openapi(&tools, "https://actions.example.com", "api_key");
        assert_eq!(
            schema["components"]["securitySchemes"]["bearerAuth"]["scheme"],
            "bearer"
        );
        assert_eq!(
            schema["paths"]["/actions/read_file"]["post"]["security"],
            json!([{ "bearerAuth": [] }])
        );
    }

    #[test]
    fn core_openapi_exposes_search_text_as_read_only() {
        let tools = crate::tools::list_tools_for_profile("core");
        let schema = build_openapi(&tools, "https://actions.example.com", "none");
        let operation = &schema["paths"]["/actions/search_text"]["post"];

        assert_eq!(operation["operationId"], "coding_search_text");
        assert_eq!(operation["x-openai-isConsequential"], false);
        let request_schema = &operation["requestBody"]["content"]["application/json"]["schema"];
        assert_eq!(request_schema["additionalProperties"], false);
        assert_eq!(request_schema["properties"]["query"]["type"], "string");
        assert_eq!(
            request_schema["properties"]["workspace_folder_id"]["type"],
            "string"
        );
        assert!(request_schema["required"]
            .as_array()
            .expect("required fields")
            .contains(&Value::String("workspace_folder_id".into())));
        assert!(operation["description"]
            .as_str()
            .expect("description")
            .contains("no default folder"));
    }

    #[test]
    fn session_control_can_route_without_explicit_folder_id() {
        let tools = crate::tools::list_tools_for_profile("full");
        let schema = build_openapi(&tools, "https://actions.example.com", "none");
        let request_schema = &schema["paths"]["/actions/wait_command"]["post"]["requestBody"]
            ["content"]["application/json"]["schema"];

        assert!(request_schema["properties"]["workspace_folder_id"].is_object());
        assert!(!request_schema["required"]
            .as_array()
            .expect("required fields")
            .contains(&Value::String("workspace_folder_id".into())));
    }

    #[test]
    fn openapi_exposes_folder_listing_but_not_global_switch() {
        let tools = crate::tools::list_tools_for_profile("core");
        let schema = build_openapi(&tools, "https://actions.example.com", "none");

        assert!(schema["paths"]["/actions/list_workspace_folders"].is_object());
        assert!(schema["paths"]["/actions/switch_workspace_folder"].is_null());
        assert!(
            schema["paths"]["/actions/list_workspace_folders"]["post"]["requestBody"]["content"]
                ["application/json"]["schema"]["properties"]["workspace_folder_id"]
                .is_null()
        );
    }
}
