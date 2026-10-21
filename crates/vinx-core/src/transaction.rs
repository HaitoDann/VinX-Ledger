//! Transactions utilisateur (SPEC §5.1, §5.2).

use crate::{Amount, CoreError, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_crypto::{Address, Domain, Hash, KeyPair, PublicKey, Signature, hash_tagged, tagged};

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Transaction {
    pub chain_id: String,
    pub sender: Address,
    /// Doit valoir `nonce du compte + 1`.
    pub nonce: u64,
    /// Au moins `min_fee`, crédité à la trésorerie.
    pub fee: Amount,
    pub body: TxBody,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum TxBody {
    Transfer {
        to: Address,
        amount: Amount,
        memo: Option<Vec<u8>>,
    } = 0x01,
    SetMemoRequired {
        required: bool,
    } = 0x02,
    RotateKey {
        new_key: PublicKey,
    } = 0x03,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedTransaction {
    pub tx: Transaction,
    /// Obligatoire tant que la clé du compte n'est pas en état, interdite ensuite.
    pub public_key: Option<PublicKey>,
    pub signature: Signature,
}

impl Transaction {
    /// Message signé : `"vinx/tx" ‖ borsh(tx)`.
    pub fn signing_message(&self) -> Vec<u8> {
        tagged(Domain::Tx, &encode(self))
    }

    /// Signe la transaction. `include_key` joint la clé publique (première dépense).
    pub fn sign(self, keypair: &KeyPair, include_key: bool) -> SignedTransaction {
        let signature = keypair.sign(&self.signing_message());
        SignedTransaction {
            tx: self,
            public_key: include_key.then(|| keypair.public_key()),
            signature,
        }
    }
}

impl SignedTransaction {
    /// `H("vinx/txid" ‖ borsh(signed_tx))`.
    pub fn id(&self) -> Hash {
        hash_tagged(Domain::TxId, &encode(self))
    }

    /// Vérifie la signature avec la clé du compte (fournie par l'état ou jointe).
    pub fn verify_signature(&self, key: &PublicKey) -> Result<(), CoreError> {
        key.verify(&self.tx.signing_message(), &self.signature)
            .map_err(|_| CoreError::InvalidSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    fn transfer(kp: &KeyPair) -> Transaction {
        Transaction {
            chain_id: "vinx-test".into(),
            sender: Address::from_public_key(&kp.public_key()),
            nonce: 1,
            fee: 1_000_000,
            body: TxBody::Transfer {
                to: Address::treasury(),
                amount: 5,
                memo: Some(b"FAC-1".to_vec()),
            },
        }
    }

    #[test]
    fn body_codes_match_spec() {
        let codes = [
            TxBody::Transfer {
                to: Address::treasury(),
                amount: 1,
                memo: None,
            },
            TxBody::SetMemoRequired { required: true },
            TxBody::RotateKey {
                new_key: KeyPair::generate().public_key(),
            },
        ]
        .map(|b| encode(&b)[0]);
        assert_eq!(codes, [0x01, 0x02, 0x03]);
        assert!(decode::<TxBody>(&[0x00, 1]).is_err());
        assert!(decode::<TxBody>(&[0x04, 1]).is_err());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let kp = KeyPair::generate();
        let stx = transfer(&kp).sign(&kp, true);
        assert_eq!(stx.public_key, Some(kp.public_key()));
        assert_eq!(stx.verify_signature(&kp.public_key()), Ok(()));
        let bytes = encode(&stx);
        assert_eq!(decode::<SignedTransaction>(&bytes).unwrap(), stx);
        let mut extra = bytes;
        extra.push(0);
        assert!(decode::<SignedTransaction>(&extra).is_err());
    }

    #[test]
    fn any_change_invalidates_signature() {
        let kp = KeyPair::generate();
        let stx = transfer(&kp).sign(&kp, false);
        for mutate in [
            |t: &mut Transaction| t.chain_id = "autre-chaine".into(),
            |t: &mut Transaction| t.nonce = 2,
            |t: &mut Transaction| t.fee = 0,
            |t: &mut Transaction| t.body = TxBody::SetMemoRequired { required: false },
        ] {
            let mut forged = stx.clone();
            mutate(&mut forged.tx);
            assert_eq!(
                forged.verify_signature(&kp.public_key()),
                Err(CoreError::InvalidSignature)
            );
        }
    }

    #[test]
    fn id_covers_signature_and_key() {
        let kp = KeyPair::generate();
        let with_key = transfer(&kp).sign(&kp, true);
        let without_key = transfer(&kp).sign(&kp, false);
        assert_ne!(with_key.id(), without_key.id());
    }
}
