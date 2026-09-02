use crate::config::ContextSummaryMode;
use crate::domain::ids::ConfigRef;
use chrono::{DateTime, Utc};
use openresponses_rust::CreateResponseBody;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionConfigItem {
    pub name: String,
    pub path: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub config: serde_json::Value,
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConfigRevision {
    pub format_version: u32,
    pub config_ref: ConfigRef,
    pub created_at: DateTime<Utc>,
    pub response_template: CreateResponseBody,
    #[serde(default)]
    pub extensions: Vec<ExtensionConfigItem>,
    #[serde(default)]
    pub context_summary: ContextSummaryMode,
}

#[derive(Serialize)]
struct CanonicalConfigPayload<'a> {
    format_version: u32,
    response_template: &'a CreateResponseBody,
    extensions: &'a [ExtensionConfigItem],
    context_summary: &'a ContextSummaryMode,
}

impl ConfigRevision {
    pub fn new(
        response_template: CreateResponseBody,
        extensions: Vec<ExtensionConfigItem>,
        context_summary: ContextSummaryMode,
    ) -> Self {
        let format_version = 1;
        let config_ref = Self::compute_ref(
            format_version,
            &response_template,
            &extensions,
            &context_summary,
        );
        Self {
            format_version,
            config_ref,
            created_at: Utc::now(),
            response_template,
            extensions,
            context_summary,
        }
    }

    pub fn compute_self_ref(&self) -> ConfigRef {
        Self::compute_ref(
            self.format_version,
            &self.response_template,
            &self.extensions,
            &self.context_summary,
        )
    }

    pub fn compute_ref(
        format_version: u32,
        response_template: &CreateResponseBody,
        extensions: &[ExtensionConfigItem],
        context_summary: &ContextSummaryMode,
    ) -> ConfigRef {
        let payload = CanonicalConfigPayload {
            format_version,
            response_template,
            extensions,
            context_summary,
        };
        // Serialize to canonical JSON (sorted keys)
        let json_val = serde_json::to_value(&payload).expect("config payload to value");
        let canonical_bytes = canonical_json_bytes(&json_val);
        let mut hasher = Sha256::new();
        hasher.update(&canonical_bytes);
        let hash = hasher.finalize();
        ConfigRef::new(format!("sha256:{}", hex_encode(&hash)))
    }
}

fn canonical_json_bytes(val: &serde_json::Value) -> Vec<u8> {
    match val {
        serde_json::Value::Null => b"null".to_vec(),
        serde_json::Value::Bool(b) => {
            if *b {
                b"true".to_vec()
            } else {
                b"false".to_vec()
            }
        }
        serde_json::Value::Number(n) => n.to_string().into_bytes(),
        serde_json::Value::String(s) => serde_json::to_vec(s).expect("valid string json"),
        serde_json::Value::Array(arr) => {
            let mut out = Vec::new();
            out.push(b'[');
            for (i, elem) in arr.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(canonical_json_bytes(elem));
            }
            out.push(b']');
            out
        }
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by_key(|(k, _)| *k);
            let mut out = Vec::new();
            out.push(b'{');
            for (i, (k, v)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(k).expect("valid string key"));
                out.push(b':');
                out.extend(canonical_json_bytes(v));
            }
            out.push(b'}');
            out
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        write!(s, "{:02x}", b).unwrap();
    }
    s
}
