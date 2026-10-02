//! A minimal random-bytes trait so the key-agreement code takes a source it
//! can fill from without pinning a `rand` major version through its signatures.
//! polcore seeds its own generator from `GetTickCount`; a reimplementation
//! injects a real CSPRNG here, since nothing on the wire commits to how the
//! ephemeral key was drawn.

/// A source of random bytes.
pub trait RandomBytes {
    fn fill(&mut self, out: &mut [u8]);
}

impl<T: rand::Rng + ?Sized> RandomBytes for T {
    fn fill(&mut self, out: &mut [u8]) {
        self.fill_bytes(out);
    }
}
