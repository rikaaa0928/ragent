use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! define_string_id {
    ($name:ident, $prefix:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new(val: impl Into<String>) -> Self {
                Self(val.into())
            }

            pub fn generate() -> Self {
                let uuid = Uuid::now_v7();
                Self(format!("{}_{}", $prefix, uuid.simple()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

define_string_id!(SessionId, "sess");
define_string_id!(ActivationId, "act");
define_string_id!(TurnId, "turn");
define_string_id!(EventId, "evt");
define_string_id!(CommandId, "cmd");
define_string_id!(WorkspaceId, "ws");
define_string_id!(ConfigRef, "cfg");
define_string_id!(PermissionSnapshotRef, "perm");

macro_rules! define_seq_id {
    ($name:ident) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            PartialOrd,
            Ord,
            Hash,
            Default,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub const ZERO: Self = Self(0);

            pub const fn new(val: u64) -> Self {
                Self(val)
            }

            pub const fn as_u64(&self) -> u64 {
                self.0
            }

            pub const fn next(&self) -> Self {
                Self(self.0 + 1)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<u64> for $name {
            fn from(v: u64) -> Self {
                Self(v)
            }
        }

        impl From<$name> for u64 {
            fn from(v: $name) -> Self {
                v.0
            }
        }

        impl std::ops::Add<u64> for $name {
            type Output = Self;
            fn add(self, rhs: u64) -> Self::Output {
                Self(self.0 + rhs)
            }
        }

        impl std::ops::Sub<u64> for $name {
            type Output = Self;
            fn sub(self, rhs: u64) -> Self::Output {
                Self(self.0 - rhs)
            }
        }
    };
}

define_seq_id!(BatchSeq);
define_seq_id!(LocalItemSeq);
define_seq_id!(EventSeq);
define_seq_id!(ContextPos);

/// 验证 Session ID 是否合法（防止路径遍历和非法字符）
pub fn validate_session_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Session ID 不能为空".into());
    }
    if id.len() > 128 {
        return Err("Session ID 长度不能超过 128 字符".into());
    }
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err("Session ID 包含非法路径字符".into());
    }
    let is_valid_char =
        |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == ':';
    if !id.chars().all(is_valid_char) {
        return Err("Session ID 包含非法字符，仅支持字母、数字、_、-、.、:".into());
    }
    Ok(())
}
