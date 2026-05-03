use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read};

#[derive(Debug, Deserialize)]
pub struct CheckRequest {
    pub source: Value,
    pub version: Option<Value>,
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct Version(pub Value);

#[derive(Debug, Serialize, PartialEq)]
pub struct CheckResponse(pub Vec<Value>);

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct InResponse {
    pub version: Value,
    pub metadata: Vec<MetadataItem>,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct OutResponse {
    pub version: Value,
    pub metadata: Vec<MetadataItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MetadataItem {
    pub name: String,
    pub value: String,
}

pub enum ConcourseMode {
    Check,
    In,
    Out,
}

fn read_to_string_from<R: Read>(mut reader: R) -> io::Result<String> {
    let mut s = String::new();
    reader.read_to_string(&mut s)?;
    Ok(s)
}

/// Handle `check` mode: if a version is present in the request, return it in an array;
/// otherwise return an empty array.
pub fn handle_check<R: Read>(reader: R) -> anyhow::Result<CheckResponse> {
    let s = read_to_string_from(reader)?;
    let req: CheckRequest = serde_json::from_str(&s)
        .map_err(|e| anyhow::anyhow!("invalid JSON payload (schema validation failed): {}", e))?;

    if let Some(v) = req.version {
        Ok(CheckResponse(vec![v]))
    } else {
        // First check: return a synthetic version to indicate resource availability
        Ok(CheckResponse(vec![serde_json::json!({"ref": "latest"})]))
    }
}

/// Handle `in` mode: return a version (echoed or generated), some metadata and the current path.
pub fn handle_in<R: Read>(reader: R) -> anyhow::Result<InResponse> {
    let s = read_to_string_from(reader)?;
    let req: CheckRequest = serde_json::from_str(&s)
        .map_err(|e| anyhow::anyhow!("invalid JSON payload (schema validation failed): {}", e))?;

    let version = req
        .version
        .unwrap_or_else(|| serde_json::json!({"generated": "in-1"}));

    let metadata = vec![MetadataItem {
        name: "source".to_string(),
        value: serde_json::to_string(&req.source)
            .unwrap_or_else(|_| "<invalid-source>".to_string()),
    }];

    let path = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string());

    Ok(InResponse {
        version,
        metadata,
        path,
    })
}

/// Handle `out` mode: return a version (echoed or generated) and metadata.
pub fn handle_out<R: Read>(reader: R) -> anyhow::Result<OutResponse> {
    let s = read_to_string_from(reader)?;
    let req: CheckRequest = serde_json::from_str(&s)
        .map_err(|e| anyhow::anyhow!("invalid JSON payload (schema validation failed): {}", e))?;

    let version = req
        .version
        .unwrap_or_else(|| serde_json::json!({"generated": "out-1"}));

    let metadata = vec![MetadataItem {
        name: "source".to_string(),
        value: serde_json::to_string(&req.source)
            .unwrap_or_else(|_| "<invalid-source>".to_string()),
    }];

    Ok(OutResponse { version, metadata })
}

/// Keep the simple run() route used by main.rs; read stdin and write JSON to stdout.
pub fn run(mode: ConcourseMode) {
    match mode {
        ConcourseMode::Check => match handle_check(io::stdin()) {
            Ok(resp) => match serde_json::to_string(&resp.0) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("Failed to serialize CheckResponse: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error running check: {}", e);
                std::process::exit(1);
            }
        },
        ConcourseMode::In => match handle_in(io::stdin()) {
            Ok(resp) => match serde_json::to_string(&resp) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("Failed to serialize InResponse: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error running in: {}", e);
                std::process::exit(1);
            }
        },
        ConcourseMode::Out => match handle_out(io::stdin()) {
            Ok(resp) => match serde_json::to_string(&resp) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("Failed to serialize OutResponse: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                eprintln!("Error running out: {}", e);
                std::process::exit(1);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_check_with_version() {
        let input = json!({"source": {}, "version": {"v": "1"}}).to_string();
        let resp = handle_check(input.as_bytes()).expect("handle_check");
        assert_eq!(resp.0.len(), 1);
        assert_eq!(resp.0[0], json!({"v": "1"}));
    }

    #[test]
    fn test_check_without_version() {
        let input = json!({"source": {}}).to_string();
        let resp = handle_check(input.as_bytes()).expect("handle_check");
        assert_eq!(resp.0.len(), 1); // First check returns synthetic version
    }

    #[test]
    fn test_in_out_serde_roundtrip() {
        let v = json!({"v": "1"});
        let metadata = vec![MetadataItem {
            name: "k".into(),
            value: "v".into(),
        }];
        let path = "/tmp".to_string();

        let inresp = InResponse {
            version: v.clone(),
            metadata: metadata.clone(),
            path: path.clone(),
        };

        let s = serde_json::to_string(&inresp).expect("serialize inresp");
        let parsed: InResponse = serde_json::from_str(&s).expect("deserialize inresp");
        assert_eq!(parsed, inresp);

        let outresp = OutResponse {
            version: v,
            metadata,
        };

        let s2 = serde_json::to_string(&outresp).expect("serialize outresp");
        let parsed2: OutResponse = serde_json::from_str(&s2).expect("deserialize outresp");
        assert_eq!(parsed2, outresp);
    }
}
