use ahash::{AHashMap, AHashSet};

use serde::{Deserialize, Serialize};
use vinx_core::{block::GENESIS_PREV_HASH, Block, BlockHeader, Transaction};
use vinx_crypto::{Address, Hash32};

#[derive(Serialize, Deserialize)]
pub struct Chain {
    /// Stored as (block_hash, block) indexed by height.
    blocks: Vec<(Hash32, Block)>,
    /// Highest height whose block has reached quorum co-signatures (ADR 0002).
    /// Everything at or below is **final** — never reorganized. Genesis (0) is final.
    #[serde(default)]
    finalized_height: u64,
    /// Height of the first stored block. Zero for chains starting from genesis;
    /// non-zero for chains bootstrapped from a state snapshot (ADR snapshot-sync):
    /// `tip_height() = height_base + blocks.len() - 1`. Backward-compat: defaults to 0.
    #[serde(default)]
    pub height_base: u64,
    /// Maps raw tx hash -> (block_height, tx_position). Not persisted via serde
    /// (rebuilt or imported by Storage). Keyed by the 32-byte hash directly —
    /// no hex allocation per insert/lookup — hashed with ahash on the hot path.
    #[serde(skip)]
    tx_index: AHashMap<Hash32, (u64, u32)>,
    /// Maps address -> ordered list of tx hashes (oldest first). Not persisted via
    /// serde. Keyed by the raw 20-byte `Address` (Copy) rather than a bech32 String.
    #[serde(skip)]
    account_tx_index: AHashMap<Address, Vec<Hash32>>,
    /// Maps validator addr -> height -> set of block hashes signed (equivocation detection).
    #[serde(skip)]
    slash_evidence: AHashMap<Address, AHashMap<u64, AHashSet<Hash32>>>,
    /// Heights whose stored block changed since the last persistence flush (new
    /// block, co-signature landed, tx/sig data pruned). Drained by
    /// `take_dirty_heights` so Storage writes only those rows instead of
    /// re-serializing the whole chain on every persist.
    #[serde(skip)]
    dirty_heights: AHashSet<u64>,
    /// ADR 0002/0027 — **quorum historique**. Checkpoints `(from_height, quorum)` triés :
    /// le bloc de hauteur `h` requiert `quorum_at(h)` signatures pour être final. Reconstruit
    /// **déterministiquement** pendant l'application des blocs (le quorum du set **actif à la
    /// hauteur de chaque bloc**, capturé avant les changements de set de ce bloc), donc
    /// identique sur tous les nœuds. Sans lui, un bloc de genèse (peu de signataires, petit
    /// quorum d'alors) échoue au quorum courant après un ajout de validateur → la finalité se
    /// bloque pour les nœuds qui rejoignent. Non persisté : reconstruit à l'application ; après
    /// un reload, `quorum_at` retombe sur le quorum courant pour le petit suffixe non finalisé.
    #[serde(skip)]
    quorum_schedule: Vec<(u64, usize)>,
    /// ADR 0031 — **candidats de fork-choice**. Blocs valides **concurrents** (proposeur/hash
    /// différent) observés à une hauteur **non finalisée**, en plus du bloc actuellement retenu
    /// dans `blocks`. La règle `canonical_head` (poids de co-sigs → leader prévu → plus petit
    /// hash) élit la tête déterministiquement parmi {bloc retenu} ∪ candidats. Non persisté
    /// (reconstruit à la volée depuis le gossip) ; purgé sous la finalité (réorg interdite en
    /// dessous). **Tranche 2a : stockage + choix canonique seulement — aucune réorg encore.**
    #[serde(skip)]
    candidates: AHashMap<u64, Vec<Block>>,
}

impl Chain {
    pub fn new_with_genesis(validator: Address, timestamp: u64) -> (Self, Block) {
        let genesis = Block {
            header: BlockHeader {
                height: 0,
                prev_hash: GENESIS_PREV_HASH,
                timestamp,
                validator,
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            },
            transactions: vec![],
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
            bls_bitmap: vec![],
            vrf_proof: None,
        };
        let hash = genesis.hash();
        let chain = Self {
            blocks: vec![(hash, genesis.clone())],
            height_base: 0,
            finalized_height: 0,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            slash_evidence: AHashMap::new(),
            dirty_heights: AHashSet::from_iter([0]),
            quorum_schedule: Vec::new(),
            candidates: AHashMap::new(),
        };
        (chain, genesis)
    }

    /// Rebuilds a chain from persisted parts: the per-height block rows (dense,
    /// starting at `height_base`) and the finalized-height watermark. Tx indexes are
    /// restored or rebuilt separately by the caller.
    pub fn from_parts(blocks: Vec<(Hash32, Block)>, finalized_height: u64) -> Self {
        Self {
            blocks,
            height_base: 0,
            finalized_height,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            slash_evidence: AHashMap::new(),
            dirty_heights: AHashSet::new(),
            quorum_schedule: Vec::new(),
            candidates: AHashMap::new(),
        }
    }

    /// Bootstraps a chain from a state snapshot (snapshot-sync). The provided block
    /// is the only stored block; its height becomes `height_base` and `tip_height()`.
    /// Subsequent `push()` calls continue from there. The snapshot block is pre-finalized.
    pub fn new_from_snapshot(block: Block) -> Self {
        let height = block.header.height;
        let hash = block.hash();
        Self {
            blocks: vec![(hash, block)],
            height_base: height,
            finalized_height: height,
            tx_index: AHashMap::new(),
            account_tx_index: AHashMap::new(),
            slash_evidence: AHashMap::new(),
            dirty_heights: AHashSet::from_iter([height]),
            quorum_schedule: Vec::new(),
            candidates: AHashMap::new(),
        }
    }

    /// Drains and returns the set of heights whose block row must be rewritten.
    pub fn take_dirty_heights(&mut self) -> Vec<u64> {
        self.dirty_heights.drain().collect()
    }

    /// Marks every stored block dirty — used by full saves (genesis bootstrap,
    /// snapshot import) so the whole blocks table is rewritten.
    pub fn mark_all_dirty(&mut self) {
        self.dirty_heights =
            (self.height_base..self.height_base + self.blocks.len() as u64).collect();
    }

    /// Borrow of the stored `(hash, block)` row at `height`, for persistence.
    pub fn block_row(&self, height: u64) -> Option<&(Hash32, Block)> {
        let idx = height.checked_sub(self.height_base)? as usize;
        self.blocks.get(idx)
    }

    /// Highest final (quorum-signed) height. Everything at or below is irreversible.
    pub fn finalized_height(&self) -> u64 {
        self.finalized_height
    }

    /// True when `height` is final (quorum-signed and never reorganizable).
    pub fn is_final(&self, height: u64) -> bool {
        height <= self.finalized_height
    }

    /// Advances `finalized_height` over the contiguous prefix of quorum-signed blocks
    /// above the current mark (ADR 0002). Called after producing a block and after a
    /// co-signature lands. Finality is prefix-closed: it stops at the first block that
    /// has not yet reached quorum. Returns the new finalized height.
    ///
    /// `indexed_bls_pks` is the on-chain BLS registry indexed by validator position
    /// (`WorldState::indexed_bls_keys`). VINX-02/VINX-05: finality previously used
    /// `bls_signer_count()`, which counts keys the block carries itself — a block could
    /// declare its own quorum and be marked irreversible. Finality must be decided against
    /// keys registered on-chain, never against keys supplied by the block.
    pub fn advance_finality(
        &mut self,
        validator_set: &vinx_core::ValidatorSet,
        indexed_bls_pks: &[Option<[u8; 48]>],
    ) -> u64 {
        let tip = self.tip_height();
        // Seuil courant, utilisé en repli quand le schedule n'a pas d'entrée pour la hauteur
        // (ex. suffixe non finalisé après un reload : le schedule est reconstruit à
        // l'application, pas persisté).
        let fallback = validator_set.quorum();
        while self.finalized_height < tip {
            let next = self.finalized_height + 1;
            // ADR 0002/0027 : seuil = quorum **historique** à la hauteur `next`, pas le quorum
            // courant — sinon un bloc antérieur à un changement de set (moins de signataires)
            // bloquerait le préfixe.
            let threshold = self.quorum_at(next, fallback);
            let is_final = match self.block_row(next) {
                Some((_, b)) => b
                    .bls_signer_count_from_bitmap(indexed_bls_pks)
                    .map(|c| c >= threshold)
                    .unwrap_or(false),
                None => break,
            };
            if is_final {
                self.finalized_height = next;
            } else {
                break;
            }
        }
        // ADR 0031 — les candidats désormais sous la finalité ne peuvent plus gagner : purge.
        self.prune_candidates_final();
        self.finalized_height
    }

    /// ADR 0002/0027 — enregistre que les blocs à partir de `from_height` requièrent `quorum`
    /// signatures (checkpoint du quorum historique). Appelé pendant l'application de chaque
    /// bloc avec le quorum du set **actif à cette hauteur** (capturé avant les changements de
    /// set du bloc). N'ajoute un checkpoint que lorsque la valeur change ; les hauteurs sont
    /// notées dans l'ordre croissant.
    pub fn note_quorum(&mut self, from_height: u64, quorum: usize) {
        if self.quorum_schedule.last().map(|&(_, q)| q) != Some(quorum) {
            self.quorum_schedule.push((from_height, quorum));
        }
    }

    /// Quorum en vigueur à la hauteur `height` (dernier checkpoint `from_height <= height`).
    /// Retombe sur `fallback` si aucun checkpoint ne couvre la hauteur (schedule non reconstruit,
    /// ex. juste après un reload).
    pub fn quorum_at(&self, height: u64, fallback: usize) -> usize {
        self.quorum_schedule
            .iter()
            .rev()
            .find(|&&(h, _)| h <= height)
            .map(|&(_, q)| q)
            .unwrap_or(fallback)
    }

    /// ADR 0031 — enregistre un **candidat de fork-choice** : un bloc valide concurrent (hash
    /// différent) observé à une hauteur **non finalisée**. Le caller garantit la validité du
    /// bloc (proposeur ∈ set, co-signatures vérifiées) ; cette méthode ne fait que le ranger.
    ///
    /// Ne stocke **pas** : un bloc au-dessous ou à la finalité (réorg interdite en dessous), ni
    /// un doublon du bloc déjà retenu dans `blocks`, ni un candidat déjà connu (dédup par hash).
    /// Retourne `true` si un nouveau candidat a réellement été enregistré.
    ///
    /// **Tranche 2a : observation seulement — n'entraîne aucune réorganisation.**
    pub fn record_candidate(&mut self, block: Block) -> bool {
        let h = block.header.height;
        if h <= self.finalized_height {
            return false; // sous la finalité : jamais un candidat valide
        }
        let hash = block.hash();
        // Déjà le bloc retenu à cette hauteur ? alors ce n'est pas un *concurrent*.
        if self.block_row(h).map(|(bh, _)| *bh) == Some(hash) {
            return false;
        }
        let bucket = self.candidates.entry(h).or_default();
        if bucket.iter().any(|b| b.hash() == hash) {
            return false; // candidat déjà connu
        }
        bucket.push(block);
        true
    }

    /// Retire un candidat (par hash) à `height` — p.ex. lorsqu'il s'avère invalide au rejeu
    /// (state_root/supply incohérents) et ne doit donc plus peser au fork-choice. Retourne
    /// `true` si un candidat a été retiré. Purge l'entrée de hauteur devenue vide.
    pub fn remove_candidate(&mut self, height: u64, hash: Hash32) -> bool {
        if let Some(bucket) = self.candidates.get_mut(&height) {
            let before = bucket.len();
            bucket.retain(|b| b.hash() != hash);
            let removed = bucket.len() != before;
            if bucket.is_empty() {
                self.candidates.remove(&height);
            }
            return removed;
        }
        false
    }

    /// Candidats concurrents connus à `height` (hors bloc retenu dans `blocks`). Vide si aucun.
    pub fn candidates_at(&self, height: u64) -> &[Block] {
        self.candidates
            .get(&height)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// ADR 0031 — **choix canonique** à `height` : élit le hash de la tête parmi
    /// {bloc retenu} ∪ {candidats}, via `canonical_head` (poids de co-sigs → leader prévu →
    /// plus petit hash). Renvoie `None` si aucun bloc n'existe à cette hauteur.
    ///
    /// Fonction de **décision pure** (aucune mutation) : la tranche 2a l'utilise en observation
    /// (log si le choix diffère du bloc retenu) ; la tranche 2b s'en servira pour réorganiser.
    pub fn canonical_choice(
        &self,
        height: u64,
        validator_set: &vinx_core::ValidatorSet,
        indexed_bls_pks: &[Option<[u8; 48]>],
    ) -> Option<Hash32> {
        let stored = self.block_row(height).map(|(_, b)| b);
        let cands = self.candidates_at(height);
        stored
            .into_iter()
            .chain(cands.iter())
            .reduce(|a, b| crate::consensus::more_canonical(a, b, validator_set, indexed_bls_pks))
            .map(|b| b.hash())
    }

    /// `true` si un candidat concurrent l'emporterait sur le bloc actuellement retenu à
    /// `height` selon la règle de fork-choice — c.-à-d. si une réorganisation serait requise
    /// (tranche 2b). En tranche 2a, sert uniquement à journaliser une divergence observée.
    pub fn would_reorg_at(
        &self,
        height: u64,
        validator_set: &vinx_core::ValidatorSet,
        indexed_bls_pks: &[Option<[u8; 48]>],
    ) -> bool {
        match (
            self.block_row(height),
            self.canonical_choice(height, validator_set, indexed_bls_pks),
        ) {
            (Some((stored_hash, _)), Some(canonical)) => *stored_hash != canonical,
            _ => false,
        }
    }

    /// Purge les candidats devenus inutiles parce que finalisés (réorg interdite sous la
    /// finalité). Appelée après chaque avancée de `finalized_height`.
    fn prune_candidates_final(&mut self) {
        let f = self.finalized_height;
        self.candidates.retain(|&h, _| h > f);
    }

    /// Height of the latest block (0 = only genesis exists).
    pub fn tip_height(&self) -> u64 {
        self.height_base + (self.blocks.len() as u64).saturating_sub(1)
    }

    pub fn tip_hash(&self) -> Hash32 {
        self.blocks
            .last()
            .map(|(h, _)| *h)
            .unwrap_or(GENESIS_PREV_HASH)
    }

    /// Timestamp (unix seconds) of the tip block. Used for the monotonicity bound on
    /// incoming block timestamps (ADR 0005).
    pub fn tip_timestamp(&self) -> u64 {
        self.blocks
            .last()
            .map(|(_, b)| b.header.timestamp)
            .unwrap_or(0)
    }

    /// Median Time Past: median of the last `MEDIAN_TIME_BLOCKS` block timestamps
    /// (ADR 0005). A single producer cannot make this reference jump — it is a median,
    /// so it resists timestamp manipulation. Intended reference for time-sensitive
    /// comparisons (emission, unbonding) once wired onto it.
    pub fn median_time_past(&self) -> u64 {
        let n = self.blocks.len();
        let start = n.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS);
        let mut ts: Vec<u64> = self.blocks[start..]
            .iter()
            .map(|(_, b)| b.header.timestamp)
            .collect();
        if ts.is_empty() {
            return 0;
        }
        ts.sort_unstable();
        ts[ts.len() / 2]
    }

    /// Median Time Past of the chain as it will be once a block carrying `next_ts`
    /// is appended: the median over the last `MEDIAN_TIME_BLOCKS - 1` stored
    /// timestamps plus `next_ts`. This is the **protocol clock** for the block being
    /// applied (ADR 0005): emission, bond unbonding and upgrade activation compare
    /// against this median, so a single producer cannot jump protocol time with a
    /// bogus header timestamp. Every node computes it identically from the same
    /// chain prefix + header, keeping the state transition deterministic.
    pub fn median_time_past_with(&self, next_ts: u64) -> u64 {
        let n = self.blocks.len();
        let start = n.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS - 1);
        let mut ts: Vec<u64> = self.blocks[start..]
            .iter()
            .map(|(_, b)| b.header.timestamp)
            .collect();
        ts.push(next_ts);
        ts.sort_unstable();
        ts[ts.len() / 2]
    }

    /// MTP (ADR 0005) **indexé par hauteur** : médiane des timestamps des blocs
    /// `[height-(MEDIAN_TIME_BLOCKS-1) .. height)` plus `next_ts`. Contrairement à
    /// `median_time_past_with` (fenêtre au tip courant), cette variante calcule l'horloge
    /// protocole telle qu'elle était **pour le bloc de hauteur `height`** — nécessaire pour
    /// **rejouer** un bloc en milieu de chaîne lors d'une réorg (fork-choice, ADR 0031).
    /// Cohérente avec `median_time_past_with` quand `height == tip+1` (production au tip).
    pub fn median_time_past_ending_at(&self, height: u64, next_ts: u64) -> u64 {
        // VINX-08 : même défaut d'indexation que `reorg_replace`. Sur une chaîne
        // snapshot-syncée la fenêtre MTP était calculée sur les mauvais blocs, donc un
        // `state_root` divergent au rejeu de réorg.
        let end = height
            .checked_sub(self.height_base)
            .map_or(0, |i| (i as usize).min(self.blocks.len()));
        let start = end.saturating_sub(vinx_core::amount::MEDIAN_TIME_BLOCKS - 1);
        let mut ts: Vec<u64> = self.blocks[start..end]
            .iter()
            .map(|(_, b)| b.header.timestamp)
            .collect();
        ts.push(next_ts);
        ts.sort_unstable();
        ts[ts.len() / 2]
    }

    /// ADR 0031 — **réorganisation** : remplace le bloc retenu à `height` par `new_block` (élu
    /// canonique) et **tronque** tout bloc au-dessus (ils bâtissaient sur la branche perdante).
    /// Invariants garantis par l'appelant : `finalized_height < height <= tip` (réorg interdite
    /// sous la finalité) et `new_block.header.height == height`. Reconstruit les index de tx et
    /// purge les candidats de la hauteur (le choix est fait). Retourne les hachages des blocs
    /// retirés (bloc remplacé + blocs tronqués) pour nettoyage éventuel par l'appelant.
    pub fn reorg_replace(&mut self, height: u64, new_block: Block) -> Vec<Hash32> {
        debug_assert!(
            height > self.finalized_height,
            "réorg sous la finalité interdite (ADR 0031)"
        );
        debug_assert_eq!(
            new_block.header.height, height,
            "hauteur du bloc incohérente"
        );
        // VINX-08 : `blocks` est indexé depuis `height_base` (non nul sur une chaîne
        // amorcée par snapshot). Indexer par hauteur absolue faisait paniquer `drain`
        // ("start index out of range") — déni de service distant sur tout nœud
        // snapshot-syncé recevant un bloc concurrent. Comme `block_row`/`get_block`,
        // on convertit hauteur → index.
        let Some(idx) = height.checked_sub(self.height_base) else {
            return Vec::new();
        };
        let idx = idx as usize;
        if idx >= self.blocks.len() {
            return Vec::new();
        }
        // Retire le bloc contesté et tout ce qui le surplombe (branche perdante).
        let removed: Vec<Hash32> = self.blocks.drain(idx..).map(|(hash, _)| hash).collect();
        // Installe le bloc canonique à `height` (redevient le tip).
        self.blocks.push((new_block.hash(), new_block));
        // Les candidats à cette hauteur n'ont plus de raison d'être.
        self.candidates.remove(&height);
        // Index de tx : reconstruction complète (réorg rare → coût acceptable, cohérence sûre).
        self.rebuild_tx_index();
        // La persistance devra réécrire depuis `height` (et effacer les rangs tronqués) : le
        // caller déclenche une sauvegarde complète après une réorg.
        self.dirty_heights.insert(height);
        removed
    }

    pub fn get_block(&self, height: u64) -> Option<&Block> {
        let idx = height.checked_sub(self.height_base)? as usize;
        self.blocks.get(idx).map(|(_, b)| b)
    }

    /// Appends a block and returns its hash.
    pub fn push(&mut self, block: Block) -> Hash32 {
        let height = block.header.height;
        for (idx, tx) in block.transactions.iter().enumerate() {
            let tx_hash = tx.hash();
            self.tx_index.insert(tx_hash, (height, idx as u32));
            self.account_tx_index
                .entry(tx.from)
                .or_default()
                .push(tx_hash);
            if tx.to != tx.from {
                self.account_tx_index
                    .entry(tx.to)
                    .or_default()
                    .push(tx_hash);
            }
        }
        let hash = block.hash();
        self.blocks.push((hash, block));
        self.dirty_heights.insert(height);
        hash
    }

    /// Exports both tx indexes for external persistence (called by Storage::save).
    #[allow(clippy::type_complexity)]
    pub fn export_tx_indexes(
        &self,
    ) -> (
        &AHashMap<Hash32, (u64, u32)>,
        &AHashMap<Address, Vec<Hash32>>,
    ) {
        (&self.tx_index, &self.account_tx_index)
    }

    /// Replaces both tx indexes from a previously persisted snapshot.
    /// Faster than `rebuild_tx_index` — O(1) deserialization vs O(blocks × txs).
    pub fn import_tx_indexes(
        &mut self,
        tx_index: AHashMap<Hash32, (u64, u32)>,
        account_tx_index: AHashMap<Address, Vec<Hash32>>,
    ) {
        self.tx_index = tx_index;
        self.account_tx_index = account_tx_index;
    }

    /// Clears and repopulates both tx indexes from all stored blocks.
    pub fn rebuild_tx_index(&mut self) {
        self.tx_index.clear();
        self.account_tx_index.clear();
        for (_, block) in &self.blocks {
            let height = block.header.height;
            for (idx, tx) in block.transactions.iter().enumerate() {
                let tx_hash = tx.hash();
                self.tx_index.insert(tx_hash, (height, idx as u32));
                self.account_tx_index
                    .entry(tx.from)
                    .or_default()
                    .push(tx_hash);
                if tx.to != tx.from {
                    self.account_tx_index
                        .entry(tx.to)
                        .or_default()
                        .push(tx_hash);
                }
            }
        }
    }

    /// Total number of transactions involving this address.
    pub fn account_tx_count(&self, addr: &Address) -> usize {
        self.account_tx_index.get(addr).map_or(0, |v| v.len())
    }

    /// Returns tx hashes for the given address, newest-first, with pagination.
    pub fn get_account_txs(&self, addr: &Address, limit: usize, offset: usize) -> Vec<Hash32> {
        match self.account_tx_index.get(addr) {
            None => vec![],
            Some(hashes) => {
                let len = hashes.len();
                if offset >= len {
                    return vec![];
                }
                hashes
                    .iter()
                    .rev()
                    .skip(offset)
                    .take(limit)
                    .copied()
                    .collect()
            }
        }
    }

    /// Looks up a transaction by its raw 32-byte hash.
    pub fn get_tx_by_hash(&self, hash: &Hash32) -> Option<(u64, &Block, &Transaction)> {
        let &(height, tx_pos) = self.tx_index.get(hash)?;
        let idx = height.checked_sub(self.height_base)? as usize;
        let (_, block) = self.blocks.get(idx)?;
        let tx = block.transactions.get(tx_pos as usize)?;
        Some((height, block, tx))
    }

    /// Number of blocks (= tip_height + 1).
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// True when the chain holds no blocks (never the case after genesis).
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Updates the BLS aggregate co-signature on a stored block (ADR 0046).
    /// Called by the P2P handler once ≥ quorum BLS co-signatures have been aggregated.
    /// Marks the block dirty so the persistence layer rewrites it.
    pub fn set_block_bls(
        &mut self,
        height: u64,
        bls_aggregate: Vec<u8>,
        bls_cosigner_pks: Vec<Vec<u8>>,
        bls_bitmap: Vec<u8>,
    ) {
        let Some(idx) = height.checked_sub(self.height_base) else {
            return;
        };
        if let Some((_, block)) = self.blocks.get_mut(idx as usize) {
            block.bls_aggregate = Some(bls_aggregate);
            block.bls_cosigner_pks = bls_cosigner_pks;
            block.bls_bitmap = bls_bitmap;
            self.dirty_heights.insert(height);
        }
    }

    /// Records that `validator` signed `block_hash` at `height`.
    /// Returns `true` if equivocation is detected (validator signed a DIFFERENT
    /// block at the same height — a slashable offense).
    pub fn record_signature(
        &mut self,
        validator: &Address,
        height: u64,
        block_hash: Hash32,
    ) -> bool {
        let heights = self.slash_evidence.entry(*validator).or_default();
        let hashes = heights.entry(height).or_default();
        if !hashes.is_empty() && !hashes.contains(&block_hash) {
            return true; // double-sign detected
        }
        hashes.insert(block_hash);
        false
    }

    /// Time-based pruning: drops tx and signature data from blocks whose timestamp is
    /// older than `retain_secs` seconds before `now_ts`. Block headers are kept forever.
    ///
    /// This is the primary retention policy (ADR 0045 / `TX_RETENTION_SECS = 90 days`).
    /// Use `prune` for block-count-based compaction in tests.
    pub fn prune_by_age(&mut self, now_ts: u64, retain_secs: u64) {
        let cutoff = now_ts.saturating_sub(retain_secs);
        let mut pruned_blocks = 0usize;
        let mut tx_pruned = 0usize;

        for (i, (_, block)) in self.blocks.iter_mut().enumerate() {
            if block.header.timestamp >= cutoff {
                break; // blocks are monotonically ordered by time
            }
            if !block.transactions.is_empty() {
                tx_pruned += block.transactions.len();
                block.transactions.clear();
                self.dirty_heights.insert(self.height_base + i as u64);
                pruned_blocks += 1;
            }
        }

        if pruned_blocks > 0 {
            self.rebuild_tx_index();
            tracing::info!(
                pruned_blocks,
                tx_pruned,
                cutoff_ts = cutoff,
                "Chain: time-based tx pruning complete"
            );
        }
    }

    /// Compacts transaction data from blocks older than `keep_last` blocks.
    /// Block headers and hashes are retained to preserve chain integrity.
    /// This reduces memory/disk usage without breaking hash linkage verification.
    pub fn compact_old_txs(&mut self, keep_last: u64) {
        let tip = self.tip_height();
        if tip < keep_last {
            return;
        }
        let compact_up_to_idx = (tip - keep_last).saturating_sub(self.height_base) as usize;
        for i in 0..compact_up_to_idx {
            if let Some((_, block)) = self.blocks.get_mut(i) {
                // Only touch (and re-persist) blocks that still had data — repeated
                // compaction passes must not mark the whole history dirty again.
                if !block.transactions.is_empty() {
                    block.transactions.clear();
                    self.dirty_heights.insert(self.height_base + i as u64);
                }
            }
        }
        // Rebuild index to remove entries from pruned blocks
        self.rebuild_tx_index();
        tracing::info!(
            compacted = compact_up_to_idx,
            "Chain compacted old transaction data"
        );
    }

    /// Full pruning pass — runs every PRUNE_INTERVAL blocks.
    ///
    /// Three things are cleaned up:
    /// 1. Transaction data older than `keep_last` blocks (largest space consumer).
    /// 2. Signatures on finalized blocks older than `keep_last` (verified, no longer needed).
    /// 3. Slash evidence older than `keep_last * 2` (equivocation window is well past).
    ///
    /// Block headers (height, prev_hash, state_root, validator…) are NEVER dropped —
    /// they are needed for hash-chain integrity and light-client sync proofs.
    pub fn prune(&mut self, keep_last: u64) {
        let tip = self.tip_height();
        if tip < keep_last {
            return;
        }
        let prune_up_to_idx = (tip - keep_last).saturating_sub(self.height_base) as usize;

        let mut tx_pruned = 0usize;

        for i in 0..prune_up_to_idx {
            if let Some((_, block)) = self.blocks.get_mut(i) {
                if !block.transactions.is_empty() {
                    tx_pruned += block.transactions.len();
                    block.transactions.clear();
                    self.dirty_heights.insert(self.height_base + i as u64);
                }
            }
        }

        // Remove slash evidence older than 2× the retention window
        let evidence_cutoff = tip.saturating_sub(keep_last * 2);
        self.slash_evidence.retain(|_, heights| {
            heights.retain(|&h, _| h > evidence_cutoff);
            !heights.is_empty()
        });

        // Rebuild tx index to remove stale entries
        self.rebuild_tx_index();

        tracing::info!(
            tip,
            pruned_below = prune_up_to_idx,
            tx_pruned,
            "Chain pruned — headers retained, old tx data dropped"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn validator() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    #[test]
    fn test_genesis_height_is_zero() {
        let (chain, _) = Chain::new_with_genesis(validator(), 0);
        assert_eq!(chain.tip_height(), 0);
    }

    #[test]
    fn test_genesis_prev_hash_is_zero() {
        let (chain, _) = Chain::new_with_genesis(validator(), 0);
        let genesis = chain.get_block(0).unwrap();
        assert_eq!(genesis.header.prev_hash, GENESIS_PREV_HASH);
    }

    #[test]
    fn test_tip_hash_matches_genesis_block() {
        let (chain, genesis) = Chain::new_with_genesis(validator(), 0);
        assert_eq!(chain.tip_hash(), genesis.hash());
    }

    #[test]
    fn test_get_block_out_of_range() {
        let (chain, _) = Chain::new_with_genesis(validator(), 0);
        assert!(chain.get_block(999).is_none());
    }

    #[test]
    fn test_equivocation_detection() {
        let mut chain = Chain::from_parts(vec![], 0);
        let addr = Address::from_public_key(&KeyPair::generate().public_key());
        let hash_a = [1u8; 32];
        let hash_b = [2u8; 32];

        assert!(!chain.record_signature(&addr, 5, hash_a)); // first sig — ok
        assert!(!chain.record_signature(&addr, 5, hash_a)); // same hash — ok (idempotent)
        assert!(chain.record_signature(&addr, 5, hash_b)); // different hash — EQUIVOCATION
    }

    /// Deterministic BLS key for validator index `idx`, so tests can build a matching
    /// on-chain registry (`test_registry`) for the registry-bound finality path.
    fn test_bls_sk(idx: usize) -> vinx_crypto::BlsSecretKey {
        let mut seed = [0u8; 32];
        seed[0] = idx as u8 + 1;
        vinx_crypto::BlsSecretKey::from_bytes(&seed).expect("valid BLS scalar")
    }

    /// The registered BLS keys of the first `n` validator slots.
    fn test_registry(n: usize) -> Vec<Option<[u8; 48]>> {
        (0..n)
            .map(|i| Some(test_bls_sk(i).public_key().0))
            .collect()
    }

    /// Builds a block at `height` with `n_sigs` BLS co-signatures.
    fn signed_block(height: u64, prev: Hash32, proposer: Address, n_sigs: usize) -> Block {
        let header = BlockHeader {
            height,
            prev_hash: prev,
            timestamp: height,
            validator: proposer,
            tx_count: 0,
            state_root: [0u8; 32],
            base_fee: 0,
            receipts_root: [0u8; 32],
        };
        let mut block = Block {
            header,
            transactions: vec![],
            bls_aggregate: None,
            bls_cosigner_pks: vec![],
            bls_bitmap: vec![],
            vrf_proof: None,
        };
        for idx in 0..n_sigs {
            crate::consensus::sign_block_bls(&mut block, &test_bls_sk(idx), idx).unwrap();
        }
        block
    }

    #[test]
    fn test_finality_advances_with_quorum() {
        // Single validator: its own signature already meets quorum → block 1 final.
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let vs = vinx_core::ValidatorSet::single(v);
        let (mut chain, _) = Chain::new_with_genesis(v, 0);
        assert_eq!(chain.finalized_height(), 0);

        let b1 = signed_block(1, chain.tip_hash(), v, 1);
        chain.push(b1);
        assert_eq!(chain.finalized_height(), 0); // not advanced until we ask
        chain.advance_finality(&vs, &test_registry(vs.len()));
        assert_eq!(chain.finalized_height(), 1);
        assert!(chain.is_final(1));
        assert!(!chain.is_final(2));
    }

    // ── ADR 0031 — fondation du fork-choice (tranche 2a : stockage + choix, sans réorg) ──

    fn three_validators() -> (Vec<KeyPair>, Vec<Address>, vinx_core::ValidatorSet) {
        let kps: Vec<KeyPair> = (0..3).map(|_| KeyPair::generate()).collect();
        let addrs: Vec<Address> = kps
            .iter()
            .map(|k| Address::from_public_key(&k.public_key()))
            .collect();
        let vs = vinx_core::ValidatorSet::new(addrs.clone());
        (kps, addrs, vs)
    }

    #[test]
    fn test_record_candidate_dedup_and_below_finality() {
        let (_, addrs, vs) = three_validators();
        let (mut chain, _) = Chain::new_with_genesis(addrs[0], 0);
        let g = chain.tip_hash();

        // Bloc retenu à h=1 (proposeur = leader prévu).
        let leader = *vs.leader_at(1);
        let a = signed_block(1, g, leader, 1);
        chain.push(a.clone());

        // Un concurrent (proposeur différent, même prev) à h=1.
        let other = addrs.iter().copied().find(|x| *x != leader).unwrap();
        let b = signed_block(1, g, other, 2);
        assert!(
            chain.record_candidate(b.clone()),
            "nouveau candidat enregistré"
        );
        assert!(!chain.record_candidate(b), "dédup par hash");
        assert!(
            !chain.record_candidate(a),
            "le bloc déjà retenu n'est pas un concurrent"
        );
        // Un bloc à/sous la finalité (genèse h=0 finalisée) : refusé.
        let below = signed_block(0, GENESIS_PREV_HASH, addrs[0], 1);
        assert!(!chain.record_candidate(below), "sous la finalité : refusé");
        assert_eq!(chain.candidates_at(1).len(), 1);
    }

    #[test]
    fn test_canonical_choice_prefers_more_cosignatures() {
        let (_, addrs, vs) = three_validators();
        let (mut chain, _) = Chain::new_with_genesis(addrs[0], 0);
        let g = chain.tip_hash();

        // Bloc retenu : leader prévu mais 1 seule co-signature.
        let leader = *vs.leader_at(1);
        let weak = signed_block(1, g, leader, 1);
        chain.push(weak);

        // Candidat : backup mais 2 co-signatures (plus soutenu par le set).
        let backup = addrs.iter().copied().find(|x| *x != leader).unwrap();
        let strong = signed_block(1, g, backup, 2);
        let strong_hash = strong.hash();
        chain.record_candidate(strong);

        assert_eq!(
            chain.canonical_choice(1, &vs, &test_registry(vs.len())),
            Some(strong_hash),
            "le poids de co-signatures supérieur gagne (règle 3)"
        );
        assert!(
            chain.would_reorg_at(1, &vs, &test_registry(vs.len())),
            "réorg requise vers le candidat plus soutenu"
        );
    }

    #[test]
    fn test_canonical_choice_breaks_tie_by_scheduled_leader() {
        let (_, addrs, vs) = three_validators();
        let (mut chain, _) = Chain::new_with_genesis(addrs[0], 0);
        let g = chain.tip_hash();

        let leader = *vs.leader_at(1);
        let backup = addrs.iter().copied().find(|x| *x != leader).unwrap();

        // À poids de co-sigs ÉGAL (1 chacun), le bloc du leader prévu doit gagner (règle 4).
        let backup_block = signed_block(1, g, backup, 1);
        chain.push(backup_block);
        let leader_block = signed_block(1, g, leader, 1);
        let leader_hash = leader_block.hash();
        chain.record_candidate(leader_block);

        assert_eq!(
            chain.canonical_choice(1, &vs, &test_registry(vs.len())),
            Some(leader_hash),
            "à poids égal, le leader prévu l'emporte sur le backup (règle 4)"
        );
        assert!(chain.would_reorg_at(1, &vs, &test_registry(vs.len())));
    }

    #[test]
    fn test_reorg_replace_truncates_above_and_swaps() {
        let (_, addrs, vs) = three_validators();
        let (mut chain, _) = Chain::new_with_genesis(addrs[0], 0);
        let g = chain.tip_hash();

        // Chaîne linéaire h=1,2,3 (branche perdante).
        let b1 = signed_block(1, g, *vs.leader_at(1), 1);
        let h1 = b1.hash();
        chain.push(b1);
        let b2 = signed_block(2, h1, *vs.leader_at(2), 1);
        let h2 = b2.hash();
        chain.push(b2);
        let b3 = signed_block(3, h2, *vs.leader_at(3), 1);
        chain.push(b3);
        assert_eq!(chain.tip_height(), 3);

        // Réorg à h=2 : remplace le bloc 2 par un concurrent → tronque le bloc 3.
        let other = addrs
            .iter()
            .copied()
            .find(|x| *x != *vs.leader_at(2))
            .unwrap();
        let b2_alt = signed_block(2, h1, other, 2);
        let alt_hash = b2_alt.hash();
        let removed = chain.reorg_replace(2, b2_alt);

        assert_eq!(
            chain.tip_height(),
            2,
            "le bloc 3 (branche perdante) est tronqué"
        );
        assert_eq!(
            chain.block_row(2).map(|(h, _)| *h),
            Some(alt_hash),
            "le bloc 2 est remplacé par le concurrent"
        );
        assert_eq!(removed.len(), 2, "bloc 2 remplacé + bloc 3 tronqué");
        assert!(removed.contains(&h2));
    }

    #[test]
    fn test_candidates_pruned_below_finality() {
        let (_, addrs, _) = three_validators();
        // Sous-ensemble à 2 validateurs pour un quorum de 2 atteignable ici.
        let vs = vinx_core::ValidatorSet::new(vec![addrs[0], addrs[1]]);
        let (mut chain, _) = Chain::new_with_genesis(addrs[0], 0);
        let g = chain.tip_hash();

        // Bloc retenu finalisable à h=1 (2 co-sigs = quorum).
        let leader = *vs.leader_at(1);
        let a = signed_block(1, g, leader, 2);
        chain.push(a);
        // Un concurrent à h=1.
        let other = if leader == addrs[0] {
            addrs[1]
        } else {
            addrs[0]
        };
        let b = signed_block(1, g, other, 1);
        assert!(chain.record_candidate(b));
        assert_eq!(chain.candidates_at(1).len(), 1);

        // Finaliser h=1 → les candidats à h=1 sont purgés (réorg interdite sous finalité).
        chain.advance_finality(&vs, &test_registry(vs.len()));
        assert_eq!(chain.finalized_height(), 1);
        assert!(
            chain.candidates_at(1).is_empty(),
            "candidats purgés une fois la hauteur finalisée"
        );
    }

    #[test]
    fn test_median_time_past_is_the_median() {
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let (mut chain, _) = Chain::new_with_genesis(v, 0); // genesis ts = 0
        for h in 1..=5 {
            let b = signed_block(h, chain.tip_hash(), v, 1); // ts = h
            chain.push(b);
        }
        // timestamps {0,1,2,3,4,5} → median (index 3) = 3
        assert_eq!(chain.median_time_past(), 3);
    }

    #[test]
    fn test_median_time_past_with_resists_timestamp_jump() {
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let (mut chain, _) = Chain::new_with_genesis(v, 0); // genesis ts = 0
        for h in 1..=5 {
            let b = signed_block(h, chain.tip_hash(), v, 1); // ts = h
            chain.push(b);
        }
        // Honest next block (ts = 6): stored {0..=5} + 6 → median (index 3) = 3.
        assert_eq!(chain.median_time_past_with(6), 3);
        // A producer claiming ts = 1_000_000 moves the median not one second more:
        // the protocol clock ignores the outlier (ADR 0005).
        assert_eq!(chain.median_time_past_with(1_000_000), 3);
    }

    #[test]
    fn test_finality_uses_historical_quorum() {
        // ADR 0002/0027 : un bloc de l'ère « 1 validateur » (quorum 1, 1 signature) doit
        // rester final même après que le set a grandi à 3 (quorum 2) — sinon il bloquerait
        // le préfixe. C'est le bug révélé par le banc n=3.
        let kp1 = KeyPair::generate();
        let v1 = Address::from_public_key(&kp1.public_key());
        let kp2 = KeyPair::generate();
        let v2 = Address::from_public_key(&kp2.public_key());
        let kp3 = KeyPair::generate();
        let v3 = Address::from_public_key(&kp3.public_key());
        let vs_now = vinx_core::ValidatorSet::new(vec![v1, v2, v3]); // set courant : quorum 2
        let (mut chain, _) = Chain::new_with_genesis(v1, 0);

        // Bloc 1 : ère 1-validateur → quorum historique 1, une seule signature.
        chain.note_quorum(1, 1);
        let b1 = signed_block(1, chain.tip_hash(), v1, 1);
        chain.push(b1);
        // Bloc 2 : set passé à 3 → quorum 2, deux signatures.
        chain.note_quorum(2, 2);
        let b2 = signed_block(2, chain.tip_hash(), v2, 2);
        chain.push(b2);

        // Avancer avec le set COURANT (quorum 2). Sans quorum historique, le bloc 1 (1 sig)
        // bloquerait le préfixe à 0 ; avec, il finalise sous quorum-1, puis le bloc 2.
        chain.advance_finality(&vs_now, &test_registry(vs_now.len()));
        assert_eq!(
            chain.finalized_height(),
            2,
            "le quorum historique finalise le préfixe malgré le changement de set"
        );

        // Contrôle : sans checkpoint historique, quorum_at retombe sur le fallback (quorum
        // courant) → le bloc 1 (1 sig) ne finaliserait pas.
        let (mut chain2, _) = Chain::new_with_genesis(v1, 0);
        let b1b = signed_block(1, chain2.tip_hash(), v1, 1);
        chain2.push(b1b);
        chain2.advance_finality(&vs_now, &test_registry(vs_now.len()));
        assert_eq!(
            chain2.finalized_height(),
            0,
            "sans quorum historique, un bloc à 1 sig échoue au quorum courant (2)"
        );
    }

    #[test]
    fn test_finality_stops_below_quorum() {
        // Two validators (quorum 2): a block with only the proposer's sig is NOT final.
        let kp = KeyPair::generate();
        let v = Address::from_public_key(&kp.public_key());
        let other = Address::from_public_key(&KeyPair::generate().public_key());
        let vs = vinx_core::ValidatorSet::new(vec![v, other]);
        let (mut chain, _) = Chain::new_with_genesis(v, 0);

        let b1 = signed_block(1, chain.tip_hash(), v, 1); // 1 of 2 sigs
        chain.push(b1);
        chain.advance_finality(&vs, &test_registry(vs.len()));
        assert_eq!(chain.finalized_height(), 0); // below quorum → not final
    }

    #[test]
    fn test_prune_drops_tx_and_sig_data_but_keeps_headers() {
        let v = validator();
        let (mut chain, _) = Chain::new_with_genesis(v, 0);
        for h in 1u64..=10 {
            let block = Block {
                header: BlockHeader {
                    height: h,
                    prev_hash: chain.tip_hash(),
                    timestamp: h,
                    validator: v,
                    tx_count: 0,
                    state_root: [0u8; 32],
                    base_fee: 0,
                    receipts_root: [0u8; 32],
                },
                transactions: vec![],
                bls_aggregate: None,
                bls_cosigner_pks: vec![],
                bls_bitmap: vec![],
                vrf_proof: None,
            };
            chain.push(block);
        }
        // Prune keeping last 3 blocks (height 8, 9, 10); blocks 0-7 are compacted
        chain.prune(3);
        // All headers still accessible
        for h in 0u64..=10 {
            assert!(
                chain.get_block(h).is_some(),
                "block {h} missing after prune"
            );
        }
        assert_eq!(chain.tip_height(), 10);
    }

    #[test]
    fn test_prune_noop_when_chain_shorter_than_keep_last() {
        let v = validator();
        let (mut chain, _) = Chain::new_with_genesis(v, 0);
        // Only genesis — prune with keep_last=100 should be a no-op
        chain.prune(100);
        assert_eq!(chain.tip_height(), 0);
        assert!(chain.get_block(0).is_some());
    }

    #[test]
    fn test_compact_old_txs_preserves_headers() {
        let v = validator();
        let (mut chain, _) = Chain::new_with_genesis(v, 0);
        // Push 5 empty blocks
        for h in 1u64..=5 {
            let block = Block {
                header: BlockHeader {
                    height: h,
                    prev_hash: chain.tip_hash(),
                    timestamp: h,
                    validator: v,
                    tx_count: 0,
                    state_root: [0u8; 32],
                    base_fee: 0,
                    receipts_root: [0u8; 32],
                },
                transactions: vec![],
                bls_aggregate: None,
                bls_cosigner_pks: vec![],
                bls_bitmap: vec![],
                vrf_proof: None,
            };
            chain.push(block);
        }
        // Compact keeping only the last 2 blocks
        chain.compact_old_txs(2);
        // Headers still accessible
        assert!(chain.get_block(0).is_some());
        assert!(chain.get_block(4).is_some());
        assert_eq!(chain.tip_height(), 5);
    }
}
