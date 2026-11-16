use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ToolFailure {
    #[error("{0}")]
    InvalidArguments(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    PermissionDenied(String),
    #[error("{0}")]
    Conflict(String),
    #[error("approval denied: {0}")]
    ApprovalDenied(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Internal(String),
    #[error("{0}")]
    TaskHalted(String),
}

impl ToolFailure {
    pub fn code(&self) -> &'static str {
        match self {
            ToolFailure::InvalidArguments(_) => "invalid_arguments",
            ToolFailure::NotFound(_) => "not_found",
            ToolFailure::PermissionDenied(_) => "permission_denied",
            ToolFailure::Conflict(_) => "file_changed",
            ToolFailure::ApprovalDenied(_) => "approval_denied",
            ToolFailure::Unsupported(_) => "unsupported",
            ToolFailure::Io(_) => "io_error",
            ToolFailure::Internal(_) => "internal_error",
            ToolFailure::TaskHalted(_) => "task_halted",
        }
    }

    pub fn from_io(context: &str, err: std::io::Error) -> Self {
        ToolFailure::Io(format!("{context}: {err}"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolErrorBody {
    pub status: &'static str,
    pub code: &'static str,
    pub error: String,
}

impl From<ToolFailure> for ToolErrorBody {
    fn from(value: ToolFailure) -> Self {
        Self {
            status: "error",
            code: value.code(),
            error: value.to_string(),
        }
    }
}
