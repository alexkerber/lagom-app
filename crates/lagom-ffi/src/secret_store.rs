use crate::{failure, MailError, Result};
use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};
use std::{any::Any, sync::{Arc, Mutex}};

// Android supplies platform encryption; all account/password callers keep using keyring.
#[uniffi::export(callback_interface)]
pub trait SecretStore: Send + Sync {
    fn get(&self, service: String, user: String) -> Result<Option<Vec<u8>>>;
    fn set(&self, service: String, user: String, secret: Vec<u8>) -> Result<()>;
    fn delete(&self, service: String, user: String) -> Result<()>;
}

static INSTALLED: Mutex<bool> = Mutex::new(false);

#[uniffi::export]
pub fn install_secret_store(store: Box<dyn SecretStore>) -> Result<()> {
    let mut installed = INSTALLED.lock().map_err(|_| failure("lagom could not open the password saved on this device"))?;
    if *installed { return Err(failure("secret store already installed")); }
    // Installation is completed before any core is opened by the Android Application.
    keyring::set_default_credential_builder(Box::new(Builder(Arc::from(store))));
    *installed = true;
    Ok(())
}

#[cfg(target_os = "android")]
pub fn require_installed() -> Result<()> {
    if *INSTALLED.lock().map_err(|_| failure("lagom could not open the password saved on this device"))? { Ok(()) }
    else { Err(failure("Android secret store must be installed before opening mail")) }
}

struct Builder(Arc<dyn SecretStore>);
impl CredentialBuilderApi for Builder {
    fn build(&self, target: Option<&str>, service: &str, user: &str) -> keyring::Result<Box<Credential>> {
        if target.is_some() { return Err(keyring::Error::Invalid("target".into(), "unsupported".into())); }
        Ok(Box::new(Entry { store: self.0.clone(), service: service.into(), user: user.into() }))
    }
    fn as_any(&self) -> &dyn Any { self }
}
struct Entry { store: Arc<dyn SecretStore>, service: String, user: String }
fn unavailable(_: MailError) -> keyring::Error {
    keyring::Error::PlatformFailure(Box::new(std::io::Error::other("secure storage unavailable")))
}
impl CredentialApi for Entry {
    fn get_secret(&self) -> keyring::Result<Vec<u8>> {
        self.store.get(self.service.clone(), self.user.clone()).map_err(unavailable)?.ok_or(keyring::Error::NoEntry)
    }
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> {
        self.store.set(self.service.clone(), self.user.clone(), secret.to_vec()).map_err(unavailable)
    }
    fn delete_credential(&self) -> keyring::Result<()> {
        self.store.delete(self.service.clone(), self.user.clone()).map_err(unavailable)
    }
    fn as_any(&self) -> &dyn Any { self }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, sync::Mutex};
    #[derive(Default)]
    struct Fake(Mutex<HashMap<(String, String), Vec<u8>>>);
    impl SecretStore for Fake {
        fn get(&self, service: String, user: String) -> Result<Option<Vec<u8>>> { Ok(self.0.lock().unwrap().get(&(service, user)).cloned()) }
        fn set(&self, service: String, user: String, secret: Vec<u8>) -> Result<()> { self.0.lock().unwrap().insert((service, user), secret); Ok(()) }
        fn delete(&self, service: String, user: String) -> Result<()> { self.0.lock().unwrap().remove(&(service, user)); Ok(()) }
    }
    #[test]
    fn callback_backend_preserves_keyring_contract_and_isolation() {
        let builder = Builder(Arc::new(Fake::default()));
        let entry = |service, user| keyring::Entry::new_with_credential(builder.build(None, service, user).unwrap());
        let first = entry("fixture", "one");
        assert!(matches!(first.get_password(), Err(keyring::Error::NoEntry)));
        first.set_password("fixture secret").unwrap();
        assert_eq!(entry("fixture", "one").get_password().unwrap(), "fixture secret");
        assert!(entry("fixture", "two").get_password().is_err());
        assert!(entry("other", "one").get_password().is_err());
        first.delete_credential().unwrap();
        first.delete_credential().unwrap();
        assert!(first.get_password().is_err());
        assert!(builder.build(Some("target"), "fixture", "one").is_err());
    }
    struct Unavailable;
    impl SecretStore for Unavailable {
        fn get(&self, _: String, _: String) -> Result<Option<Vec<u8>>> { Err(failure("sensitive fixture")) }
        fn set(&self, _: String, _: String, _: Vec<u8>) -> Result<()> { Err(failure("sensitive fixture")) }
        fn delete(&self, _: String, _: String) -> Result<()> { Err(failure("sensitive fixture")) }
    }
    #[test]
    fn callback_failures_are_errors_without_secret_details() {
        let entry = keyring::Entry::new_with_credential(Builder(Arc::new(Unavailable)).build(None, "fixture", "one").unwrap());
        let errors = [entry.get_password().unwrap_err(), entry.set_password("fixture").unwrap_err(), entry.delete_credential().unwrap_err()];
        for error in errors {
            assert!(matches!(error, keyring::Error::PlatformFailure(_)));
            assert!(!error.to_string().contains("sensitive fixture"));
        }
    }
}
