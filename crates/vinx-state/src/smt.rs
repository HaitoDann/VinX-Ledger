//! Racine de l'arbre de Merkle creux des comptes (SPEC §4.1).
//!
//! Arbre binaire compact indexé par les 160 bits de l'adresse, du bit de poids
//! fort au bit de poids faible :
//! - sous-arbre vide : 32 octets nuls ;
//! - sous-arbre à un seul compte : `H("vinx/smt/leaf" ‖ adresse ‖ borsh(compte))` ;
//! - sinon : `H("vinx/smt/node" ‖ gauche ‖ droite)`, en séparant selon le bit suivant.
//!
//! La racine ne dépend que de l'ensemble des comptes, pas de l'ordre d'insertion.
//!
//! Les empreintes des sous-arbres proches de la racine sont mises en cache ;
//! modifier un compte n'invalide que son chemin. Après un bloc, seules les
//! branches touchées sont recalculées.

use imbl::OrdMap;
use vinx_core::{Account, encode};
use vinx_crypto::{ADDRESS_LEN, Address, Domain, Hash, hash_tagged_parts};

/// Racine d'un sous-arbre vide.
pub const EMPTY: Hash = [0; 32];

/// Profondeur jusqu'à laquelle les sous-arbres sont mis en cache. Au-delà, ils
/// sont petits (une quinzaine de comptes pour un million) et recalculés.
const CACHE_DEPTH: usize = 16;

const BITS: usize = ADDRESS_LEN * 8;

/// Sous-arbre : `depth` premiers bits de `prefix` (les autres sont nuls).
type NodeId = (u8, [u8; ADDRESS_LEN]);

/// Cache des empreintes de sous-arbres. Sa copie est quasi gratuite.
pub type SmtCache = OrdMap<NodeId, Hash>;

fn bit(bytes: &[u8; ADDRESS_LEN], depth: usize) -> bool {
    let byte = bytes.get(depth >> 3).copied().unwrap_or(0);
    // Bit `depth` en partant du poids fort : on l'amène en position 7.
    byte.rotate_left((depth & 7) as u32) & 0x80 != 0
}

fn set_bit(bytes: &mut [u8; ADDRESS_LEN], depth: usize) {
    if let Some(b) = bytes.get_mut(depth >> 3) {
        *b |= 0x80u8.rotate_right((depth & 7) as u32);
    }
}

/// Garde les `depth` premiers bits de `bytes` ; `fill` met les autres à 1.
fn mask(bytes: &[u8; ADDRESS_LEN], depth: usize, fill: bool) -> [u8; ADDRESS_LEN] {
    let mut out = [if fill { 0xff } else { 0 }; ADDRESS_LEN];
    for d in 0..depth.min(BITS) {
        let idx = d >> 3;
        let m = 0x80u8.rotate_right((d & 7) as u32);
        if let Some(o) = out.get_mut(idx) {
            if bit(bytes, d) {
                *o |= m;
            } else {
                *o &= !m;
            }
        }
    }
    out
}

fn leaf(address: &Address, account: &Account) -> Hash {
    hash_tagged_parts(Domain::SmtLeaf, &[address.as_bytes(), &encode(account)])
}

fn subtree(
    accounts: &OrdMap<Address, Account>,
    cache: &mut SmtCache,
    prefix: [u8; ADDRESS_LEN],
    depth: usize,
) -> Hash {
    let id: NodeId = (u8::try_from(depth).unwrap_or(u8::MAX), prefix);
    if depth < CACHE_DEPTH {
        if let Some(h) = cache.get(&id) {
            return *h;
        }
    }
    let lo = Address::from_bytes(prefix);
    let hi = Address::from_bytes(mask(&prefix, depth, true));
    let mut range = accounts.range(lo..=hi);
    let hash = match (range.next(), range.next()) {
        (None, _) => EMPTY,
        (Some((address, account)), None) => leaf(address, account),
        _ => {
            let next = depth.saturating_add(1);
            let mut right = prefix;
            set_bit(&mut right, depth);
            hash_tagged_parts(
                Domain::SmtNode,
                &[
                    &subtree(accounts, cache, prefix, next),
                    &subtree(accounts, cache, right, next),
                ],
            )
        }
    };
    if depth < CACHE_DEPTH {
        cache.insert(id, hash);
    }
    hash
}

/// Racine des comptes, en réutilisant et complétant `cache`.
pub fn root(accounts: &OrdMap<Address, Account>, cache: &mut SmtCache) -> Hash {
    subtree(accounts, cache, [0; ADDRESS_LEN], 0)
}

/// Invalide le chemin d'un compte modifié.
pub fn invalidate(cache: &mut SmtCache, address: &Address) {
    for depth in 0..CACHE_DEPTH {
        let id = (
            u8::try_from(depth).unwrap_or(u8::MAX),
            mask(address.as_bytes(), depth, false),
        );
        cache.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Définition directe de SPEC §4.1, sans cache : la référence.
    fn reference(accounts: &BTreeMap<Address, Account>) -> Hash {
        fn go(entries: &[(&Address, &Account)], depth: usize) -> Hash {
            match entries {
                [] => EMPTY,
                [(a, acc)] => leaf(a, acc),
                _ => {
                    let split = entries.partition_point(|(a, _)| !bit(a.as_bytes(), depth));
                    let (l, r) = entries.split_at(split);
                    hash_tagged_parts(
                        Domain::SmtNode,
                        &[
                            &go(l, depth.saturating_add(1)),
                            &go(r, depth.saturating_add(1)),
                        ],
                    )
                }
            }
        }
        let entries: Vec<_> = accounts.iter().collect();
        go(&entries, 0)
    }

    fn acc(balance: u64) -> Account {
        Account {
            balance,
            ..Account::default()
        }
    }

    fn address(i: u32) -> Address {
        let mut a = [0u8; 20];
        a[..4].copy_from_slice(&i.wrapping_mul(2_654_435_761).to_be_bytes());
        a[16..].copy_from_slice(&i.to_be_bytes());
        Address::from_bytes(a)
    }

    #[test]
    fn empty_single_and_pairs_match_the_definition() {
        let mut cache = SmtCache::new();
        assert_eq!(root(&OrdMap::new(), &mut cache), EMPTY);
        for n in [1u32, 2, 3, 17, 300] {
            let map: BTreeMap<_, _> = (0..n).map(|i| (address(i), acc(u64::from(i)))).collect();
            let ord: OrdMap<_, _> = map.clone().into_iter().collect();
            assert_eq!(root(&ord, &mut SmtCache::new()), reference(&map), "n = {n}");
        }
    }

    #[test]
    fn neighbours_sharing_a_long_prefix() {
        let a = Address::from_bytes([0x00; 20]);
        let mut bb = [0x00; 20];
        bb[19] = 1; // ne diffère qu'au tout dernier bit
        let b = Address::from_bytes(bb);
        let map = BTreeMap::from([(a, acc(1)), (b, acc(2))]);
        let ord: OrdMap<_, _> = map.clone().into_iter().collect();
        assert_eq!(root(&ord, &mut SmtCache::new()), reference(&map));
    }

    /// Le cache invalidé chemin par chemin donne toujours la racine exacte.
    #[test]
    fn incremental_updates_match_full_recomputation() {
        let mut map: BTreeMap<_, _> = (0..2_000u32).map(|i| (address(i), acc(1))).collect();
        let mut ord: OrdMap<_, _> = map.clone().into_iter().collect();
        let mut cache = SmtCache::new();
        root(&ord, &mut cache);
        for step in 0..200u32 {
            // Modifie un compte existant, et en crée un nouveau.
            let existing = address(step * 7 % 2_000);
            let fresh = address(10_000 + step);
            for (a, v) in [(existing, u64::from(step) + 5), (fresh, 3)] {
                map.insert(a, acc(v));
                ord.insert(a, acc(v));
                invalidate(&mut cache, &a);
            }
            if step % 20 == 0 {
                assert_eq!(root(&ord, &mut cache), reference(&map), "étape {step}");
            }
        }
        assert_eq!(root(&ord, &mut cache), reference(&map));
    }
}
