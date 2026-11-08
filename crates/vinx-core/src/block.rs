//! Blocs (SPEC §6).

use crate::{CommitCertificate, SignedAdminTransaction, SignedTransaction, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use std::io::{Read, Write};
use vinx_crypto::Signature;
use vinx_crypto::{Domain, Hash, hash_tagged, merkle_root};

/// Version du protocole à partir de laquelle un bloc porte `last_commit`
/// (docs/PROTOCOLE-7.md §5).
pub const LAST_COMMIT_PROTOCOL: u32 = 7;

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlockHeader {
    pub protocol_version: u32,
    pub chain_id: String,
    pub height: u64,
    /// Millisecondes Unix.
    pub timestamp: u64,
    pub prev_hash: Hash,
    /// Merkle des `admin_tx_id` puis des `tx_id`, dans l'ordre du bloc.
    pub tx_root: Hash,
    /// État après application du bloc.
    pub state_root: Hash,
    /// Ensemble de validateurs du bloc suivant.
    pub validators_hash: Hash,
    /// Indice du proposeur dans l'ensemble actif.
    pub proposer: u16,
}

/// Bloc. L'encodage borsh est manuel : `last_commit` n'est écrit qu'à partir
/// du protocole 7 (selon `header.protocol_version`), pour que les blocs plus
/// anciens gardent exactement leur encodage d'origine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub header: BlockHeader,
    pub admin_txs: Vec<SignedAdminTransaction>,
    pub txs: Vec<SignedTransaction>,
    /// Protocole 7 : certificat du bloc précédent, avec toutes les signatures
    /// reçues par le proposeur. `None` au bloc 1 et avant le protocole 7.
    pub last_commit: Option<CommitCertificate>,
    /// Protocole 7 : precommits du bloc `h - 2` absents du `last_commit` du
    /// bloc précédent (indice dans l'ensemble de `h - 2`, signature). Le
    /// proposeur suivant répare ainsi une omission (docs/PROTOCOLE-7.md §5).
    pub late_signatures: Vec<(u16, Signature)>,
}

impl BorshSerialize for Block {
    fn serialize<W: Write>(&self, writer: &mut W) -> std::io::Result<()> {
        self.header.serialize(writer)?;
        self.admin_txs.serialize(writer)?;
        self.txs.serialize(writer)?;
        if self.header.protocol_version >= LAST_COMMIT_PROTOCOL {
            self.last_commit.serialize(writer)?;
            self.late_signatures.serialize(writer)?;
        }
        Ok(())
    }
}

impl BorshDeserialize for Block {
    fn deserialize_reader<R: Read>(reader: &mut R) -> std::io::Result<Self> {
        let header = BlockHeader::deserialize_reader(reader)?;
        let admin_txs = Vec::deserialize_reader(reader)?;
        let txs = Vec::deserialize_reader(reader)?;
        let (last_commit, late_signatures) = if header.protocol_version >= LAST_COMMIT_PROTOCOL {
            (
                Option::deserialize_reader(reader)?,
                Vec::deserialize_reader(reader)?,
            )
        } else {
            (None, Vec::new())
        };
        Ok(Self {
            header,
            admin_txs,
            txs,
            last_commit,
            late_signatures,
        })
    }
}

/// Racine des transactions : Merkle des `admin_tx_id`, des `tx_id` puis, à
/// partir du protocole 7, de l'empreinte de `last_commit` et de celle des
/// signatures rattrapées (s'il y en a).
pub fn block_tx_root(
    protocol_version: u32,
    admin_txs: &[SignedAdminTransaction],
    txs: &[SignedTransaction],
    last_commit: Option<&CommitCertificate>,
    late_signatures: &[(u16, Signature)],
) -> Hash {
    let mut ids: Vec<Hash> = admin_txs
        .iter()
        .map(SignedAdminTransaction::id)
        .chain(txs.iter().map(SignedTransaction::id))
        .collect();
    if protocol_version >= LAST_COMMIT_PROTOCOL {
        if let Some(cert) = last_commit {
            ids.push(cert.hash());
        }
        if !late_signatures.is_empty() {
            ids.push(hash_tagged(
                Domain::Commit,
                &encode(&late_signatures.to_vec()),
            ));
        }
    }
    merkle_root(&ids)
}

impl BlockHeader {
    /// `H("vinx/block" ‖ borsh(header))`.
    pub fn hash(&self) -> Hash {
        hash_tagged(Domain::Block, &encode(self))
    }
}

impl Block {
    pub fn hash(&self) -> Hash {
        self.header.hash()
    }

    /// Racine des transactions du bloc, à comparer à `header.tx_root`.
    pub fn compute_tx_root(&self) -> Hash {
        block_tx_root(
            self.header.protocol_version,
            &self.admin_txs,
            &self.txs,
            self.last_commit.as_ref(),
            &self.late_signatures,
        )
    }

    /// Taille encodée, à comparer à `max_block_bytes`.
    pub fn encoded_len(&self) -> usize {
        encode(self).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdminAction, AdminTransaction, Transaction, TxBody, decode};
    use vinx_crypto::{Address, KeyPair};

    fn block() -> Block {
        let kp = KeyPair::generate();
        let tx = Transaction {
            chain_id: "vinx-test".into(),
            sender: Address::from_public_key(&kp.public_key()),
            nonce: 1,
            fee: 1,
            body: TxBody::SetMemoRequired { required: true },
        }
        .sign(&kp, true);
        let admin = AdminTransaction {
            chain_id: "vinx-test".into(),
            nonce: 1,
            action: AdminAction::EscrowRelease { amount: 1 },
        };
        let admin = SignedAdminTransaction {
            approvals: vec![admin.approve(0, &kp)],
            tx: admin,
        };
        let mut b = Block {
            header: BlockHeader {
                protocol_version: 1,
                chain_id: "vinx-test".into(),
                height: 1,
                timestamp: 1_700_000_000_000,
                prev_hash: [0; 32],
                tx_root: [0; 32],
                state_root: [1; 32],
                validators_hash: [2; 32],
                proposer: 0,
            },
            admin_txs: vec![admin],
            txs: vec![tx],
            last_commit: None,
            late_signatures: Vec::new(),
        };
        b.header.tx_root = b.compute_tx_root();
        b
    }

    #[test]
    fn roundtrip_and_hash() {
        let b = block();
        let bytes = encode(&b);
        assert_eq!(decode::<Block>(&bytes).unwrap(), b);
        assert_eq!(b.encoded_len(), bytes.len());
        let mut other = b.header.clone();
        other.timestamp += 1;
        assert_ne!(other.hash(), b.hash());
    }

    #[test]
    fn tx_root_covers_order_and_content() {
        let b = block();
        let ids = [b.admin_txs[0].id(), b.txs[0].id()];
        assert_eq!(b.compute_tx_root(), merkle_root(&ids));
        let mut swapped = b.clone();
        swapped.txs.clear();
        assert_ne!(swapped.compute_tx_root(), b.header.tx_root);
    }

    #[test]
    fn last_commit_is_encoded_from_protocol_7_only() {
        let mut b = block();
        let old = encode(&b);
        let cert = CommitCertificate {
            height: 0,
            round: 1,
            block_hash: [3; 32],
            signatures: vec![],
        };
        // Avant le protocole 7 : encodage d'origine, le champ est ignoré.
        b.last_commit = Some(cert.clone());
        assert_eq!(encode(&b), old);
        assert_eq!(b.compute_tx_root(), b.header.tx_root);
        assert_eq!(decode::<Block>(&old).unwrap().last_commit, None);
        // À partir du protocole 7 : écrit, relu, et engagé dans `tx_root`.
        b.header.protocol_version = 7;
        let bytes = encode(&b);
        assert_eq!(
            bytes.len(),
            old.len() + encode(&Some(cert.clone())).len() + 4,
            "certificat et liste vide de signatures rattrapées"
        );
        assert_eq!(decode::<Block>(&bytes).unwrap(), b);
        assert_ne!(b.compute_tx_root(), b.header.tx_root);
        let ids = [b.admin_txs[0].id(), b.txs[0].id(), cert.hash()];
        assert_eq!(b.compute_tx_root(), merkle_root(&ids));
        b.last_commit = None;
        assert_eq!(decode::<Block>(&encode(&b)).unwrap(), b);
        // Signatures rattrapées : encodées et engagées dans `tx_root`.
        let kp = KeyPair::generate();
        b.late_signatures = vec![(2, kp.sign(b"x"))];
        assert_eq!(decode::<Block>(&encode(&b)).unwrap(), b);
        let before = block_tx_root(7, &b.admin_txs, &b.txs, None, &[]);
        assert_ne!(b.compute_tx_root(), before);
    }
}
