//! Messages de consensus signés (SPEC §7.3).

use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{Block, Vote, encode};
use vinx_crypto::{Domain, Hash, KeyPair, PublicKey, Signature, tagged};

/// Proposition d'un bloc pour un round.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Proposal {
    pub chain_id: String,
    pub height: u64,
    pub round: u32,
    /// Round où ce bloc a obtenu un quorum de prevotes, s'il est re-proposé.
    pub pol_round: Option<u32>,
    pub block: Block,
}

impl Proposal {
    /// Message signé : `"vinx/proposal" ‖ borsh(chain_id, height, round, pol_round, block_hash)`.
    pub fn signing_message(&self) -> Vec<u8> {
        let content = (
            &self.chain_id,
            self.height,
            self.round,
            self.pol_round,
            self.block.hash(),
        );
        tagged(Domain::Proposal, &encode(&content))
    }

    pub fn sign(self, keypair: &KeyPair) -> SignedProposal {
        let signature = keypair.sign(&self.signing_message());
        SignedProposal {
            proposal: self,
            signature,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedProposal {
    pub proposal: Proposal,
    pub signature: Signature,
}

impl SignedProposal {
    pub fn verify(&self, proposer: &PublicKey) -> bool {
        proposer
            .verify(&self.proposal.signing_message(), &self.signature)
            .is_ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SignedVote {
    pub vote: Vote,
    pub validator: u16,
    pub signature: Signature,
}

impl SignedVote {
    pub fn new(vote: Vote, validator: u16, keypair: &KeyPair) -> Self {
        let signature = vote.sign(keypair);
        Self {
            vote,
            validator,
            signature,
        }
    }

    pub fn verify(&self, key: &PublicKey) -> bool {
        key.verify(&self.vote.signing_message(), &self.signature)
            .is_ok()
    }

    pub fn block_hash(&self) -> Option<Hash> {
        self.vote.block_hash
    }
}
