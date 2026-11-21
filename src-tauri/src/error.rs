use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
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

    /// Short machine-readable category, used by the UI to pick a friendly message.
    pub fn kind(&self) -> &str {
        match self {
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
        let mut st = s.serialize_struct("AppError", 3)?;
        st.serialize_field("message", &self.to_string())?;
        st.serialize_field("kind", self.kind())?;
        st.serialize_field("status", &self.status())?;
        st.end()
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        AppError::Msg(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
