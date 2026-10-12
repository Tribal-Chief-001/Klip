use std::ops::{Deref, DerefMut};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A memory-hardened buffer that is pinned into RAM via `mlock`
/// and guaranteed to be securely zeroized on drop.
pub struct PinnedBuffer {
    data: Vec<u8>,
    pinned: bool,
}

impl PinnedBuffer {
    pub fn new(size: usize) -> Self {
        let mut data = vec![0u8; size];
        let pinned = lock_memory(data.as_mut_slice());
        Self { data, pinned }
    }

    pub fn from_slice(slice: &[u8]) -> Self {
        let mut buf = Self::new(slice.len());
        buf.data.copy_from_slice(slice);
        buf
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }
}

impl Deref for PinnedBuffer {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for PinnedBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl Drop for PinnedBuffer {
    fn drop(&mut self) {
        // Zeroize contents before unlocking memory
        self.data.zeroize();
        if self.pinned {
            unlock_memory(&mut self.data);
        }
    }
}

impl ZeroizeOnDrop for PinnedBuffer {}

#[cfg(unix)]
fn lock_memory(slice: &mut [u8]) -> bool {
    if slice.is_empty() {
        return false;
    }
    unsafe {
        libc::mlock(slice.as_ptr() as *const libc::c_void, slice.len()) == 0
    }
}

#[cfg(not(unix))]
fn lock_memory(_slice: &mut [u8]) -> bool {
    false
}

#[cfg(unix)]
fn unlock_memory(slice: &mut [u8]) {
    if !slice.is_empty() {
        unsafe {
            libc::munlock(slice.as_ptr() as *const libc::c_void, slice.len());
        }
    }
}

#[cfg(not(unix))]
fn unlock_memory(_slice: &mut [u8]) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pinned_buffer_lifecycle() {
        let mut buf = PinnedBuffer::new(64);
        buf[0] = 42;
        buf[63] = 99;
        assert_eq!(buf[0], 42);
        assert_eq!(buf[63], 99);
        drop(buf); // verify no segfault or leak on mlock/munlock/zeroize
    }
}
