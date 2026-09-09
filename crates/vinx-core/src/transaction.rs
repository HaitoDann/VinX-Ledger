use crate::amount::Amount;
use crate::chain_id::CHAIN_ID_DEVNET;
use crate::protocol::ProtocolVersion;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_crypto::{sha256, Address, Hash32, KeyPair, PublicKey, VinxSignature};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub enum TransactionType {
    Transfer,
    Stake,
    Unstake,
    /// Admin-only: schedule a protocol upgrade at a future block height.
    AnnounceUpgrade,
    /// Slash a validator who double-signed. `to` = validator, `payload` = bincode(SlashEvidence).
    SlashValidator,
    /// Admin-only governance action executed immediately. `payload` = bincode(GovernanceAction).
    AdminAction,
    /// Module-registry operation (ADR 0010). `payload` = bincode(ModuleOp). Appended last so
    /// existing bincode/borsh variant indices are unchanged.
    AnchorState,
    /// Validator self-registers their BLS12-381 public key + Proof-of-Possession (ADR 0046).
    /// `payload` = bincode(RegisterBlsKeyPayload). Any bonded validator may call this for
    /// themselves; no admin authorization required. Appended last to preserve bincode/borsh
    /// discriminants of all prior variants.
    RegisterBlsKey,
    /// Jailed validator requests to re-enter the active set (ADR 0027).
    /// `from` = the validator address; no payload. Requires `UNJAIL_COOLDOWN_HEIGHTS` to
    /// have elapsed since the jail sentence. Appended last to preserve discriminants.
    Unjail,
    /// Validator self-registers their ECVRF public key (ADR 0029 Phase 2b).
    /// `payload` = 32-byte compressed Edwards25519 VRF public key.
    /// Any bonded validator may call this for themselves; no admin authorization required.
    /// Appended last to preserve discriminants of all prior variants.
    RegisterVrfKey,
}

impl TransactionType {
    fn discriminant(&self) -> u8 {
        match self {
            TransactionType::Transfer => 0x01,
            TransactionType::Stake => 0x02,
            TransactionType::Unstake => 0x03,
            TransactionType::AnnounceUpgrade => 0x04,
            // 0x05 / 0x06 (dedicated AddValidator / RemoveValidator) were retired in
            // ADR 0007 — validator-set changes now go through AdminAction (0x08) only.
            // The discriminants of the surviving types are kept stable.
            TransactionType::SlashValidator => 0x07,
            TransactionType::AdminAction => 0x08,
            TransactionType::AnchorState => 0x09,
            TransactionType::RegisterBlsKey => 0x0A,
            TransactionType::Unjail => 0x0B,
            TransactionType::RegisterVrfKey => 0x0C,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Transaction {
    pub tx_type: TransactionType,
    pub from: Address,
    pub to: Address,
    pub amount: Amount,
    /// Fee the sender explicitly agrees to pay (validated against protocol minimum).
    pub fee: Amount,
    pub nonce: u64,
    /// Chain this transaction is valid on — prevents cross-network replay attacks.
    /// Must match the node's configured chain ID (CHAIN_ID_MAINNET / TESTNET / DEVNET).
    #[serde(default = "default_chain_id")]
    pub chain_id: u32,
    /// Block height after which this transaction is invalid.
    /// `None` = no expiry (valid until included or evicted from mempool).
    #[serde(default)]
    pub expires_at_height: Option<u64>,
    /// Extra typed data for specialized transactions (empty for standard ops).
    /// AnnounceUpgrade: 14 bytes = major(2) || minor(2) || patch(2) || activation_ts(8)
    /// where activation_ts is a Unix timestamp in seconds (ADR 0006).
    pub payload: Vec<u8>,
    /// Sender's public key — used to verify `from` ownership.
    pub pub_key: Option<PublicKey>,
    pub signature: Option<VinxSignature>,
    /// Optional sponsor: third party who pays the fee (gasless UX).
    /// When set, the fee is debited from `sponsor` instead of `from`.
    #[serde(default)]
    pub sponsor: Option<Address>,
    /// Sponsor's public key — proves sponsor identity.
    #[serde(default)]
    pub sponsor_pub_key: Option<PublicKey>,
    /// Sponsor's signature over the transaction's signing bytes.
    /// Proves the sponsor consented to pay the fee for this exact transaction.
    #[serde(default)]
    pub sponsor_signature: Option<VinxSignature>,
}

/// Payload for a `RegisterBlsKey` transaction (ADR 0046).
/// Both fields are serialized as length-prefixed byte vectors (bincode default).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterBlsKeyPayload {
    /// BLS12-381 G1 compressed public key, 48 bytes.
    pub bls_pub_key: Vec<u8>,
    /// Proof-of-Possession: BLS G2 signature over `bls_pub_key` using `BLS_POP_DST`, 96 bytes.
    pub bls_pop: Vec<u8>,
}

/// serde default for a missing `chain_id` (ADR 0008): the invalid sentinel, not devnet.
/// A transaction that omits its chain ID is thus rejected at validation instead of being
/// silently bound to devnet.
fn default_chain_id() -> u32 {
    crate::chain_id::CHAIN_ID_INVALID
}

impl Transaction {
    /// Worst-case balance debit this transaction imposes on its **sender** when
    /// applied, in atoms. Used by stateful mempool admission (anti-spam): the
    /// sender must actually hold what the transaction claims to spend, otherwise
    /// a zero-balance account could poison the fee-priority queue with unpayable
    /// high-fee transactions for free.
    ///
    /// Sponsored transfers exclude the fee (the sponsor pays it — checked
    /// separately). Unstake moves staked funds back to the balance, so the
    /// sender's balance is only ever debited the fee (zero under the ADR 0009
    /// exemption); same for the remaining administrative types.
    pub fn admission_cost_atoms(&self) -> u128 {
        match self.tx_type {
            TransactionType::Transfer => {
                let amount = self.amount.atoms();
                if self.sponsor.is_some() {
                    amount
                } else {
                    amount.saturating_add(self.fee.atoms())
                }
            }
            TransactionType::Stake => self.amount.atoms().saturating_add(self.fee.atoms()),
            _ => self.fee.atoms(),
        }
    }

    /// Canonical byte layout signed by the sender (and sponsor). Does NOT include
    /// `pub_key` or `signature`.
    ///
    /// Addresses are the raw 20-byte payload (fixed length, so no length prefix).
    /// Layout: discriminant(1) ‖ from(20) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖
    /// nonce(8 BE) ‖ chain_id(4 BE) ‖ expiry(0 | 1‖8 BE) ‖ payload_len(4 BE) ‖ payload ‖
    /// sponsor(0 | 1‖20). Any client (web UI, SDK) must reproduce this exactly.
    ///
    /// Every variable-length field is length-prefixed or flag-delimited, so the encoding
    /// is injective: two different transactions can never share signing bytes (VINX-12).
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(128);
        bytes.push(self.tx_type.discriminant());
        bytes.extend_from_slice(self.from.as_bytes());
        bytes.extend_from_slice(self.to.as_bytes());
        bytes.extend_from_slice(&self.amount.atoms().to_be_bytes());
        bytes.extend_from_slice(&self.fee.atoms().to_be_bytes());
        bytes.extend_from_slice(&self.nonce.to_be_bytes());
        bytes.extend_from_slice(&self.chain_id.to_be_bytes());
        match self.expires_at_height {
            Some(h) => {
                bytes.push(1u8);
                bytes.extend_from_slice(&h.to_be_bytes());
            }
            None => bytes.push(0u8),
        }
        // VINX-12: length-prefixed, always — including when empty.
        //
        // `payload` used to be appended raw, immediately followed by the sponsor marker
        // (`0x00`, or `0x01 ‖ sponsor[20]`). That encoding is not injective: for any
        // sponsor address whose last byte is 0x00 (1 in 256), a sponsored transaction and
        // an unsponsored one with a re-cut payload produce identical signing bytes — and
        // therefore an identical txid, since `hash()` is `sha256(signing_bytes())`. One
        // signature was valid for two economically different transactions, and the fee
        // payer differed between them (sponsor vs sender).
        //
        // A `u32` big-endian length makes every field self-delimiting, so no re-cut of the
        // byte string can be reinterpreted as different field boundaries. The prefix is
        // written unconditionally: emitting it only for a non-empty payload would leave the
        // empty case ambiguous again.
        bytes.extend_from_slice(&(self.payload.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        // Include sponsor address so the sponsor signature commits to it
        if let Some(ref sponsor) = self.sponsor {
            bytes.push(1u8);
            bytes.extend_from_slice(sponsor.as_bytes());
        } else {
            bytes.push(0u8);
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
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs a Stake transaction.
    pub fn new_stake(keypair: &KeyPair, amount: Amount, fee: Amount, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let to = from;
        let mut tx = Self {
            tx_type: TransactionType::Stake,
            from,
            to,
            amount,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an Unstake transaction.
    pub fn new_unstake(keypair: &KeyPair, amount: Amount, fee: Amount, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let to = from;
        let mut tx = Self {
            tx_type: TransactionType::Unstake,
            from,
            to,
            amount,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an AnnounceUpgrade transaction (admin only).
    ///
    /// `activation_ts` is a Unix timestamp (seconds) that must be far enough in the future
    /// based on the upgrade type (patch ≥ 7 days, minor ≥ 30 days, major ≥ 90 days), measured
    /// against block timestamps (ADR 0006). Validation is enforced by WorldState.
    pub fn new_announce_upgrade(
        keypair: &KeyPair,
        version: ProtocolVersion,
        activation_ts: u64,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        // Payload: major(2) || minor(2) || patch(2) || activation_ts(8) = 14 bytes
        let mut payload = Vec::with_capacity(14);
        payload.extend_from_slice(&version.major.to_be_bytes());
        payload.extend_from_slice(&version.minor.to_be_bytes());
        payload.extend_from_slice(&version.patch.to_be_bytes());
        payload.extend_from_slice(&activation_ts.to_be_bytes());
        let mut tx = Self {
            tx_type: TransactionType::AnnounceUpgrade,
            from,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Decodes version + activation timestamp (Unix seconds) from the payload of an
    /// AnnounceUpgrade transaction. Returns None if the payload is malformed.
    pub fn decode_upgrade_payload(&self) -> Option<(ProtocolVersion, u64)> {
        if self.payload.len() != 14 {
            return None;
        }
        let major = u16::from_be_bytes([self.payload[0], self.payload[1]]);
        let minor = u16::from_be_bytes([self.payload[2], self.payload[3]]);
        let patch = u16::from_be_bytes([self.payload[4], self.payload[5]]);
        let activation_ts = u64::from_be_bytes(self.payload[6..14].try_into().ok()?);
        Some((ProtocolVersion::new(major, minor, patch), activation_ts))
    }

    /// Constructs a SlashValidator tx with equivocation evidence.
    pub fn new_slash_validator(
        keypair: &KeyPair,
        validator: Address,
        evidence: &crate::block::SlashEvidence,
        nonce: u64,
    ) -> Self {
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
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an AdminAction transaction (admin only).
    /// The `action` is a GovernanceAction that will be executed immediately on-chain.
    pub fn new_admin_action(
        keypair: &KeyPair,
        action: &crate::governance::GovernanceAction,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let payload =
            bincode::serialize(action).expect("GovernanceAction serialization is infallible");
        let mut tx = Self {
            tx_type: TransactionType::AdminAction,
            from,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an AnchorState transaction carrying a module-registry
    /// operation (ADR 0010). `fee` must cover the protocol minimum. The op's target module
    /// is encoded in the payload, so `to` is set to the sender by convention.
    pub fn new_anchor_state(
        keypair: &KeyPair,
        op: &crate::module::ModuleOp,
        fee: Amount,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let payload = bincode::serialize(op).expect("ModuleOp serialization is infallible");
        let mut tx = Self {
            tx_type: TransactionType::AnchorState,
            from,
            to: from,
            amount: Amount::ZERO,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs a RegisterBlsKey transaction (ADR 0046).
    /// Any bonded validator calls this to register their BLS12-381 key + Proof-of-Possession.
    pub fn new_register_bls_key(
        keypair: &KeyPair,
        payload: &RegisterBlsKeyPayload,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let raw =
            bincode::serialize(payload).expect("RegisterBlsKeyPayload serialization is infallible");
        let mut tx = Self {
            tx_type: TransactionType::RegisterBlsKey,
            from,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: raw,
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an Unjail transaction (ADR 0027).
    /// The jailed validator calls this for themselves after UNJAIL_COOLDOWN_HEIGHTS have elapsed.
    pub fn new_unjail(keypair: &KeyPair, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::Unjail,
            from,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs a RegisterVrfKey transaction (ADR 0029 Phase 2b).
    /// Any bonded validator calls this to register their ECVRF public key for committee selection.
    pub fn new_register_vrf_key(keypair: &KeyPair, vrf_pub_key: &[u8; 32], nonce: u64) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::RegisterVrfKey,
            from,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vrf_pub_key.to_vec(),
            pub_key: Some(pk),
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Builder: override the chain ID (use CHAIN_ID_MAINNET / TESTNET / DEVNET).
    pub fn with_chain_id(mut self, chain_id: u32) -> Self {
        self.chain_id = chain_id;
        self
    }

    /// Builder: set block-height expiry for this transaction.
    pub fn with_expiry(mut self, height: u64) -> Self {
        self.expires_at_height = Some(height);
        self
    }

    /// Builder: attach a sponsor who will pay the fee instead of the sender.
    /// `keypair` is the sponsor's keypair. Call after all other fields are set,
    /// since the sponsor signs the transaction's current signing bytes.
    ///
    /// # Ordering: the sender must sign *after* this call
    ///
    /// Attaching a sponsor appends the sponsor marker to [`Transaction::signing_bytes`],
    /// so any sender signature made earlier (e.g. by `new_transfer`) no longer verifies.
    /// Call [`Transaction::sign`] with the sender's keypair afterwards:
    ///
    /// ```ignore
    /// let mut tx = Transaction::new_transfer(&sender, to, amount, fee, nonce)
    ///     .with_sponsor(&sponsor);
    /// tx.sign(&sender); // re-sign: signing bytes changed
    /// ```
    ///
    /// Signature fields are themselves excluded from the signing bytes, so re-signing the
    /// sender does not invalidate the sponsor's signature.
    pub fn with_sponsor(mut self, keypair: &KeyPair) -> Self {
        let pk = keypair.public_key();
        let addr = Address::from_public_key(&pk);
        self.sponsor = Some(addr);
        self.sponsor_pub_key = Some(keypair.public_key());
        self.sponsor_signature = Some(keypair.sign(&self.signing_bytes()));
        self
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

    /// Golden vector locking the canonical `signing_bytes` layout. Any external
    /// signer (the web UI in rpc/ui.rs, third-party wallets) must reproduce this
    /// exact preimage; changing it is a consensus-breaking protocol change.
    #[test]
    fn test_signing_bytes_golden_vector() {
        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            from: Address::from_bytes([0x11; 20]),
            to: Address::from_bytes([0x22; 20]),
            amount: Amount::from_atoms(1_000_000),
            fee: Amount::from_atoms(500),
            nonce: 7,
            chain_id: 42,
            expires_at_height: None,
            payload: vec![],
            pub_key: None,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        // disc(01) ‖ from(0x11×20) ‖ to(0x22×20) ‖ amount(16 BE) ‖ fee(16 BE)
        //   ‖ nonce(8 BE) ‖ chain_id(4 BE) ‖ expiry(00) ‖ payload_len(4 BE) ‖ sponsor(00)
        let expected = concat!(
            "01",
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
            "000000000000000000000000000f4240",
            "000000000000000000000000000001f4",
            "0000000000000007",
            "0000002a",
            "00",
            "00000000",
            "00",
        );
        assert_eq!(hex::encode(tx.signing_bytes()), expected);
    }

    #[test]
    fn test_governance_signing_bytes_golden_vector() {
        // Cross-check for the admin console: the in-browser signing of governance
        // transactions must reproduce signing_bytes() exactly. These golden hexes
        // were generated by an independent JS implementation (scratchpad/gov_sign.js)
        // replicating the admin page's byte layout — matching them proves the
        // JS ↔ Rust equivalence for zero amount/fee and payload placement.
        let from = Address::from_bytes(std::array::from_fn(|i| (i + 1) as u8)); // 01..14
        let to = Address::from_bytes(std::array::from_fn(|i| (i + 21) as u8)); // 15..28
        let mk = |tx_type: TransactionType, to: Address, payload: Vec<u8>| Transaction {
            tx_type,
            from,
            to,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce: 7,
            chain_id: 42,
            expires_at_height: None,
            payload,
            pub_key: None,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };

        // ADR 0007: validator-set changes go through AdminAction (0x08) carrying a
        // bincode(GovernanceAction). The admin console (rpc/ui.rs) reproduces the same
        // bincode: enum tag u32 LE (AddValidator = 0) ‖ address(20 raw bytes).
        // Here `to = from` (self), amount/fee = 0.
        let mut add_payload = Vec::with_capacity(24);
        add_payload.extend_from_slice(&0u32.to_le_bytes()); // GovernanceAction::AddValidator
        add_payload.extend_from_slice(to.as_bytes());
        let add = mk(TransactionType::AdminAction, from, add_payload);
        let expected_add = concat!(
            "08",                                       // AdminAction discriminant
            "0102030405060708090a0b0c0d0e0f1011121314", // from  = 01..14
            "0102030405060708090a0b0c0d0e0f1011121314", // to    = self (from)
            "00000000000000000000000000000000",         // amount 0
            "00000000000000000000000000000000",         // fee 0
            "0000000000000007",                         // nonce 7
            "0000002a",                                 // chain_id 42
            "00",                                       // expiry None
            "00000018",                                 // payload_len = 24 (VINX-12)
            "00000000", // GovernanceAction::AddValidator (u32 LE = 0)
            "15161718191a1b1c1d1e1f202122232425262728", // validator address = 15..28
            "00",       // sponsor None
        );
        assert_eq!(hex::encode(add.signing_bytes()), expected_add);

        // Cross-check that the hand-built payload equals bincode(GovernanceAction) —
        // this is exactly what the in-browser admin console must reproduce.
        let gov = crate::governance::GovernanceAction::AddValidator(to);
        assert_eq!(add.payload, bincode::serialize(&gov).unwrap());

        // AnnounceUpgrade: to = self, payload = major(1) minor(2) patch(3) activation_ts(1000).
        let mut pl = Vec::with_capacity(14);
        pl.extend_from_slice(&1u16.to_be_bytes());
        pl.extend_from_slice(&2u16.to_be_bytes());
        pl.extend_from_slice(&3u16.to_be_bytes());
        pl.extend_from_slice(&1000u64.to_be_bytes());
        let upgrade = mk(TransactionType::AnnounceUpgrade, from, pl);
        assert_eq!(
            hex::encode(upgrade.signing_bytes()),
            "040102030405060708090a0b0c0d0e0f10111213140102030405060708090a0b0c0d0e0f1011121314\
             000000000000000000000000000000000000000000000000000000000000000000000000000000070000002a00\
             0000000e\
             00010002000300000000000003e800"
        );
    }

    /// VINX-12 regression — the payload/sponsor boundary must be unambiguous.
    ///
    /// Before the length prefix, for any sponsor address ending in `0x00` these two
    /// transactions produced identical signing bytes and an identical txid, so one
    /// signature was valid for both — and they disagree on who pays the fee.
    #[test]
    fn test_signing_bytes_payload_sponsor_boundary_is_unambiguous() {
        let from = Address::from_bytes([0x11; 20]);
        let to = Address::from_bytes([0x22; 20]);
        // Worst case: a sponsor address whose last byte is 0x00.
        let mut sponsor_raw = [0x33u8; 20];
        sponsor_raw[19] = 0x00;
        let sponsor = Address::from_bytes(sponsor_raw);

        let mk = |payload: Vec<u8>, sponsor: Option<Address>| Transaction {
            tx_type: TransactionType::Transfer,
            from,
            to,
            amount: Amount::from_atoms(1),
            fee: Amount::from_atoms(1),
            nonce: 0,
            chain_id: 42,
            expires_at_height: None,
            payload,
            pub_key: None,
            signature: None,
            sponsor,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };

        let payload = vec![0xAAu8, 0xBB];
        // A: sponsored, payload = P.
        let a = mk(payload.clone(), Some(sponsor));
        // B: unsponsored, payload = P ‖ 0x01 ‖ sponsor[0..19] — the old re-cut collision.
        let mut recut = payload.clone();
        recut.push(0x01);
        recut.extend_from_slice(&sponsor_raw[..19]);
        let b = mk(recut, None);

        assert_ne!(
            a.signing_bytes(),
            b.signing_bytes(),
            "payload/sponsor boundary must not be re-cuttable"
        );
        assert_ne!(a.hash(), b.hash(), "and the txids must differ");
    }

    /// The length prefix must also separate an empty payload from a short one, and keep
    /// payloads of different lengths distinguishable at the boundary.
    #[test]
    fn test_signing_bytes_payload_length_is_committed() {
        let mk = |payload: Vec<u8>| Transaction {
            tx_type: TransactionType::Transfer,
            from: Address::from_bytes([0x11; 20]),
            to: Address::from_bytes([0x22; 20]),
            amount: Amount::from_atoms(1),
            fee: Amount::from_atoms(1),
            nonce: 0,
            chain_id: 42,
            expires_at_height: None,
            payload,
            pub_key: None,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        let empty = mk(vec![]);
        let one_zero = mk(vec![0x00]);
        assert_ne!(empty.signing_bytes(), one_zero.signing_bytes());
        assert_ne!(empty.hash(), one_zero.hash());
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
    fn test_missing_chain_id_deserializes_to_invalid() {
        // ADR 0008: a transaction JSON that omits chain_id must deserialize to the
        // invalid sentinel (rejected downstream), never silently to devnet.
        // (Small atom amounts so the JSON round-trip via serde_json::Value — capped at
        // u64 — doesn't overflow; unrelated to the chain_id default under test.)
        use crate::chain_id::{CHAIN_ID_DEVNET, CHAIN_ID_INVALID};
        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            from: Address::from_bytes([0x11; 20]),
            to: Address::from_bytes([0x22; 20]),
            amount: Amount::from_atoms(5),
            fee: Amount::from_atoms(1),
            nonce: 0,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: None,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        let mut v = serde_json::to_value(&tx).unwrap();
        v.as_object_mut().unwrap().remove("chain_id");
        let tx2: Transaction = serde_json::from_value(v).unwrap();
        assert_eq!(tx2.chain_id, CHAIN_ID_INVALID);
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
}
