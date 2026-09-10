//! Fonction de hachage du protocole VinX (ADR 0069).

/// Condensat de 32 octets — l'unique largeur de hachage du protocole.
pub type Hash32 = [u8; 32];

/// Hachage canonique de VinX : **BLAKE3** (ADR 0069).
///
/// C'est le point d'étranglement unique du protocole : hachages de blocs, de transactions,
/// racines de Merkle, dérivation d'adresse, `consensus_root` et départages déterministes
/// passent tous par ici. Changer cette fonction change **tous** les hachages du protocole —
/// c'est un hard fork, à ne faire qu'avant la genèse (voir ADR 0079 §2.2).
///
/// # Pourquoi BLAKE3 plutôt que SHA-256
///
/// Même marge de sécurité (128 bits en collision) pour un débit de 5 à 8× supérieur sur du
/// matériel moderne, ce qui compte sur les chemins chauds : vérification de blocs, rebuild
/// de l'arbre de Merkle, résolution de compact blocks.
///
/// Un point de sécurité mérite d'être noté : BLAKE3 n'est **pas** vulnérable à l'extension
/// de longueur, contrairement à SHA-256 (Merkle-Damgård). VinX ne dépendait pas de cette
/// propriété — les encodages signés sont injectifs par construction depuis ADR 0073 — mais
/// elle retire une classe entière de pièges pour tout futur encodage.
pub fn hash256(data: &[u8]) -> Hash32 {
    *blake3::hash(data).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vecteur officiel BLAKE3 : `BLAKE3("")`. Fige l'algorithme — si quelqu'un
    /// réintroduit SHA-256 ou change de fonction, ce test tombe.
    #[test]
    fn test_hash256_matches_blake3_reference_vector() {
        assert_eq!(
            hex::encode(hash256(b"")),
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    /// Vecteur officiel BLAKE3 : `BLAKE3("abc")`.
    #[test]
    fn test_hash256_abc_reference_vector() {
        assert_eq!(
            hex::encode(hash256(b"abc")),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }

    /// Le condensat ne doit surtout PAS être celui de SHA-256 — garde explicite contre un
    /// retour en arrière silencieux.
    #[test]
    fn test_hash256_is_not_sha256() {
        let sha256_empty =
            hex::decode("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
                .unwrap();
        assert_ne!(hash256(b"").as_slice(), sha256_empty.as_slice());
    }

    #[test]
    fn test_hash256_deterministic() {
        assert_eq!(hash256(b"vinx ledger"), hash256(b"vinx ledger"));
    }

    #[test]
    fn test_hash256_different_inputs() {
        assert_ne!(hash256(b"hello"), hash256(b"world"));
    }
}
