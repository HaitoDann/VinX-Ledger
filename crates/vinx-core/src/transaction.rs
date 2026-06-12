use crate::amount::Amount;
use crate::protocol::ProtocolVersion;
use serde::{Deserialize, Serialize};
use vinx_crypto::{sha256, Address, Hash32, KeyPair, PublicKey, VinxSignature};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionType {
    Transfer,
    Stake,
    Unstake,
    FreezeAccount,
    UnfreezeAccount,
    /// Admin-only: schedule a protocol upgrade at a future block height.
    AnnounceUpgrade,
    /// Protocol-internal: moves tokens from reserve to the public sale pool each block.
    Emission,
    /// Admin-only: add a new address to the PoA validator set. `to` = new validator.
    AddValidator,
    /// Admin-only: remove an address from the PoA validator set. `to` = validator to remove.
    RemoveValidator,
    /// Slash a validator who double-signed. `to` = validator, `payload` = bincode(SlashEvidence).
    SlashValidator,
    /// Submit a governance proposal. `payload` = bincode(SubmitProposalPayload).
    SubmitProposal,
    /// Cast a vote on a proposal. `payload` = bincode(VotePayload).
    VoteProposal,
}

impl TransactionType {
    fn discriminant(&self) -> u8 {
        match self {
            TransactionType::Transfer => 0x01,
            TransactionType::Stake => 0x02,
            TransactionType::Unstake => 0x03,
            TransactionType::FreezeAccount => 0x04,
            TransactionType::UnfreezeAccount => 0x05,
            TransactionType::AnnounceUpgrade => 0x06,
            TransactionType::Emission => 0x07,
            TransactionType::AddValidator => 0x08,
            TransactionType::RemoveValidator => 0x09,
            TransactionType::SlashValidator => 0x0A,
            TransactionType::SubmitProposal => 0x0B,
            TransactionType::VoteProposal => 0x0C,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    pub tx_type: TransactionType,
    pub from: Address,
    pub to: Address,
    pub amount: Amount,
    /// Fee the sender explicitly agrees to pay (validated against protocol minimum).
    pub fee: Amount,
    pub nonce: u64,
    /// Extra typed data for specialized transactions (empty for standard ops).
    /// AnnounceUpgrade: 14 bytes = major(2) || minor(2) || patch(2) || activation_height(8)
    pub payload: Vec<u8>,
    /// Sender's public key — used to verify `from` ownership.
    pub pub_key: Option<PublicKey>,
    pub signature: Option<VinxSignature>,
}

impl Transaction {
    /// Canonical byte representation for signing. Does NOT include pub_key or signature.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(256);
        bytes.push(self.tx_type.discriminant());
        let from = self.from.as_str().as_bytes();
        bytes.push(from.len() as u8);
        bytes.extend_from_slice(from);
        let to = self.to.as_str().as_bytes();
        bytes.push(to.len() as u8);
        bytes.extend_from_slice(to);
        bytes.extend_from_slice(&self.amount.atoms().to_be_bytes());
        bytes.extend_from_slice(&self.fee.atoms().to_be_bytes());
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        if !self.payload.is_empty() {
            bytes.extend_from_slice(&self.payload);
        }
        bytes
    }

    pub fn hash(&self) -> Hash32 {
        sha256(&self.signing_bytes())
    }

    /// Signs the transaction in place using the provided keypair.
    pub fn sign(&mut self, keypair: &KeyPair) {
        let pk = keypair.public_key();
        let sig = keypair.sign(&self.signing_bytes());
        self.pub_key = Some(pk);
        self.signature = Some(sig);
    }

    /// Constructs and signs a Transfer transaction.
    pub fn new_transfer(
        keypair: &KeyPair,
        to: Address,
        amount: Amount,
        fee: Amount,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::Transfer,
            from,
            to,
            amount,
            fee,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs a Stake transaction.
    pub fn new_stake(keypair: &KeyPair, amount: Amount, fee: Amount, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let to = from.clone();
        let mut tx = Self {
            tx_type: TransactionType::Stake,
            from,
            to,
            amount,
            fee,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an Unstake transaction.
    pub fn new_unstake(keypair: &KeyPair, amount: Amount, fee: Amount, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let to = from.clone();
        let mut tx = Self {
            tx_type: TransactionType::Unstake,
            from,
            to,
            amount,
            fee,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs a FreezeAccount transaction (admin only).
    pub fn new_freeze(keypair: &KeyPair, target: Address, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::FreezeAccount,
            from,
            to: target,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an UnfreezeAccount transaction (admin only).
    pub fn new_unfreeze(keypair: &KeyPair, target: Address, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::UnfreezeAccount,
            from,
            to: target,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an AnnounceUpgrade transaction (admin only).
    ///
    /// The `activation_height` must be far enough in the future based on the upgrade type
    /// (patch ≥ 7 days, minor ≥ 30 days, major ≥ 90 days). Validation is enforced by WorldState.
    pub fn new_announce_upgrade(
        keypair: &KeyPair,
        version: ProtocolVersion,
        activation_height: u64,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        // Payload: major(2) || minor(2) || patch(2) || activation_height(8) = 14 bytes
        let mut payload = Vec::with_capacity(14);
        payload.extend_from_slice(&version.major.to_be_bytes());
        payload.extend_from_slice(&version.minor.to_be_bytes());
        payload.extend_from_slice(&version.patch.to_be_bytes());
        payload.extend_from_slice(&activation_height.to_be_bytes());
        let mut tx = Self {
            tx_type: TransactionType::AnnounceUpgrade,
            from: from.clone(),
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload,
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Decodes version + activation_height from the payload of an AnnounceUpgrade transaction.
    /// Returns None if the payload is malformed.
    pub fn decode_upgrade_payload(&self) -> Option<(ProtocolVersion, u64)> {
        if self.payload.len() != 14 {
            return None;
        }
        let major = u16::from_be_bytes([self.payload[0], self.payload[1]]);
        let minor = u16::from_be_bytes([self.payload[2], self.payload[3]]);
        let patch = u16::from_be_bytes([self.payload[4], self.payload[5]]);
        let activation_height = u64::from_be_bytes(
            self.payload[6..14].try_into().ok()?
        );
        Some((ProtocolVersion::new(major, minor, patch), activation_height))
    }

    /// Constructs and signs an AddValidator transaction (admin only).
    pub fn new_add_validator(keypair: &KeyPair, validator: Address, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::AddValidator,
            from,
            to: validator,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs a RemoveValidator transaction (admin only).
    pub fn new_remove_validator(keypair: &KeyPair, validator: Address, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::RemoveValidator,
            from,
            to: validator,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs a SlashValidator tx with equivocation evidence.
    pub fn new_slash_validator(keypair: &KeyPair, validator: Address, evidence: &crate::block::SlashEvidence, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let payload = bincode::serialize(evidence).expect("slash evidence serializable");
        let mut tx = Self {
            tx_type: TransactionType::SlashValidator,
            from,
            to: validator,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload,
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs a SubmitProposal tx.
    pub fn new_submit_proposal(
        keypair: &KeyPair,
        description: String,
        action: crate::governance::GovernanceAction,
        voting_period_blocks: u64,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let payload_data = crate::governance::SubmitProposalPayload { description, action, voting_period_blocks };
        let payload = bincode::serialize(&payload_data).expect("proposal serializable");
        let mut tx = Self {
            tx_type: TransactionType::SubmitProposal,
            from: from.clone(),
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload,
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs a VoteProposal tx.
    pub fn new_vote_proposal(keypair: &KeyPair, proposal_id: u64, approve: bool, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let payload = bincode::serialize(&crate::governance::VotePayload { proposal_id, approve }).expect("vote serializable");
        let mut tx = Self {
            tx_type: TransactionType::VoteProposal,
            from: from.clone(),
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            payload,
            pub_key: Some(pk),
            signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs an Emission transaction (no signature — protocol-only).
    pub fn new_emission(to: Address, amount: Amount, from_reserve: Address) -> Self {
        Self {
            tx_type: TransactionType::Emission,
            from: from_reserve,
            to,
            amount,
            fee: Amount::ZERO,
            nonce: 0,
            payload: vec![],
            pub_key: None,
            signature: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::amount::{Amount, DEFAULT_FEE_FLOOR_ATOMS};

    fn make_transfer() -> (KeyPair, Transaction) {
        let sender = KeyPair::generate();
        let receiver = KeyPair::generate();
        let to = Address::from_public_key(&receiver.public_key());
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let tx = Transaction::new_transfer(&sender, to, Amount::from_vinx(100), fee, 0);
        (sender, tx)
    }

    #[test]
    fn test_transfer_is_signed() {
        let (_, tx) = make_transfer();
        assert!(tx.signature.is_some());
        assert!(tx.pub_key.is_some());
    }

    #[test]
    fn test_signing_bytes_include_all_fields() {
        let (_, tx) = make_transfer();
        let bytes = tx.signing_bytes();
        assert!(!bytes.is_empty());
        assert_eq!(bytes[0], 0x01); // Transfer discriminant
    }

    #[test]
    fn test_hash_is_deterministic() {
        let (_, tx) = make_transfer();
        assert_eq!(tx.hash(), tx.hash());
    }

    #[test]
    fn test_different_txs_different_hashes() {
        let (_, tx1) = make_transfer();
        let (_, tx2) = make_transfer();
        assert_ne!(tx1.hash(), tx2.hash());
    }

    #[test]
    fn test_signature_verifies_against_pub_key() {
        let (_, tx) = make_transfer();
        let pk = tx.pub_key.as_ref().unwrap();
        let sig = tx.signature.as_ref().unwrap();
        assert!(pk.verify(&tx.signing_bytes(), sig).is_ok());
    }

    #[test]
    fn test_emission_has_no_signature() {
        let pool = Address::from_public_key(&KeyPair::generate().public_key());
        let reserve = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = Transaction::new_emission(pool, Amount::from_vinx(3_155), reserve);
        assert!(tx.signature.is_none());
        assert!(tx.pub_key.is_none());
        assert_eq!(tx.tx_type, TransactionType::Emission);
    }

    #[test]
    fn test_nonce_is_in_signing_bytes() {
        let sender = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let tx0 = Transaction::new_transfer(&sender, to.clone(), Amount::from_vinx(10), fee, 0);
        let tx1 = Transaction::new_transfer(&sender, to, Amount::from_vinx(10), fee, 1);
        assert_ne!(tx0.signing_bytes(), tx1.signing_bytes());
        assert_ne!(tx0.hash(), tx1.hash());
    }

    #[test]
    fn test_announce_upgrade_payload_roundtrip() {
        let kp = KeyPair::generate();
        let version = ProtocolVersion::new(1, 1, 0);
        let activation = 500_000u64;
        let tx = Transaction::new_announce_upgrade(&kp, version.clone(), activation, 0);
        let (decoded_ver, decoded_height) = tx.decode_upgrade_payload().unwrap();
        assert_eq!(decoded_ver, version);
        assert_eq!(decoded_height, activation);
    }

    #[test]
    fn test_freeze_tx_has_correct_type() {
        let kp = KeyPair::generate();
        let target = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = Transaction::new_freeze(&kp, target, 0);
        assert_eq!(tx.tx_type, TransactionType::FreezeAccount);
        assert_eq!(tx.amount, Amount::ZERO);
        assert_eq!(tx.fee, Amount::ZERO);
    }

    #[test]
    fn test_unfreeze_tx_has_correct_type() {
        let kp = KeyPair::generate();
        let target = Address::from_public_key(&KeyPair::generate().public_key());
        let tx = Transaction::new_unfreeze(&kp, target, 1);
        assert_eq!(tx.tx_type, TransactionType::UnfreezeAccount);
    }
}
