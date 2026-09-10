//! Checkpoints de subjectivité faible (ADR 0074 §2.3).
//!
//! # Le problème que cela résout
//!
//! Un nœud qui rejoint le réseau adopte un état entier par snapshot-sync, ou rejoue une
//! histoire servie par un pair. Les contrôles d'ADR 0074 §2.1 garantissent qu'un snapshot
//! est **internement cohérent** — la racine correspond à l'en-tête, le bloc porte un quorum
//! de son propre set de validateurs. Ils ne peuvent pas garantir qu'il s'agit de *la* bonne
//! histoire : un attaquant qui fabrique une chaîne complète, avec ses propres validateurs et
//! ses propres clés BLS enregistrées, produit un snapshot qui passe tous ces contrôles.
//!
//! C'est irréductible sans point d'ancrage extérieur : un nœud ne peut pas distinguer deux
//! histoires internement cohérentes. La réponse standard est la **subjectivité faible** —
//! livrer avec le binaire un petit nombre de `(hauteur, hash d'en-tête)` de confiance,
//! publiés hors-bande, et refuser toute histoire qui les contredit. Le hash livré avec le
//! binaire, lui, ne peut pas être fourni par le pair malveillant.
//!
//! # Règles appliquées
//!
//! - **Sync bloc à bloc** : tout bloc appliqué à une hauteur de checkpoint doit avoir le
//!   hash attendu.
//! - **Snapshot-sync** : lorsque des checkpoints sont configurés, le snapshot doit atterrir
//!   **exactement** sur l'un d'eux. Un snapshot n'apporte aucune histoire vérifiable, donc
//!   la seule chose qu'on puisse exiger est qu'il *soit* un point d'ancrage connu.
//!
//! Sans checkpoint configuré, les deux règles laissent tout passer et le nœud avertit : le
//! mode devnet reste utilisable, mais l'absence de garantie est dite explicitement plutôt
//! que supposée.

use vinx_crypto::Hash32;

/// Un point d'ancrage de confiance : la hauteur et le hash d'en-tête attendu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub height: u64,
    pub header_hash: Hash32,
}

/// Ensemble de checkpoints d'une chaîne, trié par hauteur croissante.
#[derive(Clone, Debug, Default)]
pub struct Checkpoints {
    inner: Vec<Checkpoint>,
}

impl Checkpoints {
    /// Ensemble vide — aucune garantie de subjectivité faible.
    pub fn none() -> Self {
        Self::default()
    }

    /// Construit depuis une liste `(hauteur, hash)`. Les doublons de hauteur sont un défaut
    /// de configuration : le dernier gagnerait silencieusement, donc on refuse.
    pub fn new(mut points: Vec<Checkpoint>) -> Result<Self, String> {
        points.sort_unstable_by_key(|c| c.height);
        if points.windows(2).any(|w| w[0].height == w[1].height) {
            return Err("deux checkpoints à la même hauteur".to_string());
        }
        Ok(Self { inner: points })
    }

    /// Parse une liste `"<hauteur>:<hash hex 64>"`, telle qu'un opérateur la configure.
    pub fn parse(entries: &[String]) -> Result<Self, String> {
        let mut points = Vec::with_capacity(entries.len());
        for e in entries {
            let (h, hash) = e
                .split_once(':')
                .ok_or_else(|| format!("checkpoint mal formé (attendu hauteur:hash) : {e}"))?;
            let height: u64 = h
                .trim()
                .parse()
                .map_err(|_| format!("hauteur de checkpoint invalide : {h}"))?;
            let raw = hex::decode(hash.trim())
                .map_err(|_| format!("hash de checkpoint non hexadécimal : {hash}"))?;
            let header_hash: Hash32 = raw.as_slice().try_into().map_err(|_| {
                format!(
                    "hash de checkpoint : 32 octets attendus, {} reçus",
                    raw.len()
                )
            })?;
            points.push(Checkpoint {
                height,
                header_hash,
            });
        }
        Self::new(points)
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// La hauteur du checkpoint le plus haut, s'il y en a un.
    pub fn highest(&self) -> Option<u64> {
        self.inner.last().map(|c| c.height)
    }

    /// Le hash attendu à `height`, si un checkpoint y est défini.
    pub fn expected_at(&self, height: u64) -> Option<Hash32> {
        self.inner
            .iter()
            .find(|c| c.height == height)
            .map(|c| c.header_hash)
    }

    /// Vérifie un bloc appliqué en sync : si un checkpoint existe à cette hauteur, le hash
    /// doit correspondre. Une hauteur sans checkpoint passe — on ne peut rien en dire.
    pub fn accepts_block(&self, height: u64, hash: Hash32) -> Result<(), String> {
        match self.expected_at(height) {
            Some(expected) if expected != hash => Err(format!(
                "le bloc à la hauteur {height} contredit le checkpoint de confiance \
                 (attendu {}, reçu {})",
                hex::encode(expected),
                hex::encode(hash)
            )),
            _ => Ok(()),
        }
    }

    /// Vérifie un snapshot : il doit atterrir exactement sur un checkpoint.
    ///
    /// Un snapshot ne transporte aucune histoire vérifiable ; exiger qu'il *soit* un point
    /// d'ancrage connu est la seule contrainte qui ait du sens. Sans checkpoint configuré,
    /// accepte en signalant l'absence de garantie à l'appelant.
    pub fn accepts_snapshot(&self, height: u64, hash: Hash32) -> Result<(), String> {
        if self.is_empty() {
            return Ok(());
        }
        match self.expected_at(height) {
            Some(expected) if expected == hash => Ok(()),
            Some(expected) => Err(format!(
                "le snapshot à la hauteur {height} contredit le checkpoint de confiance \
                 (attendu {}, reçu {})",
                hex::encode(expected),
                hex::encode(hash)
            )),
            None => Err(format!(
                "le snapshot à la hauteur {height} n'atterrit sur aucun checkpoint de \
                 confiance (hauteurs connues : {:?}) — un snapshot ne porte aucune histoire \
                 vérifiable, il doit donc être un point d'ancrage connu",
                self.inner.iter().map(|c| c.height).collect::<Vec<_>>()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> Hash32 {
        [b; 32]
    }

    #[test]
    fn empty_checkpoints_accept_everything_but_guarantee_nothing() {
        let c = Checkpoints::none();
        assert!(c.is_empty());
        assert!(c.accepts_block(100, h(1)).is_ok());
        assert!(c.accepts_snapshot(100, h(1)).is_ok());
        assert_eq!(c.highest(), None);
    }

    #[test]
    fn block_at_checkpoint_height_must_match() {
        let c = Checkpoints::new(vec![Checkpoint {
            height: 1000,
            header_hash: h(0xAA),
        }])
        .unwrap();
        assert!(c.accepts_block(1000, h(0xAA)).is_ok());
        assert!(
            c.accepts_block(1000, h(0xBB)).is_err(),
            "une histoire fabriquée doit être refusée au checkpoint"
        );
        // Une hauteur sans checkpoint ne dit rien.
        assert!(c.accepts_block(999, h(0xBB)).is_ok());
        assert!(c.accepts_block(1001, h(0xBB)).is_ok());
    }

    #[test]
    fn snapshot_must_land_on_a_checkpoint() {
        let c = Checkpoints::new(vec![
            Checkpoint {
                height: 1000,
                header_hash: h(0xAA),
            },
            Checkpoint {
                height: 2000,
                header_hash: h(0xCC),
            },
        ])
        .unwrap();
        assert!(c.accepts_snapshot(2000, h(0xCC)).is_ok());
        assert!(
            c.accepts_snapshot(2000, h(0xDD)).is_err(),
            "bon point d'ancrage, mauvais hash"
        );
        assert!(
            c.accepts_snapshot(1500, h(0xAA)).is_err(),
            "un snapshot hors point d'ancrage n'est pas vérifiable"
        );
        assert_eq!(c.highest(), Some(2000));
    }

    #[test]
    fn parsing_is_strict() {
        let ok = Checkpoints::parse(&[format!("1000:{}", hex::encode(h(0xAA)))]).unwrap();
        assert_eq!(ok.expected_at(1000), Some(h(0xAA)));

        assert!(
            Checkpoints::parse(&["1000".to_string()]).is_err(),
            "pas de séparateur"
        );
        assert!(
            Checkpoints::parse(&["abc:00".to_string()]).is_err(),
            "hauteur non numérique"
        );
        assert!(
            Checkpoints::parse(&["1000:zz".to_string()]).is_err(),
            "hash non hexadécimal"
        );
        assert!(
            Checkpoints::parse(&["1000:aabb".to_string()]).is_err(),
            "hash trop court"
        );
        // Deux checkpoints à la même hauteur : le dernier gagnerait silencieusement.
        let dup = format!("1000:{}", hex::encode(h(0xAA)));
        let dup2 = format!("1000:{}", hex::encode(h(0xBB)));
        assert!(Checkpoints::parse(&[dup, dup2]).is_err());
    }

    #[test]
    fn ordering_is_normalised() {
        let c = Checkpoints::new(vec![
            Checkpoint {
                height: 2000,
                header_hash: h(0xCC),
            },
            Checkpoint {
                height: 1000,
                header_hash: h(0xAA),
            },
        ])
        .unwrap();
        assert_eq!(c.highest(), Some(2000));
        assert_eq!(c.len(), 2);
    }
}
