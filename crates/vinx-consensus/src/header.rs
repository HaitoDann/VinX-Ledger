//! Règles de chaînage des en-têtes (SPEC §6, §7.3).

use thiserror::Error;
use vinx_core::BlockHeader;
use vinx_core::params::MAX_CLOCK_DRIFT_MS;

/// Proposeur du round `round` à la hauteur `height` : `(height + round) mod n`.
pub fn proposer_index(height: u64, round: u32, n: usize) -> Option<u16> {
    let n = u64::try_from(n).ok().filter(|n| *n > 0)?;
    let index = height.wrapping_add(u64::from(round)).checked_rem(n)?;
    u16::try_from(index).ok()
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum HeaderError {
    #[error("hauteur inattendue")]
    WrongHeight,
    #[error("parent incorrect")]
    WrongParent,
    #[error("mauvais identifiant de chaîne")]
    WrongChain,
    #[error("horodatage non croissant")]
    TimestampNotIncreasing,
    #[error("horodatage trop en avance sur l'horloge locale")]
    TimestampInFuture,
}

/// Vérifie qu'un en-tête prolonge correctement son parent.
///
/// `now_ms` est l'horloge locale du nœud qui vérifie.
pub fn check_header_link(
    parent: &BlockHeader,
    header: &BlockHeader,
    now_ms: u64,
) -> Result<(), HeaderError> {
    if parent.height.checked_add(1) != Some(header.height) {
        return Err(HeaderError::WrongHeight);
    }
    if header.prev_hash != parent.hash() {
        return Err(HeaderError::WrongParent);
    }
    if header.chain_id != parent.chain_id {
        return Err(HeaderError::WrongChain);
    }
    if header.timestamp <= parent.timestamp {
        return Err(HeaderError::TimestampNotIncreasing);
    }
    if header.timestamp > now_ms.saturating_add(MAX_CLOCK_DRIFT_MS) {
        return Err(HeaderError::TimestampInFuture);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(height: u64, timestamp: u64) -> BlockHeader {
        BlockHeader {
            protocol_version: 1,
            chain_id: "vinx-test".into(),
            height,
            timestamp,
            prev_hash: [0; 32],
            tx_root: [0; 32],
            state_root: [0; 32],
            validators_hash: [0; 32],
            proposer: 0,
        }
    }

    #[test]
    fn proposer_rotates() {
        let p: Vec<u16> = (0..5).map(|r| proposer_index(10, r, 4).unwrap()).collect();
        assert_eq!(p, [2, 3, 0, 1, 2]);
        assert_eq!(proposer_index(1, 0, 0), None);
    }

    #[test]
    fn link_rules() {
        let parent = header(5, 100_000);
        let mut child = header(6, 200_000);
        child.prev_hash = parent.hash();
        assert_eq!(check_header_link(&parent, &child, 200_000), Ok(()));

        let mut c = child.clone();
        c.height = 7;
        assert_eq!(
            check_header_link(&parent, &c, 200_000),
            Err(HeaderError::WrongHeight)
        );
        let mut c = child.clone();
        c.prev_hash = [1; 32];
        assert_eq!(
            check_header_link(&parent, &c, 200_000),
            Err(HeaderError::WrongParent)
        );
        let mut c = child.clone();
        c.chain_id = "autre".into();
        assert_eq!(
            check_header_link(&parent, &c, 200_000),
            Err(HeaderError::WrongChain)
        );
        let mut c = child.clone();
        c.timestamp = 100_000;
        assert_eq!(
            check_header_link(&parent, &c, 200_000),
            Err(HeaderError::TimestampNotIncreasing)
        );
        assert_eq!(
            check_header_link(&parent, &child, 200_000 - MAX_CLOCK_DRIFT_MS - 1),
            Err(HeaderError::TimestampInFuture)
        );
    }
}
