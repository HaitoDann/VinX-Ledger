use crate::amount::Amount;
use crate::chain_id::CHAIN_ID_DEVNET;
use crate::protocol::ProtocolVersion;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_crypto::{hash256, Address, Hash32, KeyPair, PublicKey, VinxSignature};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub enum TransactionType {
    Transfer,
    Stake,
    Unstake,
    /// Admin-only: schedule a protocol upgrade at a future block height.
    AnnounceUpgrade,
    /// Slash a validator who double-signed. `to` = validator, `payload` = borsh(VoteEquivocation).
    SlashValidator,
    /// Admin-only governance action executed immediately. `payload` = borsh(GovernanceAction).
    AdminAction,
    /// Validator self-registers their BLS12-381 public key + Proof-of-Possession (ADR 0046).
    /// `payload` = borsh(RegisterBlsKeyPayload). Any bonded validator may call this for
    /// themselves; no admin authorization required. Appended last to preserve borsh
    /// discriminants of all prior variants.
    RegisterBlsKey,
    /// Jailed validator requests to re-enter the active set (ADR 0027).
    /// `from` = the validator address; no payload. Requires `UNJAIL_COOLDOWN_HEIGHTS` to
    /// have elapsed since the jail sentence. Appended last to preserve discriminants.
    Unjail,
    /// Account option (ADR 0085): `payload = [1]` makes the sender's account refuse
    /// transfers without a memo (exchange deposit addresses), `[0]` lifts it. Pays the
    /// base fee.
    SetMemoRequired,
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
            // 0x09 (AnchorState, module registry) was retired in ADR 0081 with the modules
            // themselves (ADR 0064) — never reuse it.
            TransactionType::RegisterBlsKey => 0x0A,
            TransactionType::Unjail => 0x0B,
            TransactionType::SetMemoRequired => 0x0D,
            // 0x0C (RegisterVrfKey) retired with the VRF leader selection (ADR 0081 C2).
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Transaction {
    pub tx_type: TransactionType,
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
    /// Sender's public key, tagged with its key type (ADR 0081 D6). Mandatory: the
    /// sender address is *derived* from it ([`Transaction::sender`]), never sent
    /// separately — so the two can never disagree.
    pub pub_key: PublicKey,
    pub signature: Option<VinxSignature>,
    /// Optional sponsor: third party who pays the fee (gasless UX).
    /// When set, the fee is debited from `sponsor` instead of the sender.
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
/// Both fields are serialized as length-prefixed byte vectors (borsh: u32 length prefix).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct RegisterBlsKeyPayload {
    /// BLS12-381 G1 compressed public key, 48 bytes.
    pub bls_pub_key: Vec<u8>,
    /// Proof-of-Possession: BLS G2 signature over `bls_pub_key` using `BLS_POP_DST`, 96 bytes.
    pub bls_pop: Vec<u8>,
    /// Operator address (ADR 0084 S5): the key kept on the validator server, allowed only
    /// operational actions (unjail). Funds, bond and keys stay with the owner — the
    /// address that signs this payload. `None` = the owner operates its node itself.
    #[serde(default)]
    pub operator: Option<Address>,
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

    /// Canonical byte layout signed by the sender (and sponsor). Includes the sender's
    /// tagged `pub_key` but not the signatures.
    ///
    /// Addresses are the raw 20-byte payload (fixed length, so no length prefix).
    /// Layout: discriminant(1) ‖ pub_key(key_type(1) ‖ key) ‖ to(20) ‖ amount(16 BE) ‖ fee(16 BE) ‖
    /// nonce(8 BE) ‖ chain_id(4 BE) ‖ expiry(0 | 1‖8 BE) ‖ payload_len(4 BE) ‖ payload ‖
    /// sponsor(0 | 1‖20). Any client (web UI, SDK) must reproduce this exactly.
    ///
    /// Every variable-length field is length-prefixed or flag-delimited, so the encoding
    /// is injective: two different transactions can never share signing bytes (VINX-12).
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(128);
        bytes.push(self.tx_type.discriminant());
        // ADR 0081 D6: the signer's tagged public key (key type ‖ key) replaces the old
        // `from` address, so the signature commits to the exact key and its scheme.
        bytes.extend_from_slice(&self.pub_key.to_tagged_bytes());
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
        // therefore an identical txid, since `hash()` is `hash256(signing_bytes())`. One
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
        hash256(&self.signing_bytes())
    }

    /// Signs the transaction in place using the provided keypair.
    pub fn sign(&mut self, keypair: &KeyPair) {
        // The signing bytes commit to the public key, so set it first.
        self.pub_key = keypair.public_key();
        self.signature = Some(keypair.sign(&self.signing_bytes()));
    }

    /// Sender address, derived from the tagged public key (ADR 0081 D6).
    pub fn sender(&self) -> Address {
        Address::from_public_key(&self.pub_key)
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
        let mut tx = Self {
            tx_type: TransactionType::Transfer,
            to,
            amount,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: pk,
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
            to,
            amount,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: pk,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Construit et signe un `Stake` portant la clé BLS du validateur (ADR 0075 §3.1).
    ///
    /// Un bond qui fait **entrer** au pool de validateurs doit transporter la clé BLS et sa
    /// Proof-of-Possession : `quorum()` compte tous les membres du set, y compris ceux qui
    /// ne pourraient pas produire de bloc accepté, donc un membre sans clé rapproche le
    /// réseau d'un quorum inatteignable. Utiliser [`Transaction::new_stake`] pour un simple
    /// abondement d'un bond existant.
    ///
    /// `chain_id` est requis parce que la PoP y est liée (VINX-11) : elle ne vaut que pour
    /// cette clé, ce validateur et cette chaîne.
    pub fn new_stake_with_bls(
        keypair: &KeyPair,
        amount: Amount,
        fee: Amount,
        nonce: u64,
        bls_sk: &vinx_crypto::BlsSecretKey,
        chain_id: u32,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let payload = RegisterBlsKeyPayload {
            bls_pub_key: bls_sk.public_key().0.to_vec(),
            bls_pop: bls_sk
                .proof_of_possession(from.as_bytes(), chain_id)
                .0
                .to_vec(),
            operator: None,
        };
        let mut tx = Self {
            tx_type: TransactionType::Stake,
            to: from,
            amount,
            fee,
            nonce,
            chain_id,
            expires_at_height: None,
            payload: borsh::to_vec(&payload)
                .expect("RegisterBlsKeyPayload serialization is infallible"),
            pub_key: pk,
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
            to,
            amount,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: pk,
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
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: pk,
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

    /// Constructs a SlashValidator tx carrying vote-equivocation evidence (ADR 0082).
    pub fn new_slash_validator(
        keypair: &KeyPair,
        validator: Address,
        evidence: &crate::consensus::VoteEquivocation,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let payload = borsh::to_vec(evidence).expect("slash evidence serializable");
        let mut tx = Self {
            tx_type: TransactionType::SlashValidator,
            to: validator,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: pk,
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
        let payload = borsh::to_vec(action).expect("GovernanceAction serialization is infallible");
        let mut tx = Self {
            tx_type: TransactionType::AdminAction,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload,
            pub_key: pk,
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
            borsh::to_vec(payload).expect("RegisterBlsKeyPayload serialization is infallible");
        let mut tx = Self {
            tx_type: TransactionType::RegisterBlsKey,
            to: from,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: raw,
            pub_key: pk,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// Constructs and signs an Unjail transaction (ADR 0027) for the keypair's own address.
    /// Transfer carrying a memo (ADR 0085), at most `MAX_MEMO_BYTES` bytes. The memo is
    /// the transfer's `payload`, covered by the signature.
    pub fn new_transfer_with_memo(
        keypair: &KeyPair,
        to: Address,
        amount: Amount,
        fee: Amount,
        nonce: u64,
        memo: &[u8],
    ) -> Self {
        let mut tx = Self::new_transfer(keypair, to, amount, fee, nonce);
        tx.payload = memo.to_vec();
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    /// The memo of a transfer (its payload), if any.
    pub fn memo(&self) -> Option<&[u8]> {
        (self.tx_type == TransactionType::Transfer && !self.payload.is_empty())
            .then_some(self.payload.as_slice())
    }

    /// Sets or lifts the "memo required" option on the signer's account (ADR 0085).
    pub fn new_set_memo_required(
        keypair: &KeyPair,
        required: bool,
        fee: Amount,
        nonce: u64,
    ) -> Self {
        let pk = keypair.public_key();
        let from = Address::from_public_key(&pk);
        let mut tx = Self {
            tx_type: TransactionType::SetMemoRequired,
            to: from,
            amount: Amount::ZERO,
            fee,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![required as u8],
            pub_key: pk,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        tx.signature = Some(keypair.sign(&tx.signing_bytes()));
        tx
    }

    pub fn new_unjail(keypair: &KeyPair, nonce: u64) -> Self {
        let own = Address::from_public_key(&keypair.public_key());
        Self::new_unjail_for(keypair, own, nonce)
    }

    /// Unjail of `validator`, signed by its owner or its registered operator (ADR 0084 S5).
    pub fn new_unjail_for(keypair: &KeyPair, validator: Address, nonce: u64) -> Self {
        let pk = keypair.public_key();
        let mut tx = Self {
            tx_type: TransactionType::Unjail,
            to: validator,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
            pub_key: pk,
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
        assert_eq!(tx.sender(), Address::from_public_key(&tx.pub_key));
    }

    /// Golden vector locking the canonical `signing_bytes` layout. Any external
    /// signer (the web UI in rpc/ui.rs, third-party wallets) must reproduce this
    /// exact preimage; changing it is a consensus-breaking protocol change.
    #[test]
    fn test_signing_bytes_golden_vector() {
        let tx = Transaction {
            tx_type: TransactionType::Transfer,
            pub_key: PublicKey::Ed25519([0x11; 32]),
            to: Address::from_bytes([0x22; 20]),
            amount: Amount::from_atoms(1_000_000),
            fee: Amount::from_atoms(500),
            nonce: 7,
            chain_id: 42,
            expires_at_height: None,
            payload: vec![],
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };
        // disc(01) ‖ pub_key(00 ‖ 0x11×32) ‖ to(0x22×20) ‖ amount(16 BE) ‖ fee(16 BE)
        //   ‖ nonce(8 BE) ‖ chain_id(4 BE) ‖ expiry(00) ‖ payload_len(4 BE) ‖ sponsor(00)
        let expected = concat!(
            "01",
            "00",
            "1111111111111111111111111111111111111111111111111111111111111111",
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
        let pk = PublicKey::Ed25519(std::array::from_fn(|i| (i + 1) as u8)); // 01..20
        let admin = Address::from_bytes(std::array::from_fn(|i| (i + 1) as u8)); // 01..14
        let to = Address::from_bytes(std::array::from_fn(|i| (i + 21) as u8)); // 15..28
        let mk = |tx_type: TransactionType, to: Address, payload: Vec<u8>| Transaction {
            tx_type,
            pub_key: pk.clone(),
            to,
            amount: Amount::ZERO,
            fee: Amount::ZERO,
            nonce: 7,
            chain_id: 42,
            expires_at_height: None,
            payload,
            signature: None,
            sponsor: None,
            sponsor_pub_key: None,
            sponsor_signature: None,
        };

        // ADR 0007: validator-set changes go through AdminAction (0x08) carrying a
        // borsh(GovernanceAction). The admin console (rpc/ui.rs) reproduces the same
        // borsh: enum tag u8 (AddValidator = 0) ‖ address(20 raw bytes).
        // Here `to` = an arbitrary admin address, amount/fee = 0.
        let mut add_payload = Vec::with_capacity(21);
        add_payload.push(0u8); // GovernanceAction::AddValidator
        add_payload.extend_from_slice(to.as_bytes());
        let add = mk(TransactionType::AdminAction, admin, add_payload);
        let expected_add = concat!(
            "08",                                                               // AdminAction discriminant
            "00",                                                               // key type Ed25519
            "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20", // pub_key
            "0102030405060708090a0b0c0d0e0f1011121314",                         // to    = 01..14
            "00000000000000000000000000000000",                                 // amount 0
            "00000000000000000000000000000000",                                 // fee 0
            "0000000000000007",                                                 // nonce 7
            "0000002a",                                                         // chain_id 42
            "00",                                                               // expiry None
            "00000015",                                 // payload_len = 21 (VINX-12)
            "00", // GovernanceAction::AddValidator (borsh u8 tag = 0)
            "15161718191a1b1c1d1e1f202122232425262728", // validator address = 15..28
            "00", // sponsor None
        );
        assert_eq!(hex::encode(add.signing_bytes()), expected_add);

        // Cross-check that the hand-built payload equals borsh(GovernanceAction) —
        // this is exactly what the in-browser admin console must reproduce.
        let gov = crate::governance::GovernanceAction::AddValidator(to);
        assert_eq!(add.payload, borsh::to_vec(&gov).unwrap());

        // AnnounceUpgrade: to = self, payload = major(1) minor(2) patch(3) activation_ts(1000).
        let mut pl = Vec::with_capacity(14);
        pl.extend_from_slice(&1u16.to_be_bytes());
        pl.extend_from_slice(&2u16.to_be_bytes());
        pl.extend_from_slice(&3u16.to_be_bytes());
        pl.extend_from_slice(&1000u64.to_be_bytes());
        let upgrade = mk(TransactionType::AnnounceUpgrade, admin, pl);
        assert_eq!(
            hex::encode(upgrade.signing_bytes()),
            "04000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20\
             0102030405060708090a0b0c0d0e0f1011121314\
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
        let pub_key = PublicKey::Ed25519([0x11; 32]);
        let to = Address::from_bytes([0x22; 20]);
        // Worst case: a sponsor address whose last byte is 0x00.
        let mut sponsor_raw = [0x33u8; 20];
        sponsor_raw[19] = 0x00;
        let sponsor = Address::from_bytes(sponsor_raw);

        let mk = |payload: Vec<u8>, sponsor: Option<Address>| Transaction {
            tx_type: TransactionType::Transfer,
            pub_key: pub_key.clone(),
            to,
            amount: Amount::from_atoms(1),
            fee: Amount::from_atoms(1),
            nonce: 0,
            chain_id: 42,
            expires_at_height: None,
            payload,
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
            pub_key: PublicKey::Ed25519([0x11; 32]),
            to: Address::from_bytes([0x22; 20]),
            amount: Amount::from_atoms(1),
            fee: Amount::from_atoms(1),
            nonce: 0,
            chain_id: 42,
            expires_at_height: None,
            payload,
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
        let pk = &tx.pub_key;
        let sig = tx.signature.as_ref().unwrap();
        assert!(pk.verify(&tx.signing_bytes(), sig).is_ok());
    }

    #[test]
    fn test_nonce_is_in_signing_bytes() {
        let sender = KeyPair::generate();
        let to = Address::from_public_key(&KeyPair::generate().public_key());
        let fee = Amount::from_atoms(DEFAULT_FEE_FLOOR_ATOMS);
        let tx0 = Transaction::new_transfer(&sender, to, Amount::from_vinx(10), fee, 0);
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
            pub_key: PublicKey::Ed25519([0x11; 32]),
            to: Address::from_bytes([0x22; 20]),
            amount: Amount::from_atoms(5),
            fee: Amount::from_atoms(1),
            nonce: 0,
            chain_id: CHAIN_ID_DEVNET,
            expires_at_height: None,
            payload: vec![],
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
