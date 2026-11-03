//! Chaîne locale : état courant, vérification et construction des blocs.

use vinx_consensus::{HeaderError, check_header_link};
use vinx_core::params::MAX_CLOCK_DRIFT_MS;
use vinx_core::{
    Block, BlockHeader, CommitCertificate, LAST_COMMIT_PROTOCOL, SignedAdminTransaction,
    SignedTransaction, block_tx_root,
};
use vinx_crypto::Hash;
use vinx_state::{BlockError, State};

#[derive(Debug, PartialEq, Eq)]
pub enum CheckError {
    Header(HeaderError),
    State(BlockError),
}

pub struct Chain {
    pub state: State,
    /// En-tête du dernier bloc finalisé ; `None` juste après la genèse.
    pub last: Option<BlockHeader>,
    /// Racine d'état de la genèse : `prev_hash` du bloc 1 (SPEC §12).
    pub genesis_root: Hash,
    pub genesis_time_ms: u64,
    /// Certificat du dernier bloc, complété des precommits reçus après la
    /// décision : c'est le `last_commit` du prochain bloc (protocole 7).
    pub last_commit: Option<CommitCertificate>,
}

impl Chain {
    pub fn parent_hash(&self) -> Hash {
        self.last
            .as_ref()
            .map_or(self.genesis_root, BlockHeader::hash)
    }

    pub fn parent_time_ms(&self) -> u64 {
        self.last
            .as_ref()
            .map_or(self.genesis_time_ms, |h| h.timestamp)
    }

    /// Ajoute au certificat du dernier bloc un precommit arrivé après la
    /// décision. Renvoie `true` s'il était nouveau et valide.
    pub fn add_late_precommit(
        &mut self,
        vote: &vinx_core::Vote,
        validator: u16,
        signature: &vinx_crypto::Signature,
    ) -> bool {
        let Some(cert) = &mut self.last_commit else {
            return false;
        };
        if vote.kind != vinx_core::VoteKind::Precommit
            || vote.chain_id != self.state.chain_id
            || vote.height != cert.height
            || vote.round != cert.round
            || vote.block_hash != Some(cert.block_hash)
        {
            return false;
        }
        let Err(at) = cert
            .signatures
            .binary_search_by_key(&validator, |(i, _)| *i)
        else {
            return false;
        };
        let set = self.state.validators_at(cert.height);
        let valid = set
            .get(usize::from(validator))
            .is_some_and(|key| key.verify(&vote.signing_message(), signature).is_ok());
        if valid {
            cert.signatures.insert(at, (validator, signature.clone()));
        }
        valid
    }

    pub fn next_height(&self) -> u64 {
        self.state.height.saturating_add(1)
    }

    /// Vérifie un bloc proposé et renvoie l'état qui en résulterait.
    pub fn check(&self, block: &Block, now_ms: u64) -> Result<State, CheckError> {
        let h = &block.header;
        match &self.last {
            Some(parent) => check_header_link(parent, h, now_ms).map_err(CheckError::Header)?,
            None => {
                if h.height != 1 {
                    return Err(CheckError::Header(HeaderError::WrongHeight));
                }
                if h.prev_hash != self.genesis_root {
                    return Err(CheckError::Header(HeaderError::WrongParent));
                }
                if h.timestamp <= self.genesis_time_ms {
                    return Err(CheckError::Header(HeaderError::TimestampNotIncreasing));
                }
                if h.timestamp > now_ms.saturating_add(MAX_CLOCK_DRIFT_MS) {
                    return Err(CheckError::Header(HeaderError::TimestampInFuture));
                }
            }
        }
        let mut next = self.state.clone();
        next.apply_block(block).map_err(CheckError::State)?;
        Ok(next)
    }

    /// Construit un bloc avec les transactions applicables, dans l'ordre donné.
    /// Les transactions refusées sont simplement écartées.
    pub fn build(
        &self,
        timestamp_ms: u64,
        proposer: u16,
        admin_candidates: &[SignedAdminTransaction],
        candidates: &[SignedTransaction],
        max_bytes: usize,
    ) -> Block {
        let height = self.next_height();
        let mut admin: Vec<SignedAdminTransaction> = Vec::new();
        for a in admin_candidates {
            admin.push(a.clone());
            if self
                .state
                .execute(height, timestamp_ms, &admin, &[])
                .is_err()
            {
                admin.pop();
            }
        }
        let mut txs = Vec::new();
        if let Ok(mut work) = self.state.execute(height, timestamp_ms, &admin, &[]) {
            // `apply_tx` prend la hauteur du bloc en cours à `height + 1` :
            // on la ramène à celle du parent, comme pendant `execute`.
            work.height = self.state.height;
            let limit = usize::try_from(work.params.max_block_txs)
                .unwrap_or(usize::MAX)
                .saturating_sub(admin.len());
            let mut bytes = 0usize;
            let budget = max_bytes.saturating_sub(64 * 1024); // marge pour l'en-tête et l'admin
            let mut remaining: Vec<&SignedTransaction> = candidates.iter().collect();
            // Plusieurs passes : une transaction peut ne devenir applicable qu'après
            // une autre du même compte placée plus loin (frais plus faibles).
            loop {
                let before = txs.len();
                remaining.retain(|tx| {
                    if txs.len() >= limit {
                        return false;
                    }
                    let size = vinx_core::encode(*tx).len();
                    if bytes.saturating_add(size) > budget {
                        return true;
                    }
                    if work.apply_tx(tx).is_ok() {
                        bytes = bytes.saturating_add(size);
                        txs.push((*tx).clone());
                        false
                    } else {
                        true
                    }
                });
                if txs.len() == before || remaining.is_empty() {
                    break;
                }
            }
        }
        // Protocole 7 : le bloc porte le certificat de son parent.
        let last_commit = (height > 1 && self.state.version_at(height) >= LAST_COMMIT_PROTOCOL)
            .then(|| self.last_commit.clone())
            .flatten();
        // `execute` ne peut pas échouer ici : chaque transaction a été appliquée avec succès.
        let next = self
            .state
            .execute_block(
                height,
                timestamp_ms,
                Some(proposer),
                last_commit.as_ref(),
                &admin,
                &txs,
            )
            .unwrap_or_else(|_| self.state.clone());
        Block {
            header: BlockHeader {
                protocol_version: next.protocol_version,
                chain_id: self.state.chain_id.clone(),
                height,
                timestamp: timestamp_ms,
                prev_hash: self.parent_hash(),
                tx_root: block_tx_root(next.protocol_version, &admin, &txs, last_commit.as_ref()),
                state_root: next.state_root(),
                validators_hash: next.next_validators_hash(),
                proposer,
            },
            admin_txs: admin,
            txs,
            last_commit,
        }
    }

    /// Adopte un bloc finalisé, dont l'état résultant a déjà été calculé.
    pub fn advance(&mut self, block: &Block, cert: &CommitCertificate, next: State) {
        self.state = next;
        self.last = Some(block.header.clone());
        self.last_commit = Some(cert.clone());
    }
}
