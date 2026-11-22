use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A failure the UI says in the person's language (ui/js/errors.js): `kind` names it,
    /// `params` fill its sentence, `message` is the English text kept under Details.
    #[error("{message}")]
    Coded { kind: &'static str, params: Value, message: String },
    #[error("{0}")]
    Msg(String),
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{message}")]
    Api {
        status: u16,
        kind: String,
        message: String,
    },
    #[error("cancelled")]
    Cancelled,
}

impl AppError {
    pub fn msg(s: impl Into<String>) -> Self {
        AppError::Msg(s.into())
    }

    /// A coded failure (see `Coded`): `params` a JSON object, or null when the sentence needs none.
    pub fn coded(kind: &'static str, params: Value, message: impl Into<String>) -> Self {
        AppError::Coded { kind, params, message: message.into() }
    }

    /// What fills the UI's sentence: a coded failure's params, an I/O failure's kind of error.
    pub fn params(&self) -> Value {
        match self {
            AppError::Coded { params, .. } => params.clone(),
            AppError::Io(e) => json!({"io": format!("{:?}", e.kind())}),
            _ => Value::Null,
        }
    }

    /// Short machine-readable category, used by the UI to pick a friendly message.
    pub fn kind(&self) -> &str {
        match self {
            AppError::Coded { kind, .. } => kind,
            AppError::Msg(_) => "error",
            AppError::Db(_) => "database",
            AppError::Io(_) => "io",
            AppError::Http(_) => "network",
            AppError::Json(_) => "json",
            AppError::Api { kind, .. } => kind.as_str(),
            AppError::Cancelled => "cancelled",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            AppError::Api { status, .. } => *status,
            _ => 0,
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("AppError", 4)?;
        st.serialize_field("message", &self.to_string())?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("status", &self.status())?;
        match self.params() {
            Value::Null => st.skip_field("params")?,
            p => st.serialize_field("params", &p)?,
        }
        st.end()
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        AppError::Msg(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

/// A notice (not a failure) the UI says in the person's language: the fields of a coded failure.
pub fn coded_notice(kind: &'static str, params: Value, message: impl Into<String>) -> Value {
    json!({"kind": kind, "params": params, "message": message.into()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coded_failure_carries_its_params_and_others_only_what_they_have() {
        let e = AppError::coded("folder_not_found", json!({"path": "C:/x"}), "Folder not found: C:/x");
        assert_eq!(serde_json::to_value(&e).unwrap(), json!({"message": "Folder not found: C:/x", "kind": "folder_not_found", "status": 0, "params": {"path": "C:/x"}}));
        assert_eq!(serde_json::to_value(AppError::coded("link_not_web", Value::Null, "x")).unwrap(), json!({"message": "x", "kind": "link_not_web", "status": 0}));
        assert_eq!(serde_json::to_value(AppError::msg("plain")).unwrap(), json!({"message": "plain", "kind": "error", "status": 0}));
        let io = AppError::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(serde_json::to_value(&io).unwrap()["params"], json!({"io": "PermissionDenied"}));
    }
}
