// Tokens and where they are kept. A token lives in a `Secret`: it cannot be printed, serialized
// or sent to the interface, and its bytes are overwritten when it is dropped. The store is keyed
// by `(service, account)`, as a macOS generic password is: the app keeps them in the Keychain
// (`KeychainStore`), and the tests use `MemoryStore`.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

// No `Display`, no `Serialize` and no `Clone`: the only way to the text is `expose`, at the place
// that puts it in a request header or in the store.
pub struct Secret(String);

impl Secret {
    pub fn new(text: String) -> Self {
        Self(text)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(***)")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        // Volatile writes, so the compiler does not drop them as stores nobody reads. Zeros keep
        // the string valid UTF-8 until it is freed.
        // SAFETY: every byte becomes 0, which is valid UTF-8.
        let bytes = unsafe { self.0.as_bytes_mut() };
        for byte in bytes.iter_mut() {
            // SAFETY: `byte` is a valid, aligned reference into the string's buffer.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

// What a store reports when it fails. It never carries the secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretError(pub String);

impl fmt::Display for SecretError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

pub trait SecretStore: Send + Sync {
    // `None` when nothing is stored for that service and account.
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, SecretError>;
    // Replaces what was stored.
    fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), SecretError>;
    // Deleting what is not there is not an error.
    fn delete(&self, service: &str, account: &str) -> Result<(), SecretError>;
}

// The store of the tests: nothing leaves the process.
#[derive(Default)]
pub struct MemoryStore {
    items: Mutex<HashMap<(String, String), Secret>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SecretStore for MemoryStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, SecretError> {
        let items = self.items.lock().unwrap();
        let key = (service.to_string(), account.to_string());
        Ok(items
            .get(&key)
            .map(|secret| Secret::new(secret.expose().to_string())))
    }

    fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), SecretError> {
        let mut items = self.items.lock().unwrap();
        let key = (service.to_string(), account.to_string());
        items.insert(key, Secret::new(secret.expose().to_string()));
        Ok(())
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretError> {
        let mut items = self.items.lock().unwrap();
        items.remove(&(service.to_string(), account.to_string()));
        Ok(())
    }
}

// The login Keychain, as generic passwords. The secret goes to Security.framework as bytes, never
// through `/usr/bin/security`'s arguments. The item's access list trusts the binary that made it,
// so an unsigned build that changed may make macOS ask before reading it.
pub struct KeychainStore;

// `errSecItemNotFound`.
const ITEM_NOT_FOUND: i32 = -25300;

fn keychain_failure(error: security_framework::base::Error) -> SecretError {
    SecretError(format!("Keychain error: {error}"))
}

impl SecretStore for KeychainStore {
    fn get(&self, service: &str, account: &str) -> Result<Option<Secret>, SecretError> {
        match security_framework::passwords::get_generic_password(service, account) {
            // `from_utf8` keeps the same buffer, which the `Secret` then wipes.
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => Ok(Some(Secret::new(text))),
                Err(error) => {
                    let mut bytes = error.into_bytes();
                    bytes.iter_mut().for_each(|byte| *byte = 0);
                    Err(SecretError(
                        "Keychain error: the stored token is not text".to_string(),
                    ))
                }
            },
            Err(error) if error.code() == ITEM_NOT_FOUND => Ok(None),
            Err(error) => Err(keychain_failure(error)),
        }
    }

    fn set(&self, service: &str, account: &str, secret: &Secret) -> Result<(), SecretError> {
        security_framework::passwords::set_generic_password(
            service,
            account,
            secret.expose().as_bytes(),
        )
        .map_err(keychain_failure)
    }

    fn delete(&self, service: &str, account: &str) -> Result<(), SecretError> {
        match security_framework::passwords::delete_generic_password(service, account) {
            Err(error) if error.code() != ITEM_NOT_FOUND => Err(keychain_failure(error)),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_prints_no_text() {
        let secret = Secret::new("ghp_SECRET123".to_string());
        assert_eq!(format!("{secret:?}"), "Secret(***)");
        assert_eq!(
            format!("{:#?}", Some(&secret)),
            "Some(\n    Secret(***),\n)"
        );
        assert_eq!(secret.expose(), "ghp_SECRET123");
    }

    #[test]
    fn the_memory_store_keeps_one_secret_per_service_and_account() {
        let store = MemoryStore::new();
        let service = "rama GitHub github.com";
        assert!(store.get(service, "octocat").unwrap().is_none());

        store
            .set(service, "octocat", &Secret::new("first".to_string()))
            .unwrap();
        store
            .set(service, "octocat", &Secret::new("second".to_string()))
            .unwrap();
        store
            .set(service, "hubot", &Secret::new("other".to_string()))
            .unwrap();
        store
            .set(
                "rama GitHub ghe.example.com",
                "octocat",
                &Secret::new("ghe".to_string()),
            )
            .unwrap();

        let read = |service: &str, account: &str| {
            store
                .get(service, account)
                .unwrap()
                .map(|secret| secret.expose().to_string())
        };
        assert_eq!(read(service, "octocat").as_deref(), Some("second"));
        assert_eq!(read(service, "hubot").as_deref(), Some("other"));
        assert_eq!(
            read("rama GitHub ghe.example.com", "octocat").as_deref(),
            Some("ghe")
        );

        store.delete(service, "octocat").unwrap();
        store.delete(service, "octocat").unwrap();
        assert_eq!(read(service, "octocat"), None);
        assert_eq!(read(service, "hubot").as_deref(), Some("other"));
    }

    // By hand only: it writes to the login Keychain of whoever runs it, under a service nobody else
    // uses, and deletes it again. `cargo test --lib keychain -- --ignored`.
    #[test]
    #[ignore = "uses the real login Keychain; run by hand"]
    fn the_keychain_store_keeps_replaces_and_deletes_a_token() {
        let service = format!("rama GitHub test-{}.invalid", std::process::id());
        let store = KeychainStore;
        assert!(store.get(&service, "octocat").unwrap().is_none());
        store
            .set(&service, "octocat", &Secret::new("first".to_string()))
            .unwrap();
        store
            .set(&service, "octocat", &Secret::new("second".to_string()))
            .unwrap();
        let read = store.get(&service, "octocat").unwrap().unwrap();
        assert_eq!(read.expose(), "second");
        store.delete(&service, "octocat").unwrap();
        store.delete(&service, "octocat").unwrap();
        assert!(store.get(&service, "octocat").unwrap().is_none());
    }
}
