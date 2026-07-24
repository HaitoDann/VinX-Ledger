use crate::amount::Amount;
use serde::{Deserialize, Serialize};
use vinx_crypto::Address;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub address: Address,
    pub balance: Amount,
    pub nonce: u64,
    /// Bond posted to secure a validator seat. Earns no yield — it is skin in the
    /// game, slashable on equivocation. Withdrawing it goes through the unbonding
    /// delay (see `WorldState::pending_unbonds`), so it is not credited back here
    /// immediately on unstake.
    pub staked: Amount,
}

impl Account {
    pub fn new(address: Address) -> Self {
        Self {
            address,
            balance: Amount::ZERO,
            nonce: 0,
            staked: Amount::ZERO,
        }
    }

    pub fn new_with_balance(address: Address, balance: Amount) -> Self {
        Self {
            address,
            balance,
            nonce: 0,
            staked: Amount::ZERO,
        }
    }

    pub fn total_holdings(&self) -> Option<Amount> {
        self.balance.checked_add(self.staked)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_crypto::KeyPair;

    fn make_address() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    #[test]
    fn test_new_account_zero_balance() {
        let acc = Account::new(make_address());
        assert_eq!(acc.balance, Amount::ZERO);
        assert_eq!(acc.staked, Amount::ZERO);
        assert_eq!(acc.nonce, 0);
    }

    #[test]
    fn test_new_with_balance() {
        let addr = make_address();
        let acc = Account::new_with_balance(addr, Amount::from_vinx(1_000));
        assert_eq!(acc.balance, Amount::from_vinx(1_000));
    }

    #[test]
    fn test_total_holdings() {
        let mut acc = Account::new(make_address());
        acc.balance = Amount::from_vinx(100);
        acc.staked = Amount::from_vinx(50);
        assert_eq!(acc.total_holdings(), Some(Amount::from_vinx(150)));
    }
}
