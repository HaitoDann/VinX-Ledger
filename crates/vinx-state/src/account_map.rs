//! Account map with O(1) clone (ADR 0083, L2).
//!
//! Every block is built and validated on a *copy* of the state, so a plain `BTreeMap` of
//! accounts made each proposal, validation and commit cost O(number of accounts). This
//! persistent B-tree (`imbl::OrdMap`) shares its nodes between copies: cloning is O(1)
//! and a copy pays only for the accounts it touches, O(log n) each.
//!
//! Serialization is byte-for-byte that of a `BTreeMap<Address, Account>` (sorted
//! entries), so the on-disk and consensus encodings are unchanged.

use std::collections::BTreeMap;
use std::ops::{Deref, DerefMut};

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use vinx_core::Account;
use vinx_crypto::Address;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccountMap(imbl::OrdMap<Address, Account>);

impl AccountMap {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Deref for AccountMap {
    type Target = imbl::OrdMap<Address, Account>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for AccountMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl FromIterator<(Address, Account)> for AccountMap {
    fn from_iter<I: IntoIterator<Item = (Address, Account)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl From<BTreeMap<Address, Account>> for AccountMap {
    fn from(m: BTreeMap<Address, Account>) -> Self {
        m.into_iter().collect()
    }
}

impl Serialize for AccountMap {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.0.iter())
    }
}

impl<'de> Deserialize<'de> for AccountMap {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        <BTreeMap<Address, Account> as Deserialize>::deserialize(d).map(Self::from)
    }
}

impl BorshSerialize for AccountMap {
    fn serialize<W: std::io::Write>(&self, w: &mut W) -> std::io::Result<()> {
        // Same layout as borsh's BTreeMap: u32 length, then entries in key order.
        let len = u32::try_from(self.0.len())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "too many"))?;
        BorshSerialize::serialize(&len, w)?;
        for (k, v) in self.0.iter() {
            BorshSerialize::serialize(k, w)?;
            BorshSerialize::serialize(v, w)?;
        }
        Ok(())
    }
}

impl BorshDeserialize for AccountMap {
    fn deserialize_reader<R: std::io::Read>(r: &mut R) -> std::io::Result<Self> {
        BTreeMap::<Address, Account>::deserialize_reader(r).map(Self::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::Amount;

    fn sample() -> BTreeMap<Address, Account> {
        (1u8..=20)
            .map(|i| {
                let a = Address::from_bytes([i; 20]);
                (
                    a,
                    Account::new_with_balance(a, Amount::from_atoms(i as u128)),
                )
            })
            .collect()
    }

    #[test]
    fn encodings_match_btreemap() {
        let b = sample();
        let m = AccountMap::from(b.clone());
        assert_eq!(borsh::to_vec(&m).unwrap(), borsh::to_vec(&b).unwrap());
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
        let back: AccountMap = borsh::from_slice(&borsh::to_vec(&b).unwrap()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn clone_is_independent() {
        let mut a = AccountMap::from(sample());
        let snapshot = a.clone();
        let k = Address::from_bytes([1; 20]);
        a.get_mut(&k).unwrap().balance = Amount::from_atoms(999);
        assert_eq!(snapshot[&k].balance, Amount::from_atoms(1));
        assert_eq!(a[&k].balance, Amount::from_atoms(999));
    }
}
