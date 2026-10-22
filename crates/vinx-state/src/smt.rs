//! Racine de l'arbre de Merkle creux des comptes (SPEC §4.1).
//!
//! Arbre binaire compact indexé par les 160 bits de l'adresse, du bit de poids
//! fort au bit de poids faible :
//! - sous-arbre vide : 32 octets nuls ;
//! - sous-arbre à un seul compte : `H("vinx/smt/leaf" ‖ adresse ‖ borsh(compte))` ;
//! - sinon : `H("vinx/smt/node" ‖ gauche ‖ droite)`, en séparant selon le bit suivant.
//!
//! La racine ne dépend que de l'ensemble des comptes, pas de l'ordre d'insertion.

use std::collections::BTreeMap;
use vinx_core::{Account, encode};
use vinx_crypto::{ADDRESS_LEN, Address, Domain, Hash, hash_tagged_parts};

/// Racine d'un sous-arbre vide.
pub const EMPTY: Hash = [0; 32];

fn bit(address: &Address, depth: usize) -> bool {
    let byte = address.as_bytes().get(depth >> 3).copied().unwrap_or(0);
    // Bit `depth` en partant du poids fort : on l'amène en position 7.
    byte.rotate_left((depth & 7) as u32) & 0x80 != 0
}

fn leaf(address: &Address, account: &Account) -> Hash {
    hash_tagged_parts(Domain::SmtLeaf, &[address.as_bytes(), &encode(account)])
}

fn subtree(entries: &[(&Address, &Account)], depth: usize) -> Hash {
    match entries {
        [] => EMPTY,
        [(address, account)] => leaf(address, account),
        _ => {
            // Les entrées sont triées par adresse : celles dont le bit vaut 0 viennent d'abord.
            let split = entries.partition_point(|(a, _)| !bit(a, depth));
            let (left, right) = entries.split_at(split);
            let next = depth.saturating_add(1);
            hash_tagged_parts(
                Domain::SmtNode,
                &[&subtree(left, next), &subtree(right, next)],
            )
        }
    }
}

/// Racine des comptes.
pub fn root(accounts: &BTreeMap<Address, Account>) -> Hash {
    let entries: Vec<(&Address, &Account)> = accounts.iter().collect();
    debug_assert!(entries.len() < 2 || ADDRESS_LEN == 20);
    subtree(&entries, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acc(balance: u64) -> Account {
        Account {
            balance,
            ..Account::default()
        }
    }

    #[test]
    fn empty_and_single() {
        let mut m = BTreeMap::new();
        assert_eq!(root(&m), EMPTY);
        let a = Address::from_bytes([0x80; 20]);
        m.insert(a, acc(1));
        assert_eq!(root(&m), leaf(&a, &acc(1)));
    }

    #[test]
    fn split_on_first_differing_bit() {
        let a = Address::from_bytes([0x00; 20]); // bit 0 = 0
        let b = Address::from_bytes([0x80; 20]); // bit 0 = 1
        let m = BTreeMap::from([(a, acc(1)), (b, acc(2))]);
        let expected =
            hash_tagged_parts(Domain::SmtNode, &[&leaf(&a, &acc(1)), &leaf(&b, &acc(2))]);
        assert_eq!(root(&m), expected);
    }

    #[test]
    fn shared_prefix_goes_through_empty_siblings() {
        let a = Address::from_bytes([0x00; 20]);
        let mut bb = [0x00; 20];
        bb[0] = 0x40; // bit 0 = 0, bit 1 = 1
        let b = Address::from_bytes(bb);
        let m = BTreeMap::from([(a, acc(1)), (b, acc(2))]);
        let inner = hash_tagged_parts(Domain::SmtNode, &[&leaf(&a, &acc(1)), &leaf(&b, &acc(2))]);
        let expected = hash_tagged_parts(Domain::SmtNode, &[&inner, &EMPTY]);
        assert_eq!(root(&m), expected);
    }

    #[test]
    fn any_balance_change_changes_root() {
        let mut m: BTreeMap<Address, Account> = (0u8..50)
            .map(|i| {
                (
                    Address::from_bytes([i.wrapping_mul(37); 20]),
                    acc(u64::from(i)),
                )
            })
            .collect();
        let before = root(&m);
        if let Some(a) = m.values_mut().nth(17) {
            a.balance += 1;
        }
        assert_ne!(root(&m), before);
    }
}
