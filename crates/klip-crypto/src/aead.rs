use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use klip_proto::wire::WireNonce;
use std::sync::atomic::{AtomicU64, Ordering};
use zeroize::{Zeroize, ZeroizeOnDrop};

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    DecryptionFailure,
    EncryptionFailure,
    InvalidKeyLength,
    InvalidNonceLength,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DecryptionFailure => write!(
                f,
                "AEAD authentication tag mismatch or corrupted ciphertext"
            ),
            Self::EncryptionFailure => write!(f, "AEAD in-place encryption failed"),
            Self::InvalidKeyLength => write!(f, "Symmetric key length must be 32 bytes"),
            Self::InvalidNonceLength => write!(f, "Nonce length must be 12 bytes"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// 256-bit symmetric key that wipes itself from memory on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SymmetricKey([u8; 32]);

impl std::fmt::Debug for SymmetricKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SymmetricKey([REDACTED])")
    }
}

impl SymmetricKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn random() -> Self {
        let mut key = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key);
        Self(key)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Monotonic 64-bit sequence counter for 96-bit nonce generation.
/// Structure: [0x00, 0x00, 0x00, 0x00 | MonotonicCounter (64-bit BE)]
pub struct NonceGenerator {
    counter: AtomicU64,
}

impl NonceGenerator {
    pub fn new(initial_counter: u64) -> Self {
        Self {
            counter: AtomicU64::new(initial_counter),
        }
    }

    pub fn next_nonce(&self) -> WireNonce {
        let cnt = self.counter.fetch_add(1, Ordering::SeqCst);
        WireNonce::new(cnt)
    }

    pub fn current_counter(&self) -> u64 {
        self.counter.load(Ordering::SeqCst)
    }
}

/// In-place zero-allocation AEAD encryption using ChaCha20-Poly1305 with empty AAD.
pub fn encrypt_payload_in_place(
    key: &SymmetricKey,
    nonce: &WireNonce,
    buffer: &mut [u8],
) -> Result<[u8; 16], CryptoError> {
    encrypt_payload_in_place_with_aad(key, nonce, b"", buffer)
}

/// In-place zero-allocation AEAD encryption using ChaCha20-Poly1305 with Associated Authenticated Data (AAD).
/// Encrypts `buffer` in-place and returns detached 16-byte Poly1305 tag.
pub fn encrypt_payload_in_place_with_aad(
    key: &SymmetricKey,
    nonce: &WireNonce,
    aad: &[u8],
    buffer: &mut [u8],
) -> Result<[u8; 16], CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    let chacha_nonce = Nonce::from_slice(nonce.as_bytes());

    let tag = cipher
        .encrypt_in_place_detached(chacha_nonce, aad, buffer)
        .map_err(|_| CryptoError::EncryptionFailure)?;

    let mut tag_bytes = [0u8; 16];
    tag_bytes.copy_from_slice(tag.as_slice());
    Ok(tag_bytes)
}

/// In-place zero-allocation AEAD decryption using ChaCha20-Poly1305 with empty AAD.
pub fn decrypt_payload_in_place(
    key: &SymmetricKey,
    nonce: &WireNonce,
    buffer: &mut [u8],
    tag: &[u8; 16],
) -> Result<(), CryptoError> {
    decrypt_payload_in_place_with_aad(key, nonce, b"", buffer, tag)
}

/// In-place zero-allocation AEAD decryption using ChaCha20-Poly1305 with Associated Authenticated Data (AAD).
/// Authenticates both `aad` and `buffer` in-place. If authentication fails, returns CryptoError::DecryptionFailure.
pub fn decrypt_payload_in_place_with_aad(
    key: &SymmetricKey,
    nonce: &WireNonce,
    aad: &[u8],
    buffer: &mut [u8],
    tag: &[u8; 16],
) -> Result<(), CryptoError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    let chacha_nonce = Nonce::from_slice(nonce.as_bytes());
    let poly_tag = Tag::from_slice(tag);

    cipher
        .decrypt_in_place_detached(chacha_nonce, aad, buffer, poly_tag)
        .map_err(|_| CryptoError::DecryptionFailure)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aead_inplace_roundtrip() {
        let key = SymmetricKey::random();
        let nonce_gen = NonceGenerator::new(1);
        let nonce = nonce_gen.next_nonce();

        let original = b"Top Secret Cross-Platform Clipboard Payload 2026";
        let mut buffer = original.to_vec();

        let tag =
            encrypt_payload_in_place(&key, &nonce, &mut buffer).expect("Encryption must succeed");

        assert_ne!(buffer.as_slice(), original);

        decrypt_payload_in_place(&key, &nonce, &mut buffer, &tag).expect("Decryption must succeed");

        assert_eq!(buffer.as_slice(), original);
    }

    #[test]
    fn test_aead_tamper_detection() {
        let key = SymmetricKey::random();
        let nonce = WireNonce::new(42);

        let mut buffer = b"sensitive credentials".to_vec();
        let tag = encrypt_payload_in_place(&key, &nonce, &mut buffer).unwrap();

        // Tamper ciphertext
        buffer[0] ^= 0xFF;

        let res = decrypt_payload_in_place(&key, &nonce, &mut buffer, &tag);
        assert_eq!(res, Err(CryptoError::DecryptionFailure));
    }
}
