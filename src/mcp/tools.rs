use crate::serve::oauth::Role;
use crate::serve::ServerState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
            name: "rename_note_file".to_string(),
            description: "Rename a markdown file within a notebook".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "note_id": { "type": "string", "description": "Notebook ID" },
                    "old_filename": { "type": "string", "description": "Current filename" },
                    "new_filename": { "type": "string", "description": "New filename" }
                },
                "required": ["note_id", "old_filename", "new_filename"]
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
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
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
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let filename = args
                .get("filename")
                .and_then(|v| v.as_str())
                .ok_or("Missing filename")?;
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
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or("Missing query")?;
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
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let filename = args
                .get("filename")
                .and_then(|v| v.as_str())
                .ok_or("Missing filename")?;
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or("Missing content")?;

            let md_dir = state.note_markdown_dir(note_id);
            tokio::fs::create_dir_all(&md_dir)
                .await
                .map_err(|e| format!("Failed to create dir: {}", e))?;

            let file_path = md_dir.join(filename);
            tokio::fs::write(&file_path, content)
                .await
                .map_err(|e| format!("Failed to write file: {}", e))?;

            let room = state.get_or_create_room(note_id).await;
            let fc = crate::serve::api::SseMsg::FileContent {
                note_id: note_id.to_string(),
                filename: filename.to_string(),
                content: content.to_string(),
            };
            crate::serve::api::broadcast_to_room(&room, &fc);
            crate::serve::api::broadcast_file_list(state, note_id).await;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Successfully saved {}/{}", note_id, filename)
                }]
            }))
        }
        "rename_note_file" => {
            if role == Role::Guest {
                return Err("Guest role is not permitted to rename files".to_string());
            }
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let old_filename = args
                .get("old_filename")
                .or_else(|| args.get("filename"))
                .and_then(|v| v.as_str())
                .ok_or("Missing old_filename")?;
            let new_filename = args
                .get("new_filename")
                .or_else(|| args.get("new_name"))
                .and_then(|v| v.as_str())
                .ok_or("Missing new_filename")?;

            if !crate::serve::api::is_valid_filename(new_filename) {
                return Err(format!("Invalid filename: {}", new_filename));
            }

            let md_dir = state.note_markdown_dir(note_id);
            let old_path = md_dir.join(old_filename);
            let new_path = md_dir.join(new_filename);

            if !old_path.exists() {
                return Err(format!("File not found: {}", old_filename));
            }
            if new_path.exists() {
                return Err(format!("File already exists: {}", new_filename));
            }

            tokio::fs::rename(&old_path, &new_path)
                .await
                .map_err(|e| format!("Failed to rename file: {}", e))?;

            // If visibility was set for old_filename, update it for new_filename
            let visibility = state.chat_db.get_file_visibility(note_id);
            if let Some((_, public)) = visibility.iter().find(|(f, _)| f == old_filename) {
                let pub_val = *public;
                let _ = state.chat_db.remove_file_visibility(note_id, old_filename);
                let _ = state
                    .chat_db
                    .set_file_public(note_id, new_filename, pub_val);
            }

            let room = state.get_or_create_room(note_id).await;
            let del = crate::serve::api::SseMsg::FileDeleted {
                filename: old_filename.to_string(),
            };
            crate::serve::api::broadcast_to_room(&room, &del);
            crate::serve::api::broadcast_file_list(state, note_id).await;

            Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!(
                        "Successfully renamed {} to {} in notebook {}",
                        old_filename, new_filename, note_id
                    )
                }]
            }))
        }
        "delete_note_file" => {
            if role == Role::Guest {
                return Err("Guest role is not permitted to delete files".to_string());
            }
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let filename = args
                .get("filename")
                .and_then(|v| v.as_str())
                .ok_or("Missing filename")?;

            let file_path = state.note_markdown_dir(note_id).join(filename);
            if file_path.exists() {
                tokio::fs::remove_file(&file_path)
                    .await
                    .map_err(|e| format!("Failed to delete file: {}", e))?;
            }
            let _ = state.chat_db.remove_file_visibility(note_id, filename);

            let room = state.get_or_create_room(note_id).await;
            let del = crate::serve::api::SseMsg::FileDeleted {
                filename: filename.to_string(),
            };
            crate::serve::api::broadcast_to_room(&room, &del);
            crate::serve::api::broadcast_file_list(state, note_id).await;

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
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or("Missing notebook name")?;
            let icon = args.get("icon").and_then(|v| v.as_str());

            if name.is_empty() {
                return Err("Notebook name cannot be empty".to_string());
            }

            let _ = state.chat_db.ensure_persistent();
            state
                .chat_db
                .create_note(name, name, icon)
                .map_err(|e| format!("Failed to create notebook: {}", e))?;
            let md_dir = state.note_markdown_dir(name);
            let _ = tokio::fs::create_dir_all(&md_dir).await;

            crate::serve::api::broadcast_note_list(state).await;

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
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let new_name = args
                .get("new_name")
                .and_then(|v| v.as_str())
                .ok_or("Missing new_name")?;

            if new_name.is_empty() {
                return Err("New notebook name cannot be empty".to_string());
            }

            let _ = state.chat_db.ensure_persistent();
            match state.chat_db.rename_note(note_id, new_name, None) {
                Ok(Some(new_id)) => {
                    let old_dir = state.data_dir.join("notes").join(note_id);
                    let new_dir = state.data_dir.join("notes").join(&new_id);
                    if old_dir.exists() && old_dir != new_dir {
                        if !new_dir.exists() {
                            let _ = tokio::fs::rename(&old_dir, &new_dir).await;
                        } else {
                            crate::serve::api::merge_note_dirs(&old_dir, &new_dir).await;
                            let _ = tokio::fs::remove_dir_all(&old_dir).await;
                        }
                    }
                    crate::serve::api::broadcast_note_list(state).await;
                    Ok(json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Successfully renamed notebook {} to {}", note_id, new_name)
                        }]
                    }))
                }
                Ok(None) => Err(format!("Notebook '{}' not found", note_id)),
                Err(e) => Err(format!("Failed to rename notebook: {}", e)),
            }
        }
        "delete_notebook" => {
            if role != Role::Admin {
                return Err("Admin role required to delete notebooks".to_string());
            }
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;

            let _ = state.chat_db.ensure_persistent();
            match state.chat_db.delete_note(note_id) {
                Ok(true) => {
                    // Cancel any running AI task in the room, then remove the room
                    {
                        let rooms = state.rooms.read().await;
                        if let Some(room) = rooms.get(note_id) {
                            let guard = room.cancel_token.lock().unwrap();
                            if let Some(ref token) = *guard {
                                token.cancel();
                            }
                        }
                    }
                    state.rooms.write().await.remove(note_id);

                    let note_dir = state.data_dir.join("notes").join(note_id);
                    if note_dir.exists() {
                        let _ = tokio::fs::remove_dir_all(&note_dir).await;
                    }

                    crate::serve::api::broadcast_note_list(state).await;

                    Ok(json!({
                        "content": [{
                            "type": "text",
                            "text": format!("Successfully deleted notebook {}", note_id)
                        }]
                    }))
                }
                Ok(false) => Err(format!("Notebook '{}' not found", note_id)),
                Err(e) => Err(format!("Failed to delete notebook: {}", e)),
            }
        }
        "set_note_visibility" => {
            if role != Role::Admin {
                return Err("Admin role required to change note visibility".to_string());
            }
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let public = args
                .get("public")
                .and_then(|v| v.as_bool())
                .ok_or("Missing public boolean")?;

            let _ = state.chat_db.ensure_persistent();
            state
                .chat_db
                .set_note_public(note_id, public)
                .map_err(|e| format!("Failed to set note visibility: {}", e))?;

            crate::serve::api::broadcast_note_list(state).await;

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
            let note_id = args
                .get("note_id")
                .and_then(|v| v.as_str())
                .ok_or("Missing note_id")?;
            let filename = args
                .get("filename")
                .and_then(|v| v.as_str())
                .ok_or("Missing filename")?;
            let public = args
                .get("public")
                .and_then(|v| v.as_bool())
                .ok_or("Missing public boolean")?;

            let _ = state.chat_db.ensure_persistent();
            state
                .chat_db
                .set_file_public(note_id, filename, public)
                .map_err(|e| format!("Failed to set file visibility: {}", e))?;

            crate::serve::api::broadcast_file_list(state, note_id).await;

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
    use crate::config::RuneConfig;
    use crate::provider::ModelInfo;
    use crate::serve::db::ChatDb;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::{broadcast, RwLock};

    fn test_state(tmp: &TempDir) -> ServerState {
        let (admin_broadcast_tx, _) = broadcast::channel(256);
        let db_path = tmp.path().join("test.db");
        let db = ChatDb::open(&db_path).unwrap();
        ServerState {
            config: RuneConfig::default(),
            sessions: crate::serve::oauth::SessionStore::new(),
            files: Arc::new(RwLock::new(std::collections::HashMap::new())),
            active_file: Arc::new(RwLock::new(String::new())),
            models: Arc::new(RwLock::new(vec![ModelInfo {
                provider: None,
                id: "gpt-5-mini".into(),
                context_window: None,
                reasoning_efforts: vec![],
                supported_endpoints: vec![],
            }])),
            rooms: Arc::new(RwLock::new(std::collections::HashMap::new())),
            global_default_model: Arc::new(RwLock::new("gpt-5-mini".into())),
            admin_broadcast_tx,
            chat_db: db,
            data_dir: tmp.path().join(".rune"),
            oauth_codes: crate::serve::oauth_pkce::AuthCodeStore::new(),
            oauth_tokens: crate::serve::oauth_pkce::OAuthTokenStore::new(),
            oauth_providers: Arc::new(RwLock::new(std::collections::HashMap::new())),
            mcp_sessions: crate::mcp::mcp_session::McpSessionStore::new(),
            provider_registry: Arc::new(RwLock::new(crate::provider::ProviderRegistry::new())),
        }
    }

    #[test]
    fn test_get_available_tools_role_permissions() {
        let admin_tools = get_available_tools(Role::Admin);
        let user_tools = get_available_tools(Role::User);
        let guest_tools = get_available_tools(Role::Guest);

        let admin_names: Vec<&str> = admin_tools.iter().map(|t| t.name.as_str()).collect();
        let user_names: Vec<&str> = user_tools.iter().map(|t| t.name.as_str()).collect();
        let guest_names: Vec<&str> = guest_tools.iter().map(|t| t.name.as_str()).collect();

        assert_eq!(
            guest_names,
            vec![
                "list_notebooks",
                "list_note_files",
                "read_note_file",
                "search_notes"
            ]
        );
        assert_eq!(
            user_names,
            vec![
                "list_notebooks",
                "list_note_files",
                "read_note_file",
                "search_notes",
                "write_note_file",
                "rename_note_file",
                "delete_note_file"
            ]
        );
        assert!(admin_names.contains(&"create_notebook"));
        assert!(admin_names.contains(&"rename_notebook"));
        assert!(admin_names.contains(&"delete_notebook"));
        assert!(admin_names.contains(&"set_note_visibility"));
        assert!(admin_names.contains(&"set_file_visibility"));
    }

    #[tokio::test]
    async fn test_mcp_file_write_rename_delete_broadcasts() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        // Create notebook first
        state.chat_db.create_note("note-1", "note-1", None).unwrap();
        let room = state.get_or_create_room("note-1").await;
        let mut rx = room.broadcast_tx.subscribe();

        // 1. write_note_file
        let write_args = json!({
            "note_id": "note-1",
            "filename": "hello.md",
            "content": "# Hello World"
        });
        let res = handle_tool_call(&state, "write_note_file", write_args, Role::User).await;
        assert!(res.is_ok());

        // Drain messages and check for file_content and note_list
        let mut got_file_content = false;
        let mut got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("file_content") {
                    got_file_content = true;
                }
                if msg.contains("note_list") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_file_content,
            "write_note_file should broadcast file_content"
        );
        assert!(got_note_list, "write_note_file should broadcast note_list");

        // 2. rename_note_file
        let rename_args = json!({
            "note_id": "note-1",
            "old_filename": "hello.md",
            "new_filename": "world.md"
        });
        let res = handle_tool_call(&state, "rename_note_file", rename_args, Role::User).await;
        assert!(res.is_ok());

        let old_path = state.note_markdown_dir("note-1").join("hello.md");
        let new_path = state.note_markdown_dir("note-1").join("world.md");
        assert!(!old_path.exists());
        assert!(new_path.exists());

        let mut got_file_deleted = false;
        got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("file_deleted") {
                    got_file_deleted = true;
                }
                if msg.contains("note_list") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_file_deleted,
            "rename_note_file should broadcast file_deleted for old file"
        );
        assert!(got_note_list, "rename_note_file should broadcast note_list");

        // 3. delete_note_file
        let delete_args = json!({
            "note_id": "note-1",
            "filename": "world.md"
        });
        let res = handle_tool_call(&state, "delete_note_file", delete_args, Role::User).await;
        assert!(res.is_ok());
        assert!(!new_path.exists());

        got_file_deleted = false;
        got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("file_deleted") {
                    got_file_deleted = true;
                }
                if msg.contains("note_list") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_file_deleted,
            "delete_note_file should broadcast file_deleted"
        );
        assert!(got_note_list, "delete_note_file should broadcast note_list");
    }

    #[tokio::test]
    async fn test_mcp_notebook_crud_and_visibility_broadcasts() {
        let tmp = TempDir::new().unwrap();
        let state = test_state(&tmp);

        // Pre-create room-0 to receive broadcasts
        state.chat_db.create_note("note-0", "note-0", None).unwrap();
        let room0 = state.get_or_create_room("note-0").await;
        let mut rx = room0.broadcast_tx.subscribe();

        // 1. create_notebook
        let create_args = json!({
            "name": "project-x"
        });
        let res = handle_tool_call(&state, "create_notebook", create_args, Role::Admin).await;
        assert!(res.is_ok());

        let mut got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("note_list") && msg.contains("project-x") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_note_list,
            "create_notebook should broadcast note_list with new note"
        );

        // Create a file in project-x
        let file_path = state.note_markdown_dir("project-x").join("readme.md");
        tokio::fs::write(&file_path, "# Readme").await.unwrap();

        // 2. rename_notebook
        let rename_args = json!({
            "note_id": "project-x",
            "new_name": "project-y"
        });
        let res = handle_tool_call(&state, "rename_notebook", rename_args, Role::Admin).await;
        assert!(res.is_ok());

        let old_dir = state.data_dir.join("notes").join("project-x");
        let new_file_path = state.note_markdown_dir("project-y").join("readme.md");
        assert!(!old_dir.exists());
        assert!(new_file_path.exists());

        got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("note_list") && msg.contains("project-y") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_note_list,
            "rename_notebook should broadcast note_list with renamed note"
        );

        // 3. set_note_visibility
        let vis_args = json!({
            "note_id": "project-y",
            "public": true
        });
        let res = handle_tool_call(&state, "set_note_visibility", vis_args, Role::Admin).await;
        assert!(res.is_ok());

        got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("note_list") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_note_list,
            "set_note_visibility should broadcast note_list"
        );

        // 4. set_file_visibility
        let fvis_args = json!({
            "note_id": "project-y",
            "filename": "readme.md",
            "public": true
        });
        let res = handle_tool_call(&state, "set_file_visibility", fvis_args, Role::Admin).await;
        assert!(res.is_ok());

        got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("note_list") {
                    got_note_list = true;
                }
            }
        }
        assert!(
            got_note_list,
            "set_file_visibility should broadcast note_list"
        );

        // 5. delete_notebook
        let del_args = json!({
            "note_id": "project-y"
        });
        let res = handle_tool_call(&state, "delete_notebook", del_args, Role::Admin).await;
        assert!(res.is_ok());

        let note_dir = state.data_dir.join("notes").join("project-y");
        assert!(!note_dir.exists());

        got_note_list = false;
        for _ in 0..5 {
            if let Ok(msg) = rx.try_recv() {
                if msg.contains("note_list") {
                    got_note_list = true;
                }
            }
        }
        assert!(got_note_list, "delete_notebook should broadcast note_list");
    }
}
