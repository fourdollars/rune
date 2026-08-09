use crate::serve::ServerState;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceInfo {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "mimeType")]
    pub mime_type: Option<String>,
}

pub async fn list_resources(state: &ServerState) -> Vec<McpResourceInfo> {
    let notes = state.chat_db.list_notes().unwrap_or_default();
    let mut resources = Vec::new();

    resources.push(McpResourceInfo {
        uri: "rune://notebooks".to_string(),
        name: "All Notebooks".to_string(),
        description: Some("JSON summary of all available notebooks".to_string()),
        mime_type: Some("application/json".to_string()),
    });

    for n in notes {
        resources.push(McpResourceInfo {
            uri: format!("rune://notes/{}", n.id),
            name: format!("Notebook: {}", n.name),
            description: Some(format!("FileList for notebook {}", n.name)),
            mime_type: Some("application/json".to_string()),
        });

        let md_dir = state.note_markdown_dir(&n.id);
        if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".md") {
                    resources.push(McpResourceInfo {
                        uri: format!("rune://notes/{}/{}", n.id, fname),
                        name: format!("{}/{}", n.name, fname),
                        description: Some(format!("Markdown content of {} in {}", fname, n.name)),
                        mime_type: Some("text/markdown".to_string()),
                    });
                }
            }
        }
    }

    resources
}

pub async fn read_resource(state: &ServerState, uri: &str) -> Result<Value, String> {
    if uri == "rune://notebooks" {
        let notes = state.chat_db.list_notes().map_err(|e| e.to_string())?;
        let res: Vec<_> = notes
            .into_iter()
            .map(|n| json!({"id": n.id, "name": n.name}))
            .collect();
        return Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/json",
                "text": serde_json::to_string_pretty(&res).unwrap_or_default()
            }]
        }));
    }

    let prefix = "rune://notes/";
    if let Some(path) = uri.strip_prefix(prefix) {
        let parts: Vec<&str> = path.splitn(2, '/').collect();
        if parts.len() == 1 {
            let note_id = parts[0];
            let md_dir = state.note_markdown_dir(note_id);
            let mut files = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&md_dir).await {
                while let Ok(Some(entry)) = rd.next_entry().await {
                    let fname = entry.file_name().to_string_lossy().to_string();
                    if fname.ends_with(".md") {
                        files.push(fname);
                    }
                }
            }
            files.sort();
            return Ok(json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "application/json",
                    "text": serde_json::to_string_pretty(&files).unwrap_or_default()
                }]
            }));
        } else if parts.len() == 2 {
            let note_id = parts[0];
            let filename = parts[1];
            let file_path = state.note_markdown_dir(note_id).join(filename);
            let content = tokio::fs::read_to_string(&file_path)
                .await
                .map_err(|e| format!("Resource file not found: {}", e))?;
            return Ok(json!({
                "contents": [{
                    "uri": uri,
                    "mimeType": "text/markdown",
                    "text": content
                }]
            }));
        }
    }

    Err(format!("Resource not found: {}", uri))
}
