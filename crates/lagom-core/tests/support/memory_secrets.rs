use keyring::credential::{Credential, CredentialApi, CredentialBuilderApi};
use std::{any::Any, collections::HashMap, sync::{Arc, Mutex}};

// The test binary replaces the backend before any credential operation; it never opens a keychain.
#[derive(Clone, Default, Debug)]
struct MemorySecrets(Arc<Mutex<HashMap<String, Vec<u8>>>>);
#[derive(Debug)]
struct MemoryEntry { store: MemorySecrets, id: String, user: String }

thread_local! { static DENY_DELETE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) }; }
#[allow(dead_code)] // Only the rollback test injects a failure.
pub fn fail_next_delete(user: &str) { DENY_DELETE.with(|key| key.replace(Some(user.into()))); }

impl CredentialBuilderApi for MemorySecrets {
    fn build(&self, _: Option<&str>, service: &str, user: &str) -> keyring::Result<Box<Credential>> {
        Ok(Box::new(MemoryEntry { store: self.clone(), user: user.into(), id: format!("{service}/{user}") }))
    }
    fn as_any(&self) -> &dyn Any { self }
}
impl CredentialApi for MemoryEntry {
    fn set_secret(&self, secret: &[u8]) -> keyring::Result<()> { self.store.0.lock().unwrap().insert(self.id.clone(), secret.to_vec()); Ok(()) }
    fn get_secret(&self) -> keyring::Result<Vec<u8>> { self.store.0.lock().unwrap().get(&self.id).cloned().ok_or(keyring::Error::NoEntry) }
    fn delete_credential(&self) -> keyring::Result<()> {
        if DENY_DELETE.with(|key| key.borrow_mut().take_if(|id| *id == self.user).is_some()) {
            return Err(keyring::Error::Invalid("delete".into(), "fixture failure".into()));
        }
        self.store.0.lock().unwrap().remove(&self.id);
        Ok(())
    }
    fn as_any(&self) -> &dyn Any { self }
}
pub fn install() {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| keyring::set_default_credential_builder(Box::<MemorySecrets>::default()));
}
