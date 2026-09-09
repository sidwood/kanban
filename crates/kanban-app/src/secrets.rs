//! Installation credentials never implement a transport or persistence DTO.
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use kanban_dto::ApiError;
use zeroize::Zeroizing;

pub struct InstallationSecret(Zeroizing<String>);
impl std::fmt::Debug for InstallationSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstallationSecret([REDACTED])")
    }
}
impl InstallationSecret {
    pub fn from_key(key: &[u8; 32]) -> Self {
        Self(Zeroizing::new(format!(
            "kanban_{}",
            URL_SAFE_NO_PAD.encode(key)
        )))
    }
    /// Only the native authentication and redaction adapters need these bytes.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

pub trait InstallationSecretStore: Send + Sync {
    fn load_or_create(&self) -> Result<InstallationSecret, ApiError>;
}
