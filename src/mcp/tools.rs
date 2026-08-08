use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use crate::serve::ServerState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub fn get_available_tools(is_guest: bool) -> Vec<McpToolInfo> {
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

    if !is_guest {
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

    tools
}

pub async fn handle_tool_call(
    state: &ServerState,
    name: &str,
    args: Value,
    is_guest: bool,
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
            if is_guest {
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
            if is_guest {
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
        _ => Err(format!("Unknown tool: {}", name)),
    }
}
