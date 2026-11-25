use serde::Serialize;

/// Transient authentication-only metadata. Never include credentials or raw errors.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthProgress {
    pub state: String,
    pub kind: String,
    pub message: String,
    pub output: String,
    pub url: Option<String>,
    pub user_code: Option<String>,
}
impl AuthProgress {
    pub fn waiting(kind: &str) -> Self {
        Self {
            state: "waiting".into(),
            kind: kind.into(),
            message: "Complete the native sign-in instructions.".into(),
            output: String::new(),
            url: None,
            user_code: None,
        }
    }
}
