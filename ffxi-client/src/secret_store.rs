use keyring::Entry;
use tokio::runtime::Handle as RtHandle;

pub struct SecretStore;

/// The Linux `keyring` backend goes through zbus, which panics with "there is
/// no reactor running" if invoked off-thread from a Tokio reactor. Bevy
/// systems and observers run on Bevy's own thread, not inside a Tokio task,
/// so every keyring call must be wrapped in `Handle::block_on` to give zbus
/// the reactor context it requires.
impl SecretStore {
    pub fn get(runtime: &RtHandle, service: &str, account: &str) -> Option<String> {
        runtime.block_on(async {
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
        })
    }

    pub fn set(runtime: &RtHandle, service: &str, account: &str, password: &str) -> bool {
        runtime.block_on(async {
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
        })
    }

    pub fn delete(runtime: &RtHandle, service: &str, account: &str) -> bool {
        runtime.block_on(async {
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
        })
    }
}
