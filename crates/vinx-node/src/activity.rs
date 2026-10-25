//! Activité récente du réseau, pour le tableau de bord : qui a signé chaque
//! bloc, qui devait le proposer, à quel rythme les blocs arrivent.
//!
//! Un validateur participe à un bloc s'il figure dans son certificat, ou si ce
//! nœud a reçu de lui un vote valide pour cette hauteur. Le certificat seul ne
//! suffit pas : il ne contient que le quorum, les signatures arrivées en
//! premier, et un validateur un peu plus lent que les autres en serait souvent
//! absent alors qu'il vote. Tout cela dit si un validateur vote, pas pourquoi
//! il ne vote pas (nœud arrêté, réseau coupé, nœud en retard).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use vinx_consensus::proposer_index;
use vinx_core::{Block, CommitCertificate};
use vinx_crypto::PublicKey;

/// Fenêtre suivie : une heure.
pub const WINDOW_MS: u64 = 60 * 60 * 1000;
/// Un validateur actif depuis le début de ces derniers blocs est « en ligne ».
const ONLINE_BLOCKS: usize = 3;
/// Hauteurs à venir pour lesquelles des votes sont gardés en attente du bloc.
const MAX_PENDING_HEIGHTS: u64 = 10;
/// Au-delà de ce délai sans signature, il est « absent » (sinon « en retard »).
const LATE_MS: u64 = 60_000;

/// Résumé d'un bloc finalisé.
#[derive(Clone, Debug)]
pub struct BlockRecord {
    pub height: u64,
    pub timestamp: u64,
    pub round: u32,
    pub txs: usize,
    pub peers: usize,
    /// Ensemble de validateurs de cette hauteur.
    pub set: Vec<PublicKey>,
    /// Indices (dans `set`) des signataires du certificat.
    pub signers: Vec<u16>,
    /// Validateurs dont un vote valide a été reçu pour cette hauteur.
    pub voters: BTreeSet<PublicKey>,
    /// Bloc suivi en direct, avec les votes reçus. Faux pour un bloc relu sur
    /// le disque au démarrage : son certificat ne contient que le quorum, il ne
    /// compte donc pas dans les taux de participation.
    pub live: bool,
}

impl BlockRecord {
    pub fn new(block: &Block, cert: &CommitCertificate, set: Vec<PublicKey>, peers: usize) -> Self {
        Self {
            height: block.header.height,
            timestamp: block.header.timestamp,
            round: cert.round,
            txs: block.txs.len().saturating_add(block.admin_txs.len()),
            peers,
            set,
            signers: cert.signatures.iter().map(|(i, _)| *i).collect(),
            voters: BTreeSet::new(),
            live: true,
        }
    }

    /// Le validateur a participé à ce bloc : certificat ou vote reçu.
    fn signed_by(&self, key: &PublicKey) -> bool {
        self.voters.contains(key)
            || self
                .signers
                .iter()
                .any(|i| self.set.get(usize::from(*i)) == Some(key))
    }

    /// Nombre de validateurs qui ont participé.
    pub fn participants(&self) -> usize {
        self.set.iter().filter(|k| self.signed_by(k)).count()
    }

    /// Validateurs qui devaient proposer un round antérieur au round finalisé,
    /// et ne l'ont donc pas fait.
    fn missed_proposers(&self) -> impl Iterator<Item = &PublicKey> {
        (0..self.round).filter_map(|r| {
            let i = proposer_index(self.height, r, self.set.len())?;
            self.set.get(usize::from(i))
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct Activity {
    pub blocks: VecDeque<BlockRecord>,
    /// Dernier vote reçu de chaque validateur : (hauteur, heure de réception).
    last_vote: BTreeMap<PublicKey, (u64, u64)>,
    /// Votes de hauteurs pas encore finalisées ici.
    pending: BTreeMap<u64, BTreeSet<PublicKey>>,
}

/// État d'un validateur vu des derniers blocs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    /// A signé l'un des derniers blocs.
    Online,
    /// A signé récemment, mais pas les tout derniers blocs.
    Late,
    /// N'a rien signé depuis plus d'une minute, ou jamais sur la fenêtre.
    Absent,
    /// Aucun bloc de la fenêtre ne l'attendait encore (validateur tout juste ajouté).
    New,
}

impl Presence {
    pub fn as_str(&self) -> &'static str {
        match self {
            Presence::Online => "en ligne",
            Presence::Late => "en retard",
            Presence::Absent => "absent",
            Presence::New => "nouveau",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ValidatorReport {
    pub key: PublicKey,
    pub presence: Presence,
    /// Dernière participation : bloc signé ou vote reçu.
    pub last_signed_height: Option<u64>,
    pub last_signed_ms: Option<u64>,
    /// Blocs signés et blocs qui attendaient sa signature, sur la fenêtre.
    pub signed: usize,
    pub expected: usize,
    pub missed_proposals: usize,
}

impl Activity {
    /// Note un vote valide de `key` pour `height`, reçu à `now_ms`. À
    /// n'appeler qu'après avoir vérifié sa signature.
    pub fn note_vote(&mut self, height: u64, key: &PublicKey, now_ms: u64) {
        let newer = self.last_vote.get(key).is_none_or(|(h, _)| height >= *h);
        if newer {
            self.last_vote.insert(key.clone(), (height, now_ms));
        }
        let last = self.blocks.back().map_or(0, |b| b.height);
        if let Some(b) = self.blocks.iter_mut().rev().find(|b| b.height == height) {
            b.voters.insert(key.clone());
        } else if height > last && height <= last.saturating_add(MAX_PENDING_HEIGHTS) {
            self.pending.entry(height).or_default().insert(key.clone());
        }
    }

    /// Ce vote est-il déjà noté ? Évite de revérifier les votes retransmis.
    pub fn knows_vote(&self, height: u64, key: &PublicKey) -> bool {
        self.blocks
            .iter()
            .rev()
            .find(|b| b.height == height)
            .map(|b| b.voters.contains(key))
            .or_else(|| self.pending.get(&height).map(|v| v.contains(key)))
            .unwrap_or(false)
    }

    /// Ajoute un bloc finalisé et oublie ceux qui sortent de la fenêtre.
    pub fn record(&mut self, mut record: BlockRecord) {
        let now = record.timestamp;
        if let Some(voters) = self.pending.remove(&record.height) {
            record.voters.extend(voters);
        }
        let height = record.height;
        self.pending.retain(|h, _| *h > height);
        self.blocks.push_back(record);
        while self
            .blocks
            .front()
            .is_some_and(|b| now.saturating_sub(b.timestamp) > WINDOW_MS)
        {
            self.blocks.pop_front();
        }
    }

    /// Part des blocs de la fenêtre finalisés au premier round.
    pub fn first_round_ratio(&self) -> Option<f64> {
        let total = self.blocks.len();
        let first = self.blocks.iter().filter(|b| b.round == 0).count();
        (total > 0).then(|| first as f64 / total as f64)
    }

    /// Bilan de chaque validateur de `set`, à l'instant `now_ms`.
    pub fn validators(&self, set: &[PublicKey], now_ms: u64) -> Vec<ValidatorReport> {
        let recent: Vec<&BlockRecord> = self.blocks.iter().rev().take(ONLINE_BLOCKS).collect();
        // Début de la fenêtre « en ligne » : le plus ancien des derniers blocs.
        let online_since = recent.last().map_or(0, |b| b.timestamp);
        let mut missed: BTreeMap<&PublicKey, usize> = BTreeMap::new();
        for b in &self.blocks {
            for k in b.missed_proposers() {
                let n = missed.entry(k).or_default();
                *n = n.saturating_add(1);
            }
        }
        set.iter()
            .map(|key| {
                let expected = self
                    .blocks
                    .iter()
                    .filter(|b| b.live && b.set.contains(key))
                    .count();
                let signed_blocks: Vec<&BlockRecord> =
                    self.blocks.iter().filter(|b| b.signed_by(key)).collect();
                let signed = signed_blocks.iter().filter(|b| b.live).count();
                // Dernière participation : le plus récent du dernier bloc signé
                // et du dernier vote reçu.
                let last = signed_blocks
                    .last()
                    .map(|b| (b.height, b.timestamp))
                    .into_iter()
                    .chain(self.last_vote.get(key).copied())
                    .max_by_key(|(_, t)| *t);
                let presence = if expected == 0 && last.is_none() {
                    Presence::New
                } else if recent.iter().any(|b| b.signed_by(key))
                    || last.is_some_and(|(_, t)| t >= online_since)
                {
                    Presence::Online
                } else if last.is_some_and(|(_, t)| now_ms.saturating_sub(t) <= LATE_MS) {
                    Presence::Late
                } else {
                    Presence::Absent
                };
                ValidatorReport {
                    key: key.clone(),
                    presence,
                    last_signed_height: last.map(|(h, _)| h),
                    last_signed_ms: last.map(|(_, t)| t),
                    signed,
                    expected,
                    missed_proposals: missed.get(key).copied().unwrap_or(0),
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)] // code de test
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn record(height: u64, round: u32, set: &[PublicKey], signers: &[u16]) -> BlockRecord {
        BlockRecord {
            height,
            timestamp: height * 5_000,
            round,
            txs: 0,
            peers: 3,
            set: set.to_vec(),
            signers: signers.to_vec(),
            voters: BTreeSet::new(),
            live: true,
        }
    }

    #[test]
    fn presence_follows_signatures() {
        let set: Vec<PublicKey> = (0..4).map(|_| KeyPair::generate().public_key()).collect();
        let mut a = Activity::default();
        // Le validateur 3 signe jusqu'à la hauteur 10, puis s'arrête.
        for h in 1..=10 {
            a.record(record(h, 0, &set, &[0, 1, 2, 3]));
        }
        for h in 11..=40 {
            a.record(record(h, 0, &set, &[0, 1, 2]));
        }
        let now = 40 * 5_000;
        let r = a.validators(&set, now);
        assert_eq!(r[0].presence, Presence::Online);
        assert_eq!(r[3].presence, Presence::Absent);
        assert_eq!(r[3].last_signed_height, Some(10));
        assert_eq!((r[3].signed, r[3].expected), (10, 40));
        // Peu après son arrêt (4 blocs, 20 s), il n'est qu'en retard.
        let mut early = Activity::default();
        for b in a.blocks.iter().take(14) {
            early.record(b.clone());
        }
        assert_eq!(
            early.validators(&set, 14 * 5_000)[3].presence,
            Presence::Late
        );
    }

    #[test]
    fn a_round_one_block_counts_a_missed_proposal() {
        let set: Vec<PublicKey> = (0..4).map(|_| KeyPair::generate().public_key()).collect();
        let mut a = Activity::default();
        // Hauteur 7, round 1 : le proposeur du round 0 (7 % 4 = 3) a manqué son tour.
        a.record(record(7, 1, &set, &[0, 1, 2]));
        let r = a.validators(&set, 7 * 5_000);
        assert_eq!(r[3].missed_proposals, 1);
        assert_eq!(r[0].missed_proposals, 0);
        assert_eq!(a.first_round_ratio(), Some(0.0));
    }

    #[test]
    fn a_validator_added_later_is_new() {
        let set: Vec<PublicKey> = (0..2).map(|_| KeyPair::generate().public_key()).collect();
        let mut a = Activity::default();
        a.record(record(1, 0, &set[..1], &[0]));
        let r = a.validators(&set, 5_000);
        assert_eq!(r[1].presence, Presence::New);
    }

    #[test]
    fn the_window_forgets_old_blocks() {
        let set = vec![KeyPair::generate().public_key()];
        let mut a = Activity::default();
        a.record(record(1, 0, &set, &[0]));
        let mut late = record(2, 0, &set, &[0]);
        late.timestamp = 5_000 + WINDOW_MS + 1;
        a.record(late);
        assert_eq!(a.blocks.len(), 1);
    }

    #[test]
    fn a_vote_outside_the_quorum_still_counts() {
        let set: Vec<PublicKey> = (0..4).map(|_| KeyPair::generate().public_key()).collect();
        let mut a = Activity::default();
        // Le validateur 3 vote toujours, mais arrive après le quorum : jamais
        // dans les certificats. Ses votes arrivent avant ou après le bloc.
        for h in 1..=20 {
            if h % 2 == 0 {
                a.note_vote(h, &set[3], h * 5_000);
            }
            a.record(record(h, 0, &set, &[0, 1, 2]));
            if h % 2 == 1 {
                a.note_vote(h, &set[3], h * 5_000 + 100);
            }
        }
        let r = a.validators(&set, 20 * 5_000 + 200);
        assert_eq!(r[3].presence, Presence::Online);
        assert_eq!((r[3].signed, r[3].expected), (20, 20));
        assert!(a.knows_vote(20, &set[3]));
        assert!(!a.knows_vote(20, &set[0]));
    }
}
