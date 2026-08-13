/// BLS12-381 aggregate signatures for block co-signing (ADR 0046).
///
/// Scheme: BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_
///   - Public keys  : G1 compressed, 48 bytes
///   - Signatures   : G2 compressed, 96 bytes
///   - Aggregate sig: G2 compressed, 96 bytes (one for all N co-signers)
///
/// Validators register a BLS public key alongside their Ed25519 key at admission.
/// Block co-signatures use BLS only; account transactions keep Ed25519.
use blst::min_pk::{AggregatePublicKey, AggregateSignature, PublicKey, SecretKey, Signature};
use blst::BLST_ERROR;
use rand::RngCore;

/// Domain-separation tag for VinX block co-signatures (BLS Proof-of-Possession scheme).
pub const BLS_COSIG_DST: &[u8] = b"VINX_BLS_COSIG_V1";
/// Domain-separation tag used for the Proof-of-Possession registration signature.
pub const BLS_POP_DST: &[u8] = b"VINX_BLS_POP_V1";

/// A BLS12-381 secret key (32 bytes scalar).
pub struct BlsSecretKey(SecretKey);

impl Clone for BlsSecretKey {
    fn clone(&self) -> Self {
        let bytes = self.to_bytes();
        Self::from_bytes(&bytes).expect("cloning a valid key never fails")
    }
}

/// A BLS12-381 public key (G1 point, 48 bytes compressed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlsPubKey(pub [u8; 48]);

/// A BLS12-381 signature (G2 point, 96 bytes compressed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlsSignature(pub [u8; 96]);

#[derive(Debug, thiserror::Error)]
pub enum BlsError {
    #[error("BLS key deserialization failed")]
    InvalidKey,
    #[error("BLS signature verification failed")]
    InvalidSignature,
    #[error("BLS aggregate failed: no signatures provided")]
    EmptyAggregate,
    #[error("BLS library error: {0:?}")]
    Blst(BLST_ERROR),
}

impl BlsSecretKey {
    /// Generate a fresh random BLS secret key.
    pub fn generate() -> Self {
        let mut ikm = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut ikm);
        let sk = SecretKey::key_gen(&ikm, &[]).expect("key_gen with 32-byte IKM never fails");
        BlsSecretKey(sk)
    }

    /// Deserialize from 32 raw scalar bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<Self, BlsError> {
        SecretKey::from_bytes(bytes)
            .map(BlsSecretKey)
            .map_err(|_| BlsError::InvalidKey)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Derive the corresponding public key (G1 point).
    pub fn public_key(&self) -> BlsPubKey {
        let pk = self.0.sk_to_pk();
        BlsPubKey(pk.compress())
    }

    /// Sign `msg` with this key using `DST = BLS_COSIG_DST`.
    pub fn sign(&self, msg: &[u8]) -> BlsSignature {
        let sig = self.0.sign(msg, BLS_COSIG_DST, &[]);
        BlsSignature(sig.compress())
    }

    /// Produce a Proof-of-Possession signature over the public key bytes.
    pub fn proof_of_possession(&self) -> BlsSignature {
        let pk_bytes = self.public_key().0;
        let sig = self.0.sign(&pk_bytes, BLS_POP_DST, &[]);
        BlsSignature(sig.compress())
    }
}

impl BlsPubKey {
    pub fn from_bytes(bytes: &[u8; 48]) -> Result<Self, BlsError> {
        PublicKey::from_bytes(bytes)
            .map(|_| BlsPubKey(*bytes))
            .map_err(|_| BlsError::InvalidKey)
    }

    fn inner(&self) -> Result<PublicKey, BlsError> {
        PublicKey::from_bytes(&self.0).map_err(|_| BlsError::InvalidKey)
    }

    /// Verify a Proof-of-Possession signature (registered at validator admission).
    pub fn verify_pop(&self, pop: &BlsSignature) -> Result<(), BlsError> {
        let pk = self.inner()?;
        let sig = Signature::from_bytes(&pop.0).map_err(|_| BlsError::InvalidSignature)?;
        let err = sig.verify(true, &self.0, BLS_POP_DST, &[], &pk, true);
        if err == BLST_ERROR::BLST_SUCCESS {
            Ok(())
        } else {
            Err(BlsError::InvalidSignature)
        }
    }
}

/// Verify a single BLS co-signature over `msg`.
pub fn verify(pk: &BlsPubKey, sig: &BlsSignature, msg: &[u8]) -> Result<(), BlsError> {
    let pk_inner = pk.inner()?;
    let sig_inner = Signature::from_bytes(&sig.0).map_err(|_| BlsError::InvalidSignature)?;
    let err = sig_inner.verify(true, msg, BLS_COSIG_DST, &[], &pk_inner, true);
    if err == BLST_ERROR::BLST_SUCCESS {
        Ok(())
    } else {
        Err(BlsError::InvalidSignature)
    }
}

/// Aggregate N BLS signatures into a single 96-byte aggregate.
///
/// `sigs` must be non-empty. All signers must have used the same `msg` and
/// `BLS_COSIG_DST`. The caller is responsible for ordering / deduplication.
pub fn aggregate(sigs: &[BlsSignature]) -> Result<BlsSignature, BlsError> {
    if sigs.is_empty() {
        return Err(BlsError::EmptyAggregate);
    }
    let inner: Vec<Signature> = sigs
        .iter()
        .map(|s| Signature::from_bytes(&s.0).map_err(|_| BlsError::InvalidSignature))
        .collect::<Result<_, _>>()?;
    let refs: Vec<&Signature> = inner.iter().collect();
    let agg = AggregateSignature::aggregate(&refs, true).map_err(BlsError::Blst)?;
    Ok(BlsSignature(agg.to_signature().compress()))
}

/// Verify an aggregate signature against a set of public keys over `msg`.
///
/// `pks` must match the signers in the same order used during aggregation.
/// Uses a single multi-pairing check — O(1) pairings regardless of N.
pub fn verify_aggregate(
    pks: &[BlsPubKey],
    agg_sig: &BlsSignature,
    msg: &[u8],
) -> Result<(), BlsError> {
    if pks.is_empty() {
        return Err(BlsError::EmptyAggregate);
    }
    let pk_inners: Vec<PublicKey> = pks
        .iter()
        .map(|p| p.inner())
        .collect::<Result<_, _>>()?;
    let sig = Signature::from_bytes(&agg_sig.0).map_err(|_| BlsError::InvalidSignature)?;
    let pk_refs: Vec<&PublicKey> = pk_inners.iter().collect();
    let agg_pk = AggregatePublicKey::aggregate(&pk_refs, true).map_err(BlsError::Blst)?;
    let agg_pk_inner = agg_pk.to_public_key();
    let err = sig.verify(true, msg, BLS_COSIG_DST, &[], &agg_pk_inner, true);
    if err == BLST_ERROR::BLST_SUCCESS {
        Ok(())
    } else {
        Err(BlsError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (BlsSecretKey, BlsPubKey) {
        let sk = BlsSecretKey::generate();
        let pk = sk.public_key();
        (sk, pk)
    }

    #[test]
    fn test_sign_and_verify() {
        let (sk, pk) = keypair();
        let msg = b"VinX block #42";
        let sig = sk.sign(msg);
        assert!(verify(&pk, &sig, msg).is_ok());
    }

    #[test]
    fn test_wrong_message_rejected() {
        let (sk, pk) = keypair();
        let sig = sk.sign(b"correct msg");
        assert!(verify(&pk, &sig, b"wrong msg").is_err());
    }

    #[test]
    fn test_wrong_key_rejected() {
        let (sk, _) = keypair();
        let (_, wrong_pk) = keypair();
        let sig = sk.sign(b"msg");
        assert!(verify(&wrong_pk, &sig, b"msg").is_err());
    }

    #[test]
    fn test_aggregate_and_verify() {
        let msg = b"epoch 7 block hash";
        let validators: Vec<(BlsSecretKey, BlsPubKey)> = (0..5).map(|_| keypair()).collect();
        let sigs: Vec<BlsSignature> = validators.iter().map(|(sk, _)| sk.sign(msg)).collect();
        let pks: Vec<BlsPubKey> = validators.iter().map(|(_, pk)| pk.clone()).collect();
        let agg = aggregate(&sigs).unwrap();
        assert!(verify_aggregate(&pks, &agg, msg).is_ok());
    }

    #[test]
    fn test_aggregate_wrong_key_rejected() {
        let msg = b"test";
        let (sk1, pk1) = keypair();
        let (sk2, _) = keypair();
        let (_, wrong_pk) = keypair();
        let sigs = [sk1.sign(msg), sk2.sign(msg)];
        let agg = aggregate(&sigs).unwrap();
        // Replace pk2 with an unrelated key — verification must fail.
        assert!(verify_aggregate(&[pk1, wrong_pk], &agg, msg).is_err());
    }

    #[test]
    fn test_proof_of_possession_valid() {
        let sk = BlsSecretKey::generate();
        let pk = sk.public_key();
        let pop = sk.proof_of_possession();
        assert!(pk.verify_pop(&pop).is_ok());
    }

    #[test]
    fn test_proof_of_possession_wrong_key_rejected() {
        let sk = BlsSecretKey::generate();
        let pop = sk.proof_of_possession();
        let (_, other_pk) = keypair();
        assert!(other_pk.verify_pop(&pop).is_err());
    }

    #[test]
    fn test_aggregate_empty_fails() {
        assert!(aggregate(&[]).is_err());
    }

    #[test]
    fn test_roundtrip_secret_key() {
        let sk = BlsSecretKey::generate();
        let bytes = sk.to_bytes();
        let sk2 = BlsSecretKey::from_bytes(&bytes).unwrap();
        assert_eq!(sk.public_key(), sk2.public_key());
    }
}
