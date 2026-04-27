use keyring::Entry;

pub struct SecretStore;

impl SecretStore {
    pub fn get(service: &str, account: &str) -> Option<String> {
        match Entry::new(service, account) {
            Ok(entry) => match entry.get_password() {
                Ok(pw) => Some(pw),
                Err(keyring::Error::NoEntry) => None,
                Err(e) => {
                    tracing::warn!(service, account, error = %e, "keyring: get failed");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(service, account, error = %e, "keyring: open failed");
                None
            }
        }
    }

    pub fn set(service: &str, account: &str, password: &str) -> bool {
        match Entry::new(service, account) {
            Ok(entry) => match entry.set_password(password) {
                Ok(()) => true,
                Err(e) => {
                    tracing::warn!(service, account, error = %e, "keyring: set failed");
                    false
                }
            },
            Err(e) => {
                tracing::warn!(service, account, error = %e, "keyring: open failed");
                false
            }
        }
    }

    pub fn delete(service: &str, account: &str) -> bool {
        match Entry::new(service, account) {
            Ok(entry) => match entry.delete_credential() {
                Ok(()) => true,

                Err(keyring::Error::NoEntry) => true,
                Err(e) => {
                    tracing::warn!(service, account, error = %e, "keyring: delete failed");
                    false
                }
            },
            Err(e) => {
                tracing::warn!(service, account, error = %e, "keyring: open failed");
                false
            }
        }
    }
}
