//! Validateurs nommés, caution et délai des actions admin (protocole 7,
//! docs/PROTOCOLE-7.md).
//!
//! - **Fiches** : chaque validateur, actif ou passé, a une identité publique
//!   (nom, opérateur, pays) fixée par la gouvernance, plus un site et un
//!   contact qu'il tient lui-même à jour.
//! - **Noms** : uniques sans tenir compte des majuscules, et jamais
//!   réattribués, même après le départ de leur validateur.
//! - **Cautions** : déposées depuis l'adresse de la clé du validateur, comptées
//!   dans l'invariant de supply (des unités bloquées, jamais créées).
//! - **Actions admin en attente** : à partir du protocole 7, la plupart des
//!   actions admin ne prennent effet qu'après `admin_delay` secondes, et
//!   peuvent être annulées entre-temps.

use crate::table::{Row, Table};
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{AdminAction, Amount, UNITS_PER_VINX, ValidatorIdentity};
use vinx_crypto::{Address, Hash, PublicKey};

/// Caution initiale : 10 000 VINX.
pub const DEFAULT_VALIDATOR_BOND: Amount = 10_000 * UNITS_PER_VINX;

/// Délai de grâce des validateurs en place à l'activation du protocole 7 pour
/// déposer leur caution : 30 jours.
pub const BOND_GRACE: u64 = 30 * 86_400;

/// Paramètres du protocole 7 (`SetParam`, codes `0x0E` à `0x15`).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct StakingParams {
    /// Caution exigée, en unités.
    pub validator_bond: Amount,
    /// Disponibilité minimale pour être récompensé, en dix-millièmes.
    pub min_availability: u64,
    /// Part du pot versée au prorata des signatures, en dix-millièmes.
    pub vote_share: u64,
    /// Blocs consécutifs sans signature avant la mise à l'écart.
    pub absence_blocks: u64,
    /// Blocs minimaux entre la mise à l'écart et le retour.
    pub jail_min_blocks: u64,
    /// Délai de restitution de la caution après le départ, en secondes.
    pub bond_unlock_delay: u64,
    /// Délai d'exécution des actions admin, en secondes.
    pub admin_delay: u64,
    /// Règle de diversité des opérateurs : 0 ou 1.
    pub diversity: u64,
}

impl Default for StakingParams {
    fn default() -> Self {
        Self {
            validator_bond: DEFAULT_VALIDATOR_BOND,
            min_availability: 9_700,
            vote_share: 5_000,
            absence_blocks: 1_000,
            jail_min_blocks: 1_000,
            bond_unlock_delay: 1_209_600,
            admin_delay: 172_800,
            diversity: 0,
        }
    }
}

/// Situation d'un validateur.
#[derive(Clone, Copy, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
#[borsh(use_discriminant = true)]
#[repr(u8)]
pub enum ValidatorStatus {
    /// Dans l'ensemble (ou sur le point d'y entrer).
    Active = 0,
    /// Mis à l'écart pour absence depuis la hauteur `since` ; caution conservée.
    Jailed { since: u64 } = 1,
    /// Sorti ; caution restituable à partir de `unlock_at` (secondes).
    Leaving { unlock_at: u64 } = 2,
    /// Sorti, caution restituée.
    Left = 3,
    /// Exclu définitivement après une double signature prouvée.
    Banned = 4,
}

/// Fiche publique d'un validateur.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ValidatorRecord {
    pub identity: ValidatorIdentity,
    pub website: String,
    pub contact: String,
    pub status: ValidatorStatus,
    /// Hauteur d'entrée (ou d'activation du protocole 7 pour les validateurs
    /// déjà en place).
    pub joined_at: u64,
    /// Jusqu'à quand (secondes) le validateur peut rester sans caution
    /// suffisante : validateurs en place à l'activation. 0 : aucun délai.
    pub grace_until: u64,
}

impl Row for ValidatorRecord {
    const TAG: u8 = 10;
}

/// Index des noms, en minuscules : un nom pris l'est pour toujours.
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct NameOwner {
    pub key: PublicKey,
}

impl Row for NameOwner {
    const TAG: u8 = 11;
}

/// Caution bloquée pour une clé de validateur.
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Bond {
    pub amount: Amount,
}

impl Row for Bond {
    const TAG: u8 = 12;
    fn locked(&self) -> u128 {
        u128::from(self.amount)
    }
}

/// Action admin acceptée, exécutée à partir de `executes_at` (secondes).
#[derive(Clone, Debug, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PendingAdmin {
    pub action: AdminAction,
    /// Hauteur du bloc qui l'a incluse.
    pub queued_at: u64,
    pub executes_at: u64,
}

impl Row for PendingAdmin {
    const TAG: u8 = 13;
}

/// Présence d'un validateur, mesurée par `last_commit` (protocole 7, §5).
#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Presence {
    /// Blocs de l'époque que le validateur a signés.
    pub signed: u64,
    /// Blocs de l'époque où il faisait partie de l'ensemble.
    pub expected: u64,
    /// Blocs consécutifs sans sa signature (survit au changement d'époque).
    pub missed_in_row: u64,
    /// Mis à l'écart pendant l'époque : aucune récompense.
    pub jailed_this_epoch: bool,
}

impl Presence {
    /// Disponibilité suffisante : `signés / attendus ≥ min / 10 000`.
    pub fn meets(&self, min_availability: u64) -> bool {
        self.expected > 0
            && u128::from(self.signed).saturating_mul(10_000)
                >= u128::from(self.expected).saturating_mul(u128::from(min_availability))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Staking {
    pub params: StakingParams,
    /// Présence de l'époque en cours, par validateur.
    pub presence: std::collections::BTreeMap<PublicKey, Presence>,
    pub records: Table<PublicKey, ValidatorRecord>,
    pub names: Table<String, NameOwner>,
    pub bonds: Table<PublicKey, Bond>,
    pub pending: Table<Hash, PendingAdmin>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum StakingError {
    #[error(
        "identité de validateur invalide : nom de 3 à 24 caractères (lettres, chiffres, tiret, espace), opérateur de 1 à 64 octets, pays en deux lettres majuscules"
    )]
    InvalidIdentity,
    #[error("ce nom de validateur est déjà pris (un nom n'est jamais réattribué)")]
    NameTaken,
    #[error("caution insuffisante pour ce validateur")]
    BondTooLow,
    #[error("ce validateur est exclu définitivement")]
    Banned,
    #[error("aucune fiche de validateur pour cette clé")]
    UnknownValidator,
    #[error("l'adresse de l'émetteur ne correspond pas à la clé du validateur")]
    NotValidatorAddress,
    #[error(
        "site ou contact trop long ({} octets au plus)",
        vinx_core::MAX_VALIDATOR_INFO
    )]
    InfoTooLong,
    #[error("cet opérateur aurait trop de validateurs (règle de diversité)")]
    Diversity,
    #[error("aucune action admin en attente avec cet identifiant")]
    UnknownPending,
    #[error("action remplacée au protocole 7 (utiliser AddNamedValidator)")]
    Obsolete,
    #[error("ce validateur n'est pas mis à l'écart")]
    NotJailed,
    #[error("trop tôt : le délai minimal après la mise à l'écart n'est pas écoulé")]
    JailTooRecent,
    #[error("ce validateur a déjà quitté l'ensemble")]
    AlreadyLeft,
    #[error("caution pas encore restituable (validateur actif ou délai non écoulé)")]
    BondLocked,
    #[error("aucune caution à restituer")]
    NoBond,
}

/// Forme comparable d'un nom : minuscules.
pub fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Forme comparable d'un opérateur : minuscules, espaces réduits.
pub fn operator_key(operator: &str) -> String {
    operator
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

impl Staking {
    pub fn bond(&self, key: &PublicKey) -> Amount {
        self.bonds.get(key).map_or(0, |b| b.amount)
    }

    /// Clé du validateur dont `address` est l'adresse.
    pub fn key_of(&self, address: &Address) -> Option<PublicKey> {
        self.records
            .iter()
            .map(|(k, _)| k)
            .find(|k| Address::from_public_key(k) == *address)
            .cloned()
    }

    /// Le nom est-il libre pour `key` (libre, ou déjà le sien) ?
    pub fn name_available(&self, name: &str, key: &PublicKey) -> bool {
        self.names
            .get(&name_key(name))
            .is_none_or(|owner| owner.key == *key)
    }

    /// Règle de diversité : avec `set` (l'ensemble de validateurs envisagé),
    /// aucun opérateur ne détient plus de `⌊(n − 1)/3⌋` sièges. `operator_of`
    /// donne l'opérateur d'une clé, y compris celle qu'on ajoute.
    pub fn diversity_ok(
        &self,
        set: &[PublicKey],
        operator_of: impl Fn(&PublicKey) -> String,
    ) -> bool {
        if self.params.diversity == 0 {
            return true;
        }
        let n = set.len();
        let max = n.saturating_sub(1) / 3;
        let mut counts: std::collections::BTreeMap<String, usize> = Default::default();
        for key in set {
            let c = counts.entry(operator_key(&operator_of(key))).or_insert(0);
            *c = c.saturating_add(1);
        }
        counts.values().all(|c| *c <= max.max(1))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn id(name: &str, operator: &str) -> ValidatorIdentity {
        ValidatorIdentity {
            name: name.into(),
            operator: operator.into(),
            country: *b"FR",
        }
    }

    #[test]
    fn identities_are_checked() {
        assert!(id("Cinnamon", "VinX Labs").is_valid());
        assert!(id("validateur-1", "VinX Labs").is_valid());
        assert!(!id("ab", "x").is_valid(), "nom trop court");
        assert!(!id("Cinna_mon", "x").is_valid(), "caractère interdit");
        assert!(!id(" Cinnamon", "x").is_valid(), "espace en tête");
        assert!(!id("Cinnamon", "  ").is_valid(), "opérateur vide");
        let mut bad = id("Cinnamon", "x");
        bad.country = *b"fr";
        assert!(!bad.is_valid(), "pays en minuscules");
    }

    #[test]
    fn diversity_caps_each_operator() {
        let mut s = Staking::default();
        let keys: Vec<PublicKey> = (0..7).map(|_| KeyPair::generate().public_key()).collect();
        let same = |_: &PublicKey| "VinX Labs".to_owned();
        assert!(s.diversity_ok(&keys, same), "désactivée par défaut");
        s.params.diversity = 1;
        assert!(!s.diversity_ok(&keys, same));
        // 7 validateurs : au plus 2 par opérateur.
        let two_each = |k: &PublicKey| {
            let i = keys.iter().position(|x| x == k).unwrap();
            format!("op {}", i / 2)
        };
        assert!(s.diversity_ok(&keys, two_each));
        let three = |k: &PublicKey| {
            let i = keys.iter().position(|x| x == k).unwrap();
            if i < 3 { "A".into() } else { format!("B{i}") }
        };
        assert!(!s.diversity_ok(&keys, three));
        // L'opérateur est comparé sans majuscules ni espaces en trop.
        assert_eq!(operator_key("  VinX   Labs "), "vinx labs");
    }
}
