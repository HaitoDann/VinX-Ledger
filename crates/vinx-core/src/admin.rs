//! Transactions d'administration signées par la multisig (SPEC §5.3).

use crate::{Amount, CoreError, ParamId, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_crypto::{Address, Domain, Hash, KeyPair, PublicKey, Signature, hash_tagged, tagged};

/// Nombre de signataires de la multisig admin.
pub const ADMIN_SIGNERS: usize = 3;

/// Nombre d'approbations distinctes requises.
pub const ADMIN_THRESHOLD: usize = 2;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AdminTransaction {
    pub chain_id: String,
    /// Compteur propre à l'admin : doit valoir `nonce admin + 1`.
    pub nonce: u64,
    pub action: AdminAction,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum AdminAction {
    EscrowRelease {
        amount: Amount,
    } = 0x01,
    EscrowReturn {
        amount: Amount,
    } = 0x02,
    TreasuryTransfer {
        to: Address,
        amount: Amount,
        memo: Option<Vec<u8>>,
    } = 0x03,
    AddValidator {
        key: PublicKey,
    } = 0x04,
    RemoveValidator {
        key: PublicKey,
    } = 0x05,
    SetParam {
        param: ParamId,
        value: u64,
    } = 0x06,
    ReplaceSigner {
        index: u8,
        new_key: PublicKey,
    } = 0x07,
    ScheduleUpgrade {
        version: u32,
        activation_height: u64,
    } = 0x08,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Approval {
    pub signer_index: u8,
    pub signature: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedAdminTransaction {
    pub tx: AdminTransaction,
    /// Indices strictement croissants.
    pub approvals: Vec<Approval>,
}

impl AdminTransaction {
    /// Message signé : `"vinx/admin" ‖ borsh(tx)`.
    pub fn signing_message(&self) -> Vec<u8> {
        tagged(Domain::Admin, &encode(self))
    }

    /// Approbation d'un signataire.
    pub fn approve(&self, signer_index: u8, keypair: &KeyPair) -> Approval {
        Approval {
            signer_index,
            signature: keypair.sign(&self.signing_message()),
        }
    }
}

impl SignedAdminTransaction {
    /// `H("vinx/admin/txid" ‖ borsh(signed_admin_tx))`.
    pub fn id(&self) -> Hash {
        hash_tagged(Domain::AdminTxId, &encode(self))
    }

    /// Vérifie qu'au moins [`ADMIN_THRESHOLD`] signataires distincts ont approuvé.
    ///
    /// Les indices doivent être strictement croissants : une même clé ne peut pas
    /// compter deux fois, et l'encodage d'un ensemble d'approbations est unique.
    pub fn verify_approvals(&self, signers: &[PublicKey; ADMIN_SIGNERS]) -> Result<(), CoreError> {
        if self.approvals.len() < ADMIN_THRESHOLD {
            return Err(CoreError::InvalidApprovals("seuil non atteint"));
        }
        if self.approvals.len() > ADMIN_SIGNERS {
            return Err(CoreError::InvalidApprovals("trop d'approbations"));
        }
        let message = self.tx.signing_message();
        let mut previous: Option<u8> = None;
        for approval in &self.approvals {
            if previous.is_some_and(|p| approval.signer_index <= p) {
                return Err(CoreError::InvalidApprovals(
                    "indices non strictement croissants",
                ));
            }
            previous = Some(approval.signer_index);
            let key = signers
                .get(usize::from(approval.signer_index))
                .ok_or(CoreError::InvalidApprovals("indice de signataire inconnu"))?;
            key.verify(&message, &approval.signature)
                .map_err(|_| CoreError::InvalidApprovals("signature invalide"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode;

    fn setup() -> ([KeyPair; 3], [PublicKey; 3], AdminTransaction) {
        let kps = [
            KeyPair::generate(),
            KeyPair::generate(),
            KeyPair::generate(),
        ];
        let pks = [
            kps[0].public_key(),
            kps[1].public_key(),
            kps[2].public_key(),
        ];
        let tx = AdminTransaction {
            chain_id: "vinx-test".into(),
            nonce: 1,
            action: AdminAction::EscrowRelease { amount: 42 },
        };
        (kps, pks, tx)
    }

    #[test]
    fn action_codes_match_spec() {
        let k = KeyPair::generate().public_key();
        let codes = [
            AdminAction::EscrowRelease { amount: 1 },
            AdminAction::EscrowReturn { amount: 1 },
            AdminAction::TreasuryTransfer {
                to: Address::treasury(),
                amount: 1,
                memo: None,
            },
            AdminAction::AddValidator { key: k.clone() },
            AdminAction::RemoveValidator { key: k.clone() },
            AdminAction::SetParam {
                param: ParamId::MinFee,
                value: 1,
            },
            AdminAction::ReplaceSigner {
                index: 0,
                new_key: k,
            },
            AdminAction::ScheduleUpgrade {
                version: 2,
                activation_height: 1,
            },
        ]
        .map(|a| encode(&a)[0]);
        assert_eq!(codes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert!(decode::<AdminAction>(&[0x09, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn two_of_three_is_accepted() {
        let (kps, pks, tx) = setup();
        for (a, b) in [(0u8, 1u8), (0, 2), (1, 2)] {
            let stx = SignedAdminTransaction {
                approvals: vec![
                    tx.approve(a, &kps[usize::from(a)]),
                    tx.approve(b, &kps[usize::from(b)]),
                ],
                tx: tx.clone(),
            };
            assert_eq!(stx.verify_approvals(&pks), Ok(()));
        }
    }

    #[test]
    fn invalid_approval_sets_are_rejected() {
        let (kps, pks, tx) = setup();
        let a0 = tx.approve(0, &kps[0]);
        let a1 = tx.approve(1, &kps[1]);
        let cases = [
            vec![a0.clone()],                         // une seule
            vec![a0.clone(), a0.clone()],             // doublon
            vec![a1.clone(), a0.clone()],             // ordre décroissant
            vec![a0.clone(), tx.approve(1, &kps[2])], // mauvaise clé pour l'indice
            vec![a0.clone(), tx.approve(3, &kps[1])], // indice inconnu
        ];
        for approvals in cases {
            let stx = SignedAdminTransaction {
                tx: tx.clone(),
                approvals,
            };
            assert!(stx.verify_approvals(&pks).is_err());
        }
    }

    #[test]
    fn approvals_are_bound_to_the_action() {
        let (kps, pks, tx) = setup();
        let approvals = vec![tx.approve(0, &kps[0]), tx.approve(1, &kps[1])];
        let mut forged = SignedAdminTransaction { tx, approvals };
        forged.tx.action = AdminAction::EscrowRelease { amount: 43 };
        assert!(forged.verify_approvals(&pks).is_err());
    }

    /// Une signature de transaction utilisateur n'est pas une approbation admin.
    #[test]
    fn user_signature_is_not_an_approval() {
        let (kps, pks, tx) = setup();
        let raw = encode(&tx);
        let wrong_domain = Approval {
            signer_index: 0,
            signature: kps[0].sign(&tagged(Domain::Tx, &raw)),
        };
        let stx = SignedAdminTransaction {
            tx: tx.clone(),
            approvals: vec![wrong_domain, tx.approve(1, &kps[1])],
        };
        assert!(stx.verify_approvals(&pks).is_err());
    }
}
