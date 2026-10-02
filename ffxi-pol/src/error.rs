use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// A service answered with a non-zero status. Both forms are kept: the
    /// raw byte is what the server actually said, and the code is what the
    /// Viewer would have surfaced for it, so a report names the same number
    /// the Viewer's own message would.
    #[error("{service} refused {transaction}: {meaning} (status 0x{status:02x}, code {code})")]
    Status {
        service: &'static str,
        transaction: &'static str,
        status: u8,
        code: i32,
        meaning: &'static str,
    },

    #[error("{0}")]
    Protocol(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub fn protocol(what: impl Into<String>) -> Self {
        Self::Protocol(what.into())
    }
}
