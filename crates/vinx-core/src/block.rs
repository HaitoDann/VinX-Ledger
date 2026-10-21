//! Blocs (SPEC §6).

use crate::{SignedAdminTransaction, SignedTransaction, encode};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_crypto::{Domain, Hash, hash_tagged, merkle_root};

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

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Block {
    pub header: BlockHeader,
    pub admin_txs: Vec<SignedAdminTransaction>,
    pub txs: Vec<SignedTransaction>,
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
        let ids: Vec<Hash> = self
            .admin_txs
            .iter()
            .map(SignedAdminTransaction::id)
            .chain(self.txs.iter().map(SignedTransaction::id))
            .collect();
        merkle_root(&ids)
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
}
