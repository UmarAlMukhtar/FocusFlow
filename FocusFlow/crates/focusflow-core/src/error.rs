use serde::{Deserialize, Serialize};

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    InvalidId,
    InvalidPath,
    InvalidDocument,
    UnsupportedSchema,
    UnsupportedCapability,
    RevisionConflict,
    Busy,
    NotFound,
    Io,
    EncoderFailed,
    Cancelled,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreError {
    pub code: ErrorCode,
    pub message: String,
    pub field: Option<String>,
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CoreError {}
