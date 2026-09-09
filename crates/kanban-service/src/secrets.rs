//! Native-only installation storage; there is no file or SQLite fallback.
use kanban_app::secrets::{InstallationSecret, InstallationSecretStore};
use kanban_dto::ApiError;

pub struct NativeKeychain {
    account: String,
}
impl Default for NativeKeychain {
    fn default() -> Self {
        Self {
            account: "installation".into(),
        }
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
        let _ = &self.account;
        Err(unavailable())
    }
}
fn unavailable() -> ApiError {
    ApiError::internal("installation Keychain access is unavailable")
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password,
    };
    const SERVICE: &str = "dev.kanban.desktop.installation";
    struct Fixture(String);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = delete_generic_password(SERVICE, &self.0);
        }
    }
    #[test]
    fn native_keychain_secret_exclusion_round_trip() {
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
