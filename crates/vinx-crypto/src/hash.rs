use sha2::{Digest, Sha256};

pub type Hash32 = [u8; 32];

pub fn sha256(data: &[u8]) -> Hash32 {
    let mut h = Sha256::new();
    h.update(data);
    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_empty() {
        let hash = sha256(b"");
        // SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(hash[0], 0xe3);
        assert_eq!(hash[1], 0xb0);
        assert_eq!(hash[31], 0x55);
    }

    #[test]
    fn test_sha256_deterministic() {
        let a = sha256(b"vinx ledger");
        let b = sha256(b"vinx ledger");
        assert_eq!(a, b);
    }

    #[test]
    fn test_sha256_different_inputs() {
        let a = sha256(b"hello");
        let b = sha256(b"world");
        assert_ne!(a, b);
    }
}
