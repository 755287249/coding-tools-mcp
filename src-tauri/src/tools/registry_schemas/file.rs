use serde_json::{json, Value};

pub(super) fn input_schema(name: &str) -> Option<Value> {
    match name {
        "read_file" => Some(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "minLength": 1 },
                "start_line": { "type": "integer", "minimum": 1, "default": 1 },
                "end_line": { "type": "integer", "minimum": 1 },
                "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 131072 }
            },
            "required": ["path"],
            "additionalProperties": false
        })),
        "read_many" => Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "maxItems": 100,
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "minLength": 1 },
                            "start_line": { "type": "integer", "minimum": 1, "default": 1 },
                            "end_line": { "type": "integer", "minimum": 1 },
                            "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576 }
                        },
                        "required": ["path"],
                        "additionalProperties": false
                    }
                },
                "matches": {
                    "type": "array",
                    "maxItems": 500,
                    "description": "Search match objects containing path and line, or explicit ranges.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "minLength": 1 },
                            "line": { "type": "integer", "minimum": 1 },
                            "start_line": { "type": "integer", "minimum": 1 },
                            "end_line": { "type": "integer", "minimum": 1 },
                            "max_bytes": { "type": "integer", "minimum": 1, "maximum": 1048576 },
                            "match_id": { "type": "string" }
                        },
                        "required": ["path"],
                        "additionalProperties": true
                    }
                },
                "context_lines": { "type": "integer", "minimum": 0, "maximum": 500, "default": 20 },
                "merge_overlaps": { "type": "boolean", "default": true },
                "line_numbers": { "type": "boolean", "default": false, "description": "Compatibility shortcut: when true and content_mode is omitted, return numbered-only content." },
                "content_mode": {
                    "type": "string",
                    "enum": ["plain", "numbered", "both"],
                    "description": "Choose one content representation to avoid duplicate payloads. Defaults to numbered when line_numbers=true, otherwise plain. Use both only when both representations are required."
                },
                "max_total_bytes": { "type": "integer", "minimum": 1, "maximum": 4194304, "default": 262144 },
                "max_bytes_per_file": { "type": "integer", "minimum": 1, "maximum": 1048576, "default": 131072 }
            },
            "additionalProperties": false
        })),
        "project_map" => Some(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": "." },
                "max_files": { "type": "integer", "minimum": 1, "maximum": 50000, "default": 10000 },
                "max_entries": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 1000 },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 20, "default": 4 },
                "include_hidden": { "type": "boolean", "default": false },
                "include_ignored": { "type": "boolean", "default": false },
                "include_generated": { "type": "boolean", "default": false, "description": "Include generated dependency/build trees such as node_modules, target, dist, and build. .git remains excluded." }
            },
            "additionalProperties": false
        })),
        "list_files" => Some(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "default": "." },
                "patterns": { "type": "array", "items": { "type": "string" } },
                "glob": { "type": "string", "description": "Alias for a single patterns entry" },
                "exclude_patterns": { "type": "array", "items": { "type": "string" } },
                "entry_types": { "type": "array", "items": { "type": "string", "enum": ["file", "directory", "symlink"] }, "default": ["file", "symlink"] },
                "recursive": { "type": "boolean", "default": true },
                "max_depth": { "type": "integer", "minimum": 1, "maximum": 20, "default": 20 },
                "include_hidden": { "type": "boolean", "default": false },
                "include_ignored": { "type": "boolean", "default": false },
                "include_generated": { "type": "boolean", "default": false, "description": "Include generated dependency/build trees such as node_modules, target, dist, and build. .git remains excluded." },
                "max_results": { "type": "integer", "minimum": 1, "maximum": 50000, "default": 1000 }
            },
            "additionalProperties": false
        })),
        "search_text" => Some(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "minLength": 1 },
                "queries": {
                    "type": "array",
                    "maxItems": 50,
                    "items": {
                        "oneOf": [
                            { "type": "string", "minLength": 1 },
                            {
                                "type": "object",
                                "properties": {
                                    "query": { "type": "string", "minLength": 1 },
                                    "regex": { "type": "boolean" },
                                    "case_sensitive": { "type": "boolean" }
                                },
                                "required": ["query"],
                                "additionalProperties": false
                            }
                        ]
                    }
                },
                "filename_query": { "type": "string", "minLength": 1 },
                "filename_regex": { "type": "boolean", "default": false },
                "filename_case_sensitive": { "type": "boolean", "default": false },
                "path": { "type": "string", "default": "." },
                "glob": { "type": "string", "description": "Alias appended to include_globs" },
                "include_globs": { "type": "array", "items": { "type": "string" } },
                "exclude_globs": { "type": "array", "items": { "type": "string" } },
                "regex": { "type": "boolean", "default": false },
                "case_sensitive": { "type": "boolean", "default": false },
                "context_lines": { "type": "integer", "minimum": 0, "maximum": 1000, "default": 0, "description": "Values above 20 are normalized to 20" },
                "max_preview_bytes": { "type": "integer", "minimum": 1, "maximum": 65536, "default": 512, "description": "Normalized to the supported 64-4096 byte range" },
                "max_file_bytes": { "type": "integer", "minimum": 1024, "maximum": 134217728, "default": 8388608 },
                "max_matches_per_file": { "type": "integer", "minimum": 1, "maximum": 100000 },
                "files_only": { "type": "boolean", "default": false },
                "count_only": { "type": "boolean", "default": false },
                "calculate_total": { "type": "boolean", "default": false, "description": "Continue scanning after max_results to calculate an exact total match count. count_only always calculates the total." },
                "cursor": { "type": "integer", "minimum": 0, "default": 0 },
                "include_hidden": { "type": "boolean", "default": false },
                "include_ignored": { "type": "boolean", "default": false },
                "include_generated": { "type": "boolean", "default": false, "description": "Include generated dependency/build trees such as node_modules, target, dist, and build. .git remains excluded." },
                "max_results": { "type": "integer", "minimum": 1, "maximum": 10000, "default": 200 }
            },
            "additionalProperties": false
        })),
        _ => return None,
    }
}
