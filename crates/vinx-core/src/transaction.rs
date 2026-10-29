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
    // Protocole 2 et suivants.
    /// Enregistre `nom.vinx` pour un an, ou le renouvelle si l'émetteur le possède.
    RegisterName {
        name: String,
    } = 0x04,
    /// Cède un nom possédé, non expiré.
    TransferName {
        name: String,
        to: Address,
    } = 0x05,
    /// Bloque `amount` en séquestre au profit de `seller` (SPEC §5.5).
    OpenDeal {
        seller: Address,
        arbiter: Option<Address>,
        amount: Amount,
        /// Horodatage Unix (secondes) après lequel l'acheteur peut se rembourser.
        deadline: u64,
        memo: Option<Vec<u8>>,
    } = 0x06,
    /// Verse le séquestre au vendeur : acheteur ou arbitre.
    ReleaseDeal {
        id: Hash,
    } = 0x07,
    /// Rend le séquestre à l'acheteur : vendeur, arbitre, ou acheteur après l'échéance.
    RefundDeal {
        id: Hash,
    } = 0x08,
    // Protocole 4 et suivants.
    /// Plusieurs paiements en une transaction (SPEC §5.6).
    BatchTransfer {
        payments: Vec<Payment>,
    } = 0x09,
    /// Horodatage : inscrit l'empreinte d'un document (SPEC §5.7).
    Attest {
        digest: Hash,
    } = 0x0A,
    /// Règles du compte : signataires, seuil, plafond journalier (SPEC §5.8).
    SetPolicy {
        policy: PolicyConfig,
    } = 0x0B,
    /// `body`, approuvé par des signataires de la politique du compte (SPEC §5.8).
    Approved {
        body: Box<TxBody>,
        approvals: Vec<crate::Approval>,
    } = 0x0C,
    /// Autorise `payee` à prélever sur le compte émetteur (SPEC §5.9).
    /// Remplace un mandat existant pour le même bénéficiaire.
    GrantMandate {
        payee: Address,
        /// Montant maximal prélevé par période.
        max_per_period: Amount,
        /// Durée d'une période, en secondes (1 jour à 366 jours).
        period: u64,
        /// Horodatage Unix (secondes) de fin du mandat, s'il en a une.
        expires_at: Option<u64>,
    } = 0x0D,
    /// Révoque le mandat accordé à `payee`.
    RevokeMandate {
        payee: Address,
    } = 0x0E,
    /// Prélève sur `payer`, dans les limites de son mandat ; envoyé par le bénéficiaire.
    Collect {
        payer: Address,
        amount: Amount,
        memo: Option<Vec<u8>>,
    } = 0x0F,
    /// `body`, dont les frais sont payés par le compte `sponsor` (SPEC §5.10).
    Sponsored {
        body: Box<TxBody>,
        sponsor: Address,
        signature: Signature,
    } = 0x10,
}

/// Durée minimale d'une période de mandat : un jour.
pub const MIN_MANDATE_PERIOD: u64 = 86_400;
/// Durée maximale d'une période de mandat : 366 jours.
pub const MAX_MANDATE_PERIOD: u64 = 366 * 86_400;

/// Règles d'un compte (SPEC §5.8). Tout à zéro : pas de règle.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PolicyConfig {
    /// Clés qui peuvent approuver, au plus [`MAX_POLICY_SIGNERS`].
    pub signers: Vec<PublicKey>,
    /// Approbations exigées ; 0 : aucune.
    pub threshold: u8,
    /// Sans approbation, dépenses permises par jour (UTC) ; `None` : toute
    /// transaction exige `threshold` approbations.
    pub daily_limit: Option<Amount>,
}

/// Nombre maximal de signataires d'une politique de compte.
pub const MAX_POLICY_SIGNERS: usize = 10;

/// Un paiement d'un [`TxBody::BatchTransfer`].
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Payment {
    pub to: Address,
    pub amount: Amount,
    pub memo: Option<Vec<u8>>,
}

/// Nombre maximal de paiements d'un paiement groupé.
pub const MAX_BATCH_PAYMENTS: usize = 100;

/// Paiements couverts par un frais minimal dans un paiement groupé.
pub const PAYMENTS_PER_FEE: usize = 10;

impl TxBody {
    /// Version du protocole à partir de laquelle ce type de transaction est accepté.
    pub fn min_protocol(&self) -> u32 {
        match self {
            TxBody::Transfer { .. } | TxBody::SetMemoRequired { .. } | TxBody::RotateKey { .. } => {
                1
            }
            TxBody::BatchTransfer { .. }
            | TxBody::Attest { .. }
            | TxBody::SetPolicy { .. }
            | TxBody::Approved { .. }
            | TxBody::GrantMandate { .. }
            | TxBody::RevokeMandate { .. }
            | TxBody::Collect { .. }
            | TxBody::Sponsored { .. } => 4,
            _ => 2,
        }
    }

    /// Frais minimaux pour ce corps, `min_fee` étant celui du réseau : un
    /// paiement groupé paie un frais minimal par tranche de 10 paiements.
    pub fn min_fee(&self, min_fee: Amount) -> Amount {
        match self {
            TxBody::BatchTransfer { payments } => {
                let units = payments.len().div_ceil(PAYMENTS_PER_FEE).max(1);
                min_fee.saturating_mul(Amount::try_from(units).unwrap_or(Amount::MAX))
            }
            TxBody::Approved { body, .. } | TxBody::Sponsored { body, .. } => body.min_fee(min_fee),
            _ => min_fee,
        }
    }

    /// Corps effectif : celui qu'enveloppe un [`TxBody::Approved`].
    /// Enveloppes admises : `Sponsored { Approved { corps } }`, `Sponsored {
    /// corps }`, `Approved { corps }`.
    pub fn inner(&self) -> &TxBody {
        match self {
            TxBody::Sponsored { body, .. } => body.inner(),
            TxBody::Approved { body, .. } => body,
            other => other,
        }
    }

    /// Corps sans l'enveloppe du parrain.
    pub fn unsponsored(&self) -> &TxBody {
        match self {
            TxBody::Sponsored { body, .. } => body,
            other => other,
        }
    }

    /// Approbations des signataires du compte, s'il y en a.
    pub fn approvals(&self) -> Option<&[crate::Approval]> {
        match self.unsponsored() {
            TxBody::Approved { approvals, .. } => Some(approvals),
            _ => None,
        }
    }

    /// Les enveloppes sont-elles dans un ordre permis ?
    pub fn well_wrapped(&self) -> bool {
        let wrapped = |b: &TxBody| matches!(b, TxBody::Approved { .. } | TxBody::Sponsored { .. });
        match self {
            TxBody::Sponsored { body, .. } => match body.as_ref() {
                TxBody::Approved { body, .. } => !wrapped(body),
                other => !wrapped(other),
            },
            TxBody::Approved { body, .. } => !wrapped(body),
            _ => true,
        }
    }
}

/// Identifiant d'un séquestre : `H("vinx/deal" ‖ borsh(acheteur, nonce))`.
pub fn deal_id(buyer: &Address, nonce: u64) -> Hash {
    hash_tagged(Domain::Deal, &encode(&(buyer, nonce)))
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedTransaction {
    pub tx: Transaction,
    /// Obligatoire tant que la clé du compte n'est pas en état, interdite ensuite.
    pub public_key: Option<PublicKey>,
    pub signature: Signature,
}

impl Transaction {
    /// Message approuvé par un signataire de la politique du compte :
    /// `"vinx/tx/approval" ‖ borsh(chaîne, émetteur, nonce, frais, corps)`, où
    /// le corps est celui qu'enveloppe [`TxBody::Approved`].
    pub fn approval_message(&self) -> Vec<u8> {
        let inner = self.body.inner();
        tagged(
            Domain::TxApproval,
            &encode(&(&self.chain_id, &self.sender, self.nonce, self.fee, inner)),
        )
    }

    /// Message signé par le parrain qui paie les frais :
    /// `"vinx/tx/sponsor" ‖ borsh(chaîne, émetteur, nonce, frais, corps, parrain)`.
    pub fn sponsor_message(&self, sponsor: &Address) -> Vec<u8> {
        tagged(
            Domain::TxSponsor,
            &encode(&(
                &self.chain_id,
                &self.sender,
                self.nonce,
                self.fee,
                self.body.inner(),
                sponsor,
            )),
        )
    }

    /// Approbation de `keypair`, signataire `index` de la politique du compte.
    pub fn approve(&self, index: u8, keypair: &KeyPair) -> crate::Approval {
        crate::Approval {
            signer_index: index,
            signature: keypair.sign(&self.approval_message()),
        }
    }

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
        assert!(decode::<TxBody>(&[0x11, 1]).is_err());
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
