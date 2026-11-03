//! Native installation credential storage for managed startup.
use kanban_app::secrets::{InstallationSecret, InstallationSecretStore};
use kanban_dto::ApiError;
use std::path::{Path, PathBuf};

pub struct NativeKeychain {
    account: String,
    data_dir: PathBuf,
}

impl NativeKeychain {
    pub fn for_data_dir(data_dir: &Path) -> Result<Self, ApiError> {
        use sha2::{Digest, Sha256};
        use std::os::unix::ffi::OsStrExt;
        let data = data_dir.canonicalize().map_err(|_| unavailable())?;
        let default = kanban_storage::paths::managed_data_dir().map_err(|_| unavailable())?;
        let account = if data == default.canonicalize().unwrap_or(default) {
            "installation".into()
        } else {
            format!("data-{:x}", Sha256::digest(data.as_os_str().as_bytes()))
        };
        Ok(Self {
            account,
            data_dir: data,
        })
    }
}

impl InstallationSecretStore for NativeKeychain {
    #[cfg(target_os = "macos")]
    fn load_or_create(&self) -> Result<InstallationSecret, ApiError> {
        use security_framework::os::macos::keychain::SecKeychain;
        use security_framework::random::SecRandom;
        const SERVICE: &str = "dev.kanban.desktop.installation";
        let keychain = SecKeychain::default().map_err(|_| unavailable())?;
        match keychain.find_generic_password(SERVICE, &self.account) {
            Ok((password, _)) => {
                let key: &[u8; 32] = password.as_ref().try_into().map_err(|_| unavailable())?;
                return Ok(InstallationSecret::from_key(key));
            }
            Err(error) if error.code() == -25300 => {}
            Err(_) => return Err(unavailable()),
        }
        let mut key = zeroize::Zeroizing::new([0u8; 32]);
        SecRandom::default()
            .copy_bytes(&mut *key)
            .map_err(|_| unavailable())?;
        // Add, never update: concurrent first starts must not rotate each other.
        let created = keychain.add_generic_password(SERVICE, &self.account, &*key);
        let (password, _) = keychain
            .find_generic_password(SERVICE, &self.account)
            .map_err(|_| unavailable())?;
        let stored: &[u8; 32] = password.as_ref().try_into().map_err(|_| unavailable())?;
        if let Err(error) = created
            && error.code() != -25299
        {
            return Err(unavailable());
        }
        Ok(InstallationSecret::from_key(stored))
    }

    #[cfg(not(target_os = "macos"))]
    fn load_or_create(&self) -> Result<InstallationSecret, ApiError> {
        file_installation_secret::load_or_create(&self.data_dir, &self.account)
    }
}

#[cfg(target_os = "macos")]
fn unavailable() -> ApiError {
    ApiError::internal("installation Keychain access is unavailable")
}

#[cfg(not(target_os = "macos"))]
fn unavailable() -> ApiError {
    ApiError::internal("installation credential storage is unavailable")
}

#[cfg(not(target_os = "macos"))]
mod file_installation_secret {
    use super::{InstallationSecret, unavailable};
    use kanban_dto::ApiError;
    use std::fs::{self, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::path::Path;

    const CREDENTIAL_DIR: &str = "installation-credential";

    pub(super) fn load_or_create(
        data_dir: &Path,
        account: &str,
    ) -> Result<InstallationSecret, ApiError> {
        let store_dir = data_dir.join(CREDENTIAL_DIR);
        ensure_store_dir(&store_dir)?;
        let path = store_dir.join(account);
        if path.exists() {
            return read_existing(&store_dir, &path);
        }
        let key = random_key()?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(mut file) => {
                file.write_all(&key).map_err(|_| unavailable())?;
                file.sync_all().map_err(|_| unavailable())?;
                Ok(InstallationSecret::from_key(&key))
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                read_existing(&store_dir, &path)
            }
            Err(_) => Err(unavailable()),
        }
    }

    fn ensure_store_dir(store_dir: &Path) -> Result<(), ApiError> {
        if store_dir.exists() {
            return require_mode(store_dir, 0o700, true);
        }
        fs::create_dir_all(store_dir).map_err(|_| unavailable())?;
        fs::set_permissions(store_dir, fs::Permissions::from_mode(0o700))
            .map_err(|_| unavailable())?;
        require_mode(store_dir, 0o700, true)
    }

    fn read_existing(store_dir: &Path, path: &Path) -> Result<InstallationSecret, ApiError> {
        require_mode(store_dir, 0o700, true)?;
        require_mode(path, 0o600, false)?;
        let mut file = File::open(path).map_err(|_| unavailable())?;
        let mut key = [0u8; 32];
        file.read_exact(&mut key).map_err(|_| unavailable())?;
        Ok(InstallationSecret::from_key(&key))
    }

    fn require_mode(path: &Path, expected: u32, directory: bool) -> Result<(), ApiError> {
        let metadata = fs::metadata(path).map_err(|_| unavailable())?;
        if metadata.is_dir() != directory {
            return Err(unavailable());
        }
        let mode = metadata.permissions().mode() & 0o777;
        if mode != expected {
            return Err(unavailable());
        }
        Ok(())
    }

    fn random_key() -> Result<[u8; 32], ApiError> {
        let mut key = [0u8; 32];
        File::open("/dev/urandom") // cspell:ignore urandom
            .map_err(|_| unavailable())?
            .read_exact(&mut key)
            .map_err(|_| unavailable())?;
        Ok(key)
    }

    #[cfg(test)]
    pub(super) fn credential_path(data_dir: &Path, account: &str) -> std::path::PathBuf {
        data_dir.join(CREDENTIAL_DIR).join(account)
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "../../kanban-transport/tests/common/http_response.rs"]
mod http_response;

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn file_store_custom_data_directory_uses_an_isolated_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        assert_ne!(store.account, "installation");
        let repeated = NativeKeychain::for_data_dir(&dir.path().canonicalize().unwrap()).unwrap();
        assert_eq!(store.account, repeated.account);
        let other = tempfile::TempDir::new().unwrap();
        assert_ne!(
            store.account,
            NativeKeychain::for_data_dir(other.path()).unwrap().account
        );
        let first = store
            .load_or_create()
            .expect("the file store provisions one credential");
        assert_eq!(
            first.expose(),
            repeated.load_or_create().unwrap().expose(),
            "reopening must not rotate credentials"
        );
    }

    #[test]
    fn file_store_refuses_looser_permissions() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        let store_dir = path.parent().unwrap();
        std::fs::set_permissions(store_dir, std::fs::Permissions::from_mode(0o701)).unwrap();
        assert!(store.load_or_create().is_err());
        std::fs::set_permissions(store_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o601)).unwrap();
        assert!(store.load_or_create().is_err());
    }

    #[test]
    fn file_store_provisions_mode_0700_directory_and_mode_0600_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        let store_dir = path.parent().unwrap();
        assert_eq!(
            store_dir.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password,
    };

    #[test]
    fn lifecycle_keychain_custom_data_directory_uses_an_isolated_native_account() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        assert_ne!(store.account, "installation");
        let repeated = NativeKeychain::for_data_dir(&dir.path().canonicalize().unwrap()).unwrap();
        assert_eq!(store.account, repeated.account);
        let other = tempfile::TempDir::new().unwrap();
        assert_ne!(
            store.account,
            NativeKeychain::for_data_dir(other.path()).unwrap().account
        );
        let _fixture = Fixture(store.account.clone());
        let first = store
            .load_or_create()
            .expect("native disposable account is usable");
        assert!(
            first.expose() == repeated.load_or_create().unwrap().expose(),
            "reopening must not rotate credentials"
        );
    }
    const SERVICE: &str = "dev.kanban.desktop.installation";
    struct Fixture(String);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = delete_generic_password(SERVICE, &self.0);
        }
    }
    #[test]
    fn native_keychain_loopback_auth_secret_exclusion_round_trip() {
        let _no_prompt =
            security_framework::os::macos::keychain::SecKeychain::disable_user_interaction()
                .unwrap();
        let fixture = Fixture(format!(
            "test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain {
            account: fixture.0.clone(),
            data_dir: dir.path().to_path_buf(),
        };
        assert!(
            generic_password(PasswordOptions::new_generic_password(SERVICE, &fixture.0)).is_err()
        );
        let first = store
            .load_or_create()
            .expect("the native store provisions one fixture credential");
        let second = store
            .load_or_create()
            .expect("the fixture credential survives reopening");
        assert!(
            first.expose() == second.expose(),
            "reopening must not rotate credentials"
        );
        assert!(!format!("{first:?}").contains(first.expose()));
        let credential = std::sync::Arc::new(second);
        let service = crate::serve_with_http(
            dir.path(),
            crate::ServiceRuntime {
                mcp_executable: dir.path().join("unused-adapter"),
                herdr_socket_root: dir.path().join("herdr"),
                installation_secret: Some(credential.clone()),
            },
            kanban_transport::loopback::LoopbackHttpConfig {
                bind: Some("127.0.0.1:0".parse().unwrap()),
            },
        )
        .unwrap();
        let address = service.http_address().unwrap();
        use std::io::Write;
        for (token, status) in [("invalid", "401"), (credential.expose(), "403")] {
            let mut client = std::net::TcpStream::connect(address).unwrap();
            client
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            write!(client, "POST /mcp HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nAuthorization: Bearer {token}\r\nX-Kanban-Capability: 1\r\nContent-Length: 2\r\n\r\n{{}}").unwrap();
            let response = http_response::read_response(&mut client).unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
            assert!(!response.contains(credential.expose()));
        }
        service.shutdown();
        assert!(std::net::TcpStream::connect(address).is_err());
        let raw =
            generic_password(PasswordOptions::new_generic_password(SERVICE, &fixture.0)).unwrap();
        assert_eq!(raw.len(), 32);
        delete_generic_password(SERVICE, &fixture.0).unwrap();
        let missing = generic_password(PasswordOptions::new_generic_password(SERVICE, &fixture.0))
            .unwrap_err();
        assert_eq!(
            missing.code(),
            -25300,
            "the fixture Keychain item must be removed"
        );
    }
}
