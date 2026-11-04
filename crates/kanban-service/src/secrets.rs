//! Native installation credential storage for managed startup.
use kanban_app::secrets::{InstallationSecret, InstallationSecretStore};
use kanban_dto::ApiError;
use std::path::{Path, PathBuf};

pub struct NativeKeychain {
    account: String,
    #[cfg(not(target_os = "macos"))]
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
            #[cfg(not(target_os = "macos"))]
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
    ApiError::internal("credential storage failed")
}

#[cfg(not(target_os = "macos"))]
mod file_installation_secret {
    use super::{InstallationSecret, unavailable};
    use kanban_dto::ApiError;
    use std::fs::{self, DirBuilder, File, OpenOptions};
    use std::io::{Read, Write};
    use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
    use std::os::unix::io::AsRawFd;
    use std::path::Path;

    const CREDENTIAL_DIR: &str = "installation-credential";
    const KEY_LEN: usize = 32;

    pub(super) fn load_or_create(
        data_dir: &Path,
        account: &str,
    ) -> Result<InstallationSecret, ApiError> {
        let store_dir = data_dir.join(CREDENTIAL_DIR);
        ensure_store_dir(&store_dir)?;
        let path = store_dir.join(account);
        match path.symlink_metadata() {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(unavailable()),
            Ok(_) => read_existing(&store_dir, &path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = random_key()?;
                install_new_key(&store_dir, &path, &key)
            }
            Err(_) => Err(unavailable()),
        }
    }

    fn ensure_store_dir(store_dir: &Path) -> Result<(), ApiError> {
        match store_dir.symlink_metadata() {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(unavailable());
                }
                let dir = open_store_directory(store_dir)?;
                drop(dir);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match DirBuilder::new().mode(0o700).create(store_dir) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(unavailable()),
                }
                let dir = open_store_directory(store_dir)?;
                drop(dir);
                Ok(())
            }
            Err(_) => Err(unavailable()),
        }
    }

    fn open_store_directory(store_dir: &Path) -> Result<File, ApiError> {
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(store_dir)
            .map_err(|_| unavailable())?;
        verify_directory_fd(&dir)?;
        Ok(dir)
    }

    fn verify_directory_fd(dir: &File) -> Result<(), ApiError> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(dir.as_raw_fd(), &mut stat) } != 0 {
            return Err(unavailable());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err(unavailable());
        }
        if stat.st_mode & 0o777 != 0o700 {
            return Err(unavailable());
        }
        if stat.st_uid != unsafe { libc::geteuid() } {
            return Err(unavailable());
        }
        Ok(())
    }

    fn sync_store_directory(store_dir: &Path) -> Result<(), ApiError> {
        let dir = open_store_directory(store_dir)?;
        dir.sync_all().map_err(|_| unavailable())
    }

    fn reject_non_regular_file_metadata(metadata: &fs::Metadata) -> Result<(), ApiError> {
        if metadata.file_type().is_symlink() || metadata.file_type().is_fifo() || !metadata.is_file()
        {
            return Err(unavailable());
        }
        Ok(())
    }

    fn partial_temp_path(store_dir: &Path, path: &Path) -> Result<std::path::PathBuf, ApiError> {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("credential");
        let mut suffix = [0u8; 8];
        let filled = unsafe { libc::getrandom(suffix.as_mut_ptr().cast(), suffix.len(), 0) };
        if filled != suffix.len() as isize {
            return Err(unavailable());
        }
        let suffix_hex = suffix.iter().fold(String::new(), |mut encoded, byte| {
            use std::fmt::Write;
            let _ = write!(encoded, "{byte:02x}");
            encoded
        });
        Ok(store_dir.join(format!(".{name}.partial.{suffix_hex}")))
    }

    fn publish_new_key(temp: &Path, path: &Path) -> Result<(), ApiError> {
        if std::env::var_os("KANBAN_TEST_RENAMEAT2_EINVAL").is_some() {
            return publish_via_hard_link(temp, path);
        }
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let temp_c = CString::new(temp.as_os_str().as_bytes()).map_err(|_| unavailable())?;
        let path_c = CString::new(path.as_os_str().as_bytes()).map_err(|_| unavailable())?;
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                temp_c.as_ptr(),
                libc::AT_FDCWD,
                path_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::EEXIST) => Err(unavailable()),
            Some(libc::EINVAL) => publish_via_hard_link(temp, path),
            _ => Err(unavailable()),
        }
    }

    fn publish_via_hard_link(temp: &Path, path: &Path) -> Result<(), ApiError> {
        std::fs::hard_link(temp, path).map_err(|_| unavailable())?;
        std::fs::remove_file(temp).map_err(|_| unavailable())?;
        Ok(())
    }

    fn install_new_key(
        store_dir: &Path,
        path: &Path,
        key: &zeroize::Zeroizing<[u8; 32]>,
    ) -> Result<InstallationSecret, ApiError> {
        let temp = partial_temp_path(store_dir, path)?;
        let result: Result<InstallationSecret, ApiError> = (|| {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temp)
                .map_err(|_| unavailable())?;
            verify_regular_file(&file, 0o600)?;
            let mut file = file;
            file.write_all(key.as_ref()).map_err(|_| unavailable())?;
            file.sync_all().map_err(|_| unavailable())?;
            verify_regular_file(&file, 0o600)?;
            drop(file);
            publish_new_key(&temp, path)?;
            sync_store_directory(store_dir)?;
            Ok(InstallationSecret::from_key(key))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        match result {
            Ok(secret) => Ok(secret),
            Err(_) => match path.symlink_metadata() {
                Ok(metadata) if !metadata.file_type().is_symlink() => {
                    read_existing(store_dir, path)
                }
                _ => Err(unavailable()),
            },
        }
    }

    fn read_existing(store_dir: &Path, path: &Path) -> Result<InstallationSecret, ApiError> {
        ensure_store_dir(store_dir)?;
        let metadata = path.symlink_metadata().map_err(|_| unavailable())?;
        reject_non_regular_file_metadata(&metadata)?;
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| unavailable())?;
        verify_regular_file(&file, 0o600)?;
        if file.metadata().map_err(|_| unavailable())?.size() as usize != KEY_LEN {
            return Err(unavailable());
        }
        let mut key = zeroize::Zeroizing::new([0u8; 32]);
        file.read_exact(&mut *key).map_err(|_| unavailable())?;
        Ok(InstallationSecret::from_key(&key))
    }

    fn verify_regular_file(file: &File, expected_mode: u32) -> Result<(), ApiError> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } != 0 {
            return Err(unavailable());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(unavailable());
        }
        if stat.st_mode & 0o777 != expected_mode {
            return Err(unavailable());
        }
        if stat.st_uid != unsafe { libc::geteuid() } {
            return Err(unavailable());
        }
        if stat.st_nlink != 1 {
            return Err(unavailable());
        }
        Ok(())
    }

    fn random_key() -> Result<zeroize::Zeroizing<[u8; 32]>, ApiError> {
        let mut key = zeroize::Zeroizing::new([0u8; 32]);
        let filled = unsafe { libc::getrandom(key.as_mut_ptr().cast(), KEY_LEN, 0) };
        if filled != KEY_LEN as isize {
            return Err(unavailable());
        }
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

    fn credentials_match(first: &InstallationSecret, second: &InstallationSecret) -> bool {
        first.expose().as_bytes() == second.expose().as_bytes()
    }

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
        assert!(
            credentials_match(&first, &repeated.load_or_create().unwrap()),
            "reopening must not rotate credentials"
        );
        assert!(!format!("{first:?}").contains(first.expose()));
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

    #[test]
    fn file_store_refuses_symlinked_credential_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        std::fs::remove_file(&path).unwrap();
        let real = dir.path().join("real-credential");
        std::fs::write(&real, [9u8; 32]).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&real, &path).unwrap();
        assert!(store.load_or_create().is_err());
    }

    #[test]
    fn file_store_refuses_hard_linked_credential_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        let alias = path.parent().unwrap().join("alias-credential");
        std::fs::hard_link(&path, &alias).unwrap();
        assert!(store.load_or_create().is_err());
        let _ = std::fs::remove_file(alias);
    }

    #[test]
    fn file_store_refuses_oversized_credential_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        std::fs::write(&path, [1u8; 64]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store.load_or_create().is_err());
    }

    #[test]
    fn file_store_refuses_fifo_at_credential_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        std::fs::remove_file(&path).unwrap();
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let path_c = CString::new(path.as_os_str().as_bytes()).unwrap();
        let rc = unsafe { libc::mkfifo(path_c.as_ptr(), 0o600) };
        assert_eq!(rc, 0);
        assert!(store.load_or_create().is_err());
    }

    #[test]
    fn file_store_provisions_when_renameat2_is_unavailable() {
        unsafe { std::env::set_var("KANBAN_TEST_RENAMEAT2_EINVAL", "1") };
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        let first = store.load_or_create().expect("hard-link publish works");
        unsafe { std::env::remove_var("KANBAN_TEST_RENAMEAT2_EINVAL") };
        let second = store.load_or_create().expect("second read");
        assert!(credentials_match(&first, &second));
    }

    #[test]
    fn file_store_refuses_symlinked_store_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let store_dir = dir.path().join("installation-credential");
        let real = dir.path().join("real-store");
        std::fs::rename(&store_dir, &real).unwrap();
        std::os::unix::fs::symlink(&real, &store_dir).unwrap();
        assert!(store.load_or_create().is_err());
    }

    #[test]
    fn file_store_repeated_load_or_create_preserves_first_key() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        let first = store.load_or_create().expect("first provision");
        let second = store.load_or_create().expect("second read");
        assert!(
            credentials_match(&first, &second),
            "a second load_or_create must not rotate the installation key"
        );
    }

    const UMASK_ISOLATED_TEST: &str =
        "secrets::tests::file_store_honours_0700_and_0600_when_umask_is_zero_isolated";

    #[test]
    fn file_store_honours_0700_and_0600_when_umask_is_zero() {
        let exe = std::env::current_exe().expect("test executable");
        let output = std::process::Command::new(exe)
            .env("KANBAN_FILE_STORE_UMASK_TEST", "1")
            .args([
                "--exact",
                UMASK_ISOLATED_TEST,
                "--nocapture",
                "--test-threads=1",
            ])
            .output()
            .expect("spawn isolated umask test");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        assert!(
            output.status.success(),
            "isolated umask test failed:\n{combined}",
        );
        assert!(
            combined.contains("running 1 test"),
            "child must run exactly one test, output:\n{combined}",
        );
        assert!(
            combined.contains("test result: ok. 1 passed"),
            "child must report exactly one passing test, output:\n{combined}",
        );
    }

    #[test]
    fn file_store_honours_0700_and_0600_when_umask_is_zero_isolated() {
        if std::env::var_os("KANBAN_FILE_STORE_UMASK_TEST").is_none() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        let previous = unsafe { libc::umask(0) };
        let result = store.load_or_create();
        unsafe {
            libc::umask(previous);
        }
        result.unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        let store_dir = path.parent().unwrap();
        assert_eq!(
            store_dir.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn file_store_rejects_truncated_credential_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        std::fs::write(&path, [7u8; 16]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(store.load_or_create().is_err());
    }

    #[test]
    fn file_store_reports_credential_storage_failed() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = NativeKeychain::for_data_dir(dir.path()).unwrap();
        store.load_or_create().unwrap();
        let path = file_installation_secret::credential_path(&store.data_dir, &store.account);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let error = store.load_or_create().unwrap_err();
        assert_eq!(error.message, "credential storage failed");
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
