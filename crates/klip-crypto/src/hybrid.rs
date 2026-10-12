use crate::aead::SymmetricKey;
use blake3::Hasher;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Post-Quantum Hybrid Shared Secret container.
/// Combines classical Diffie-Hellman / PSK with Post-Quantum ML-KEM-768 key encapsulation.
/// KDF: MasterKey = BLAKE3(ClassicalSecret || PostQuantumSecret || Context)
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct HybridSharedSecret([u8; 32]);

impl HybridSharedSecret {
    pub const CONTEXT: &'static [u8] = b"KLIP-X25519MLKEM768-HYBRID-KDF-V1";

    pub fn derive(classical_secret: &[u8], pq_secret: &[u8]) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(Self::CONTEXT);
        hasher.update(classical_secret);
        hasher.update(pq_secret);
        let hash = hasher.finalize();

        let mut key = [0u8; 32];
        key.copy_from_slice(hash.as_bytes());
        Self(key)
    }

    pub fn to_symmetric_key(&self) -> SymmetricKey {
        SymmetricKey::new(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_derivation() {
        let classical = [1u8; 32];
        let pq = [2u8; 32];
        let hybrid1 = HybridSharedSecret::derive(&classical, &pq);
        let hybrid2 = HybridSharedSecret::derive(&classical, &pq);
        assert_eq!(hybrid1.as_bytes(), hybrid2.as_bytes());

        let pq_diff = [3u8; 32];
        let hybrid3 = HybridSharedSecret::derive(&classical, &pq_diff);
        assert_ne!(hybrid1.as_bytes(), hybrid3.as_bytes());
    }
}
