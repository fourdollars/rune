use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::serve::ServerState;
use crate::serve::oauth::Role;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub fn get_available_tools(role: Role) -> Vec<McpToolInfo> {
    let mut tools = vec![
        McpToolInfo {
            name: "list_notebooks".to_string(),
            description: "List all notebooks in Rune Notes".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpToolInfo {
            name: "list_note_files".to_string(),
            description: "List files within a specific notebook".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" }
                },
                "required": ["note_id"]
            }),
        },
        McpToolInfo {
            name: "read_note_file".to_string(),
            description: "Read the content of a markdown file in a notebook".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" },
                    "filename": { "type": "string", "description": "Filename" }
                },
                "required": ["note_id", "filename"]
            }),
        },
        McpToolInfo {
            name: "search_notes".to_string(),
            description: "Search notes content using search query".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search keyword" }
                },
                "required": ["query"]
            }),
        },
    ];

    if role == Role::User || role == Role::Admin {
        tools.push(McpToolInfo {
            name: "write_note_file".to_string(),
            description: "Create or update a markdown file in a notebook".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" },
                    "filename": { "type": "string", "description": "Filename" },
                    "content": { "type": "string", "description": "Markdown content" }
                },
                "required": ["note_id", "filename", "content"]
            }),
        });

        tools.push(McpToolInfo {
            name: "delete_note_file".to_string(),
            description: "Delete a markdown file from a notebook".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" },
                    "filename": { "type": "string", "description": "Filename" }
                },
                "required": ["note_id", "filename"]
            }),
        });
    }

    if role == Role::Admin {
        tools.push(McpToolInfo {
            name: "create_notebook".to_string(),
            description: "Create a new notebook (Admin only)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Notebook name/ID" },
                    "icon": { "type": "string", "description": "Optional icon emoji/symbol" }
                },
                "required": ["name"]
            }),
        });

        tools.push(McpToolInfo {
            name: "rename_notebook".to_string(),
            description: "Rename an existing notebook (Admin only)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Existing notebook ID" },
                    "new_name": { "type": "string", "description": "New notebook name" }
                },
                "required": ["note_id", "new_name"]
            }),
        });

        tools.push(McpToolInfo {
            name: "delete_notebook".to_string(),
            description: "Delete a notebook (Admin only)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID to delete" }
                },
                "required": ["note_id"]
            }),
        });

        tools.push(McpToolInfo {
            name: "set_note_visibility".to_string(),
            description: "Set public/private visibility for a notebook (Admin only)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" },
                    "public": { "type": "boolean", "description": "true for public, false for private" }
                },
                "required": ["note_id", "public"]
            }),
        });

        tools.push(McpToolInfo {
            name: "set_file_visibility".to_string(),
            description: "Set public/private visibility for a specific note file (Admin only)".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" },
                    "filename": { "type": "string", "description": "Filename" },
                    "public": { "type": "boolean", "description": "true for public, false for private" }
                },
                "required": ["note_id", "filename", "public"]
            }),
        });
    }

    tools
}

pub async fn handle_tool_call(
    state: &ServerState,
    name: &str,
    args: Value,
    role: Role,
) -> Result<Value, String> {
    match name {
        "list_notebooks" => {
            let notes = state.chat_db.list_notes().map_err(|e| e.to_string())?;
            let mut result = Vec::new();
            for n in notes {
                result.push(json!({
                    "id": n.id,
                    "name": n.name,
                    "public": n.public,
                }));
            }
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&result).unwrap_or_default()
                }]
            }))
        }
        "list_note_files" => {
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let md_dir = state.note_markdown_dir(note_id);
            let mut files: Vec<String> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
                while let Ok(Some(entry)) = rd.next_entry().await {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") {
                        files.push(name);
                    }
                }
            }
            files.sort();
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&files).unwrap_or_default()
                }]
            }))
        }
        "read_note_file" => {
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let filename = args.get("filename").and_then(|v| v.as_str()).ok_or("Missing filename")?;
            let file_path = state.note_markdown_dir(note_id).join(filename);
            let content = tokio::fs::read_to_string(&file_path)
                .await
                .map_err(|e| format!("Failed to read file: {}", e))?;
            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": content
                }]
            }))
        }
        "search_notes" => {
            let query = args.get("query").and_then(|v| v.as_str()).ok_or("Missing query")?;
            let notes = state.chat_db.list_notes().map_err(|e| e.to_string())?;
            let mut matches = Vec::new();

            for n in notes {
                let md_dir = state.note_markdown_dir(&n.id);
                if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
                    while let Ok(Some(entry)) = rd.next_entry().await {
                        let fname = entry.file_name().to_string_lossy().to_string();
                        if fname.ends_with(".md") {
                            let file_path = entry.path();
                            if let Ok(content) = tokio::fs::read_to_string(&file_path).await {
                                let c_lower = content.to_lowercase();
                                let q_lower = query.to_lowercase();
                                let f_lower = fname.to_lowercase();
                                if c_lower.contains(&q_lower) || f_lower.contains(&q_lower) {
                                    matches.push(json!({
                                        "note_id": n.id,
                                        "note_name": n.name,
                                        "filename": fname,
                                    }));
                                }
                            }
                        }
                    }
                }
            }

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string_pretty(&matches).unwrap_or_default()
                }]
            }))
        }
        "write_note_file" => {
            if role == Role::Guest {
                return Err("Guest role is not permitted to mutate files".to_string());
            }
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let filename = args.get("filename").and_then(|v| v.as_str()).ok_or("Missing filename")?;
            let content = args.get("content").and_then(|v| v.as_str()).ok_or("Missing content")?;

            let md_dir = state.note_markdown_dir(note_id);
            tokio::fs::create_dir_all(&md_dir)
                .await
                .map_err(|e| format!("Failed to create dir: {}", e))?;

            let file_path = md_dir.join(filename);
            tokio::fs::write(&file_path, content)
                .await
                .map_err(|e| format!("Failed to write file: {}", e))?;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully saved {}/{}", note_id, filename)
                }]
            }))
        }
        "delete_note_file" => {
            if role == Role::Guest {
                return Err("Guest role is not permitted to delete files".to_string());
            }
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let filename = args.get("filename").and_then(|v| v.as_str()).ok_or("Missing filename")?;

            let file_path = state.note_markdown_dir(note_id).join(filename);
            if file_path.exists() {
                tokio::fs::remove_file(&file_path)
                    .await
                    .map_err(|e| format!("Failed to delete file: {}", e))?;
            }

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully deleted {}/{}", note_id, filename)
                }]
            }))
        }
        "create_notebook" => {
            if role != Role::Admin {
                return Err("Admin role required to create notebooks".to_string());
            }
            let name = args.get("name").and_then(|v| v.as_str()).ok_or("Missing notebook name")?;
            let icon = args.get("icon").and_then(|v| v.as_str());

            if name.is_empty() {
                return Err("Notebook name cannot be empty".to_string());
            }

            let _ = state.chat_db.ensure_persistent();
            state.chat_db.create_note(name, name, icon).map_err(|e| format!("Failed to create notebook: {}", e))?;
            let md_dir = state.note_markdown_dir(name);
            let _ = tokio::fs::create_dir_all(&md_dir).await;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully created notebook {}", name)
                }]
            }))
        }
        "rename_notebook" => {
            if role != Role::Admin {
                return Err("Admin role required to rename notebooks".to_string());
            }
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let new_name = args.get("new_name").and_then(|v| v.as_str()).ok_or("Missing new_name")?;

            if new_name.is_empty() {
                return Err("New notebook name cannot be empty".to_string());
            }

            let _ = state.chat_db.ensure_persistent();
            state.chat_db.rename_note(note_id, new_name, None).map_err(|e| format!("Failed to rename notebook: {}", e))?;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully renamed notebook {} to {}", note_id, new_name)
                }]
            }))
        }
        "delete_notebook" => {
            if role != Role::Admin {
                return Err("Admin role required to delete notebooks".to_string());
            }
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;

            let _ = state.chat_db.ensure_persistent();
            state.chat_db.delete_note(note_id).map_err(|e| format!("Failed to delete notebook: {}", e))?;

            let md_dir = state.note_markdown_dir(note_id);
            if md_dir.exists() {
                let _ = tokio::fs::remove_dir_all(&md_dir).await;
            }

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully deleted notebook {}", note_id)
                }]
            }))
        }
        "set_note_visibility" => {
            if role != Role::Admin {
                return Err("Admin role required to change note visibility".to_string());
            }
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let public = args.get("public").and_then(|v| v.as_bool()).ok_or("Missing public boolean")?;

            let _ = state.chat_db.ensure_persistent();
            state.chat_db.set_note_public(note_id, public).map_err(|e| format!("Failed to set note visibility: {}", e))?;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully set notebook {} visibility to public={}", note_id, public)
                }]
            }))
        }
        "set_file_visibility" => {
            if role != Role::Admin {
                return Err("Admin role required to change file visibility".to_string());
            }
            let note_id = args.get("note_id").and_then(|v| v.as_str()).ok_or("Missing note_id")?;
            let filename = args.get("filename").and_then(|v| v.as_str()).ok_or("Missing filename")?;
            let public = args.get("public").and_then(|v| v.as_bool()).ok_or("Missing public boolean")?;

            let _ = state.chat_db.ensure_persistent();
            state.chat_db.set_file_public(note_id, filename, public).map_err(|e| format!("Failed to set file visibility: {}", e))?;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully set file {}/{} visibility to public={}", note_id, filename, public)
                }]
            }))
        }
        _ => Err(format!("Unknown tool: {}", name)),
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_available_tools_role_permissions() {
        let admin_tools = get_available_tools(Role::Admin);
        let user_tools = get_available_tools(Role::User);
        let guest_tools = get_available_tools(Role::Guest);

        let admin_names: Vec<&str> = admin_tools.iter().map(|t| t.name.as_str()).collect();
        let user_names: Vec<&str> = user_tools.iter().map(|t| t.name.as_str()).collect();
        let guest_names: Vec<&str> = guest_tools.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(guest_names, vec!["list_notebooks", "list_note_files", "read_note_file", "search_notes"]);
        assert_eq!(user_names, vec!["list_notebooks", "list_note_files", "read_note_file", "search_notes", "write_note_file", "delete_note_file"]);
        assert!(admin_names.contains(&"create_notebook"));
        assert!(admin_names.contains(&"rename_notebook"));
        assert!(admin_names.contains(&"delete_notebook"));
        assert!(admin_names.contains(&"set_note_visibility"));
        assert!(admin_names.contains(&"set_file_visibility"));
    }
}
