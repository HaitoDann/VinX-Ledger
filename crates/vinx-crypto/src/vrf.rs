// ECVRF-EDWARDS25519-SHA512-TAI — RFC 9381, Suite 0x03
//
// Suite constants:
//   suite_string = 0x03
//   hash         = SHA-512  (hashlen = 64)
//   cLen         = 16       (challenge length in bytes)
//   cofactor     = 8        (Edwards25519)
//   ptLen        = 32       (compressed Edwards point)
//   qLen         = 32       (scalar size)
//   pi length    = ptLen + cLen + qLen = 80 bytes
//   beta length  = hashlen = 64 bytes

use curve25519_dalek::{
    constants::ED25519_BASEPOINT_POINT,
    edwards::{CompressedEdwardsY, EdwardsPoint},
    scalar::Scalar,
};
use sha2::{Digest, Sha512};

use crate::CryptoError;

const SUITE: u8 = 0x03;
const C_LEN: usize = 16;
const PT_LEN: usize = 32;
const Q_LEN: usize = 32;

/// The length of a serialised VRF proof: Gamma(32) || c(16) || s(32) = 80 bytes.
pub const VRF_PROOF_LEN: usize = PT_LEN + C_LEN + Q_LEN;
/// The length of the VRF output (beta): SHA-512 = 64 bytes.
pub const VRF_OUTPUT_LEN: usize = 64;

/// A 32-byte random seed that acts as the VRF secret key.
#[derive(Clone)]
pub struct VrfSecretKey(pub [u8; 32]);

/// The compressed Ed25519 public key corresponding to a `VrfSecretKey`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VrfPublicKey(pub [u8; PT_LEN]);

/// An 80-byte VRF proof: `Gamma || c || s`.
#[derive(Clone, Debug)]
pub struct VrfProof(pub [u8; VRF_PROOF_LEN]);

/// The 64-byte deterministic VRF output (beta).
pub type VrfOutput = [u8; VRF_OUTPUT_LEN];

// ─── key expansion ────────────────────────────────────────────────────────────

fn expand_sk(sk: &[u8; 32]) -> ([u8; 64], Scalar, VrfPublicKey) {
    let hash = Sha512::digest(sk);
    let mut hash_bytes = [0u8; 64];
    hash_bytes.copy_from_slice(&hash);

    // Clamp and interpret the low 32 bytes as the private scalar (RFC 8032 §5.1.5).
    let mut x_bytes = [0u8; 32];
    x_bytes.copy_from_slice(&hash_bytes[..32]);
    x_bytes[0] &= 248;
    x_bytes[31] &= 127;
    x_bytes[31] |= 64;
    let x = Scalar::from_bytes_mod_order(x_bytes);

    let pk_point = ED25519_BASEPOINT_POINT * x;
    let pk = VrfPublicKey(pk_point.compress().0);

    (hash_bytes, x, pk)
}

// ─── hash-to-curve (TAI) ──────────────────────────────────────────────────────

fn hash_to_curve(pk: &VrfPublicKey, alpha: &[u8]) -> EdwardsPoint {
    for ctr in 0u8..=255 {
        let mut h = Sha512::new();
        h.update([SUITE, 0x01]);
        h.update(pk.0);
        h.update(alpha);
        h.update([ctr]);
        let digest = h.finalize();

        // Use the first 32 bytes; clear the top 2 bits so y < p (RFC 9381 §5.4.1.1).
        let mut candidate = [0u8; 32];
        candidate.copy_from_slice(&digest[..32]);
        candidate[31] &= 0x3F;

        if let Some(pt) = CompressedEdwardsY(candidate).decompress() {
            if !pt.is_small_order() {
                // Multiply by the cofactor to clear the small-order torsion component.
                return pt.mul_by_cofactor();
            }
        }
    }
    // Probability of reaching here is 2^-256 — treat as a hard failure.
    panic!("ECVRF hash_to_curve: no valid point found after 256 attempts");
}

// ─── nonce generation (RFC 8032 style) ────────────────────────────────────────

fn nonce_generation(sk_hash: &[u8; 64], h_compressed: &[u8; 32]) -> Scalar {
    let trunc = &sk_hash[32..64];
    let mut k_hash = [0u8; 64];
    k_hash.copy_from_slice(
        &Sha512::new()
            .chain_update(trunc)
            .chain_update(h_compressed)
            .finalize(),
    );
    Scalar::from_bytes_mod_order_wide(&k_hash)
}

// ─── hash_points ──────────────────────────────────────────────────────────────

fn hash_points(
    h: &EdwardsPoint,
    gamma: &EdwardsPoint,
    u: &EdwardsPoint,
    v: &EdwardsPoint,
) -> Scalar {
    let digest = Sha512::new()
        .chain_update([SUITE, 0x02])
        .chain_update(h.compress().0)
        .chain_update(gamma.compress().0)
        .chain_update(u.compress().0)
        .chain_update(v.compress().0)
        .chain_update([0x00])
        .finalize();
    // Take the first cLen bytes as little-endian challenge, zero-padded to 32.
    let mut c_padded = [0u8; 32];
    c_padded[..C_LEN].copy_from_slice(&digest[..C_LEN]);
    Scalar::from_bytes_mod_order(c_padded)
}

// ─── proof_to_hash ────────────────────────────────────────────────────────────

fn proof_to_hash_inner(gamma: &EdwardsPoint) -> VrfOutput {
    let cofactor_gamma = gamma.mul_by_cofactor();
    let mut out = [0u8; VRF_OUTPUT_LEN];
    out.copy_from_slice(
        &Sha512::new()
            .chain_update([SUITE, 0x03])
            .chain_update(cofactor_gamma.compress().0)
            .chain_update([0x00])
            .finalize(),
    );
    out
}

// ─── public API ───────────────────────────────────────────────────────────────

impl VrfSecretKey {
    /// Generate a random VRF secret key.
    pub fn generate() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// Derive the corresponding public key.
    pub fn public_key(&self) -> VrfPublicKey {
        let (_, _, pk) = expand_sk(&self.0);
        pk
    }

    /// `ECVRF_prove` — RFC 9381 §5.1.
    /// Returns an 80-byte proof π = Γ || c || s.
    pub fn prove(&self, alpha: &[u8]) -> VrfProof {
        let (sk_hash, x, pk) = expand_sk(&self.0);

        let h = hash_to_curve(&pk, alpha);
        let h_compressed = h.compress().0;

        let gamma = h * x;

        let k = nonce_generation(&sk_hash, &h_compressed);
        let u = ED25519_BASEPOINT_POINT * k;
        let v = h * k;

        let c = hash_points(&h, &gamma, &u, &v);

        let s = k + c * x;

        // Serialise pi = Gamma(32) || c(16) || s(32).
        let mut pi = [0u8; VRF_PROOF_LEN];
        pi[..32].copy_from_slice(&gamma.compress().0);
        // c is a 32-byte scalar internally; only the first cLen bytes are meaningful.
        let c_bytes = c.to_bytes();
        pi[32..48].copy_from_slice(&c_bytes[..C_LEN]);
        pi[48..80].copy_from_slice(&s.to_bytes());

        VrfProof(pi)
    }

    /// Convenience: prove and immediately extract the VRF output.
    pub fn evaluate(&self, alpha: &[u8]) -> (VrfProof, VrfOutput) {
        let proof = self.prove(alpha);
        let output = proof_to_hash(&proof).expect("proof generated locally is always valid");
        (proof, output)
    }
}

/// Extract the VRF output (beta) from a proof **without** verifying it.
/// Use `verify` if you need proof authenticity.
pub fn proof_to_hash(proof: &VrfProof) -> Result<VrfOutput, CryptoError> {
    let gamma = CompressedEdwardsY(proof.0[..32].try_into().unwrap())
        .decompress()
        .ok_or(CryptoError::InvalidSignature)?;
    Ok(proof_to_hash_inner(&gamma))
}

/// `ECVRF_verify` — RFC 9381 §5.3.
/// Returns the VRF output (beta) on success, `Err(InvalidSignature)` on failure.
pub fn verify(pk: &VrfPublicKey, proof: &VrfProof, alpha: &[u8]) -> Result<VrfOutput, CryptoError> {
    let pi = &proof.0;

    // Decode Gamma.
    let gamma = CompressedEdwardsY(pi[..32].try_into().unwrap())
        .decompress()
        .ok_or(CryptoError::InvalidSignature)?;

    // Decode c (16 bytes, zero-padded to 32).
    let mut c_padded = [0u8; 32];
    c_padded[..C_LEN].copy_from_slice(&pi[32..48]);
    let c = Scalar::from_bytes_mod_order(c_padded);

    // Decode s (32 bytes canonical).
    let s_bytes: [u8; 32] = pi[48..80].try_into().unwrap();
    let s = Scalar::from_canonical_bytes(s_bytes)
        .into_option()
        .ok_or(CryptoError::InvalidSignature)?;

    // Decode public key.
    let y = CompressedEdwardsY(pk.0)
        .decompress()
        .ok_or(CryptoError::InvalidSignature)?;

    // Re-derive H.
    let h = hash_to_curve(pk, alpha);

    // Reconstruct U = s*B - c*Y and V = s*H - c*Gamma.
    let u = ED25519_BASEPOINT_POINT * s - y * c;
    let v = h * s - gamma * c;

    // Re-compute challenge.
    let c_prime = hash_points(&h, &gamma, &u, &v);

    if c != c_prime {
        return Err(CryptoError::InvalidSignature);
    }

    Ok(proof_to_hash_inner(&gamma))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vrf_prove_verify_roundtrip() {
        let sk = VrfSecretKey::generate();
        let pk = sk.public_key();
        let alpha = b"hello, VinX committee selection";

        let proof = sk.prove(alpha);
        let output = verify(&pk, &proof, alpha).expect("proof must verify");

        // Output must be non-zero.
        assert_ne!(output, [0u8; 64]);
    }

    #[test]
    fn test_vrf_output_is_deterministic() {
        let sk = VrfSecretKey::generate();
        let alpha = b"same input";

        let (p1, o1) = sk.evaluate(alpha);
        let (p2, o2) = sk.evaluate(alpha);

        // Proof and output are both deterministic for a fixed (sk, alpha).
        assert_eq!(p1.0, p2.0);
        assert_eq!(o1, o2);
    }

    #[test]
    fn test_vrf_wrong_pk_fails_verification() {
        let sk = VrfSecretKey::generate();
        let attacker_sk = VrfSecretKey::generate();
        let alpha = b"committee seed";

        let proof = sk.prove(alpha);
        let wrong_pk = attacker_sk.public_key();

        assert!(verify(&wrong_pk, &proof, alpha).is_err());
    }

    #[test]
    fn test_vrf_tampered_proof_fails_verification() {
        let sk = VrfSecretKey::generate();
        let pk = sk.public_key();
        let alpha = b"committee seed";

        let proof = sk.prove(alpha);
        let mut tampered = proof.0;
        tampered[40] ^= 0xFF; // Flip bits in the s component.
        let bad_proof = VrfProof(tampered);

        assert!(verify(&pk, &bad_proof, alpha).is_err());
    }

    #[test]
    fn test_vrf_different_alphas_give_different_outputs() {
        let sk = VrfSecretKey::generate();
        let pk = sk.public_key();

        let (_, o1) = sk.evaluate(b"alpha1");
        let (_, o2) = sk.evaluate(b"alpha2");

        assert_ne!(o1, o2);
        // Both proofs verify correctly.
        assert!(verify(&pk, &sk.prove(b"alpha1"), b"alpha1").is_ok());
        assert!(verify(&pk, &sk.prove(b"alpha2"), b"alpha2").is_ok());
    }
}
