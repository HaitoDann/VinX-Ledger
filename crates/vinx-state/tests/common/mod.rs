#![allow(dead_code, clippy::unwrap_used, clippy::arithmetic_side_effects)]

use vinx_core::{
    AdminAction, AdminTransaction, SignedAdminTransaction, SignedTransaction, Transaction, TxBody,
    UNITS_PER_VINX,
};
use vinx_crypto::{Address, KeyPair};
use vinx_state::{Genesis, State};

pub const CHAIN: &str = "vinx-test";
pub const T0: u64 = 1_800_000_000;
pub const PERIOD: u64 = 2_629_800;
pub const VINX: u64 = UNITS_PER_VINX;

pub struct Net {
    pub admins: [KeyPair; 3],
    pub validators: Vec<KeyPair>,
    pub state: State,
    pub admin_nonce: u64,
}

impl Net {
    /// Réseau aux règles d'origine (protocole 1, trésorerie de 100 M).
    pub fn new() -> Self {
        Self::with(Genesis::legacy)
    }

    /// Réseau d'une genèse standard (protocole 3), avec un robinet de 7 M.
    pub fn v3() -> Self {
        Self::with(|c, t, a, v| {
            Genesis::standard(c, t, a, v).map(|g| g.with_faucet(7_000_000 * VINX).unwrap())
        })
    }

    fn with(
        make: impl FnOnce(
            String,
            u64,
            [vinx_crypto::PublicKey; 3],
            Vec<vinx_crypto::PublicKey>,
        ) -> Option<Genesis>,
    ) -> Self {
        let admins = [
            KeyPair::generate(),
            KeyPair::generate(),
            KeyPair::generate(),
        ];
        let validators: Vec<KeyPair> = (0..4).map(|_| KeyPair::generate()).collect();
        let state = make(
            CHAIN.into(),
            T0,
            [
                admins[0].public_key(),
                admins[1].public_key(),
                admins[2].public_key(),
            ],
            validators.iter().map(KeyPair::public_key).collect(),
        )
        .unwrap()
        .into_state()
        .unwrap();
        Self {
            admins,
            validators,
            state,
            admin_nonce: 0,
        }
    }

    /// Transaction admin approuvée par les signataires 0 et 1.
    pub fn admin(&mut self, action: AdminAction) -> SignedAdminTransaction {
        self.admin_nonce += 1;
        let tx = AdminTransaction {
            chain_id: CHAIN.into(),
            nonce: self.admin_nonce,
            action,
        };
        SignedAdminTransaction {
            approvals: vec![
                tx.approve(0, &self.admins[0]),
                tx.approve(1, &self.admins[1]),
            ],
            tx,
        }
    }

    /// Exécute un bloc et l'adopte.
    pub fn block(
        &mut self,
        timestamp_s: u64,
        admin_txs: &[SignedAdminTransaction],
        txs: &[SignedTransaction],
    ) -> Result<(), vinx_state::BlockError> {
        let next = self
            .state
            .execute(self.state.height + 1, timestamp_s * 1000, admin_txs, txs)?;
        self.state = next;
        Ok(())
    }

    /// Comme [`Self::block`], avec l'indice du proposeur (émission, protocole 3).
    pub fn block_by(
        &mut self,
        timestamp_s: u64,
        proposer: u16,
        admin_txs: &[SignedAdminTransaction],
        txs: &[SignedTransaction],
    ) -> Result<(), vinx_state::BlockError> {
        let next = self.state.execute_by(
            self.state.height + 1,
            timestamp_s * 1000,
            Some(proposer),
            admin_txs,
            txs,
        )?;
        self.state = next;
        Ok(())
    }

    /// Crée et finance un compte depuis la trésorerie.
    pub fn funded(&mut self, amount: u64) -> KeyPair {
        let kp = KeyPair::generate();
        let tx = self.admin(AdminAction::TreasuryTransfer {
            to: addr(&kp),
            amount,
            memo: None,
        });
        self.block(T0, &[tx], &[]).unwrap();
        kp
    }
}

pub fn addr(kp: &KeyPair) -> Address {
    Address::from_public_key(&kp.public_key())
}

pub fn transfer(
    from: &KeyPair,
    nonce: u64,
    to: Address,
    amount: u64,
    first: bool,
) -> SignedTransaction {
    user_tx(
        from,
        nonce,
        TxBody::Transfer {
            to,
            amount,
            memo: None,
        },
        first,
    )
}

pub fn user_tx(from: &KeyPair, nonce: u64, body: TxBody, first: bool) -> SignedTransaction {
    Transaction {
        chain_id: CHAIN.into(),
        sender: addr(from),
        nonce,
        fee: VINX / 1000,
        body,
    }
    .sign(from, first)
}
