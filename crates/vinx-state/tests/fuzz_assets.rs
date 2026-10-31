// Code de test : les lints réservés au code de production ne s'appliquent pas.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing
)]

//! Fuzzing de la machine d'état des jetons (protocoles 5 et 6) : des suites
//! aléatoires d'opérations, correctement signées, entre quelques comptes.
//! Chaque opération réussit ou est refusée proprement ; après chacune :
//! - la supply du VINX est conservée (invariant de la chaîne) ;
//! - la supply de chaque jeton égale ses soldes plus ses séquestres ;
//! - à la fin, l'état relu ligne par ligne redonne la même racine.

mod common;

use common::*;
use proptest::prelude::*;
use vinx_core::{AssetPowers, SignedTransaction, TxBody, deal_id};
use vinx_crypto::{Hash, KeyPair};
use vinx_state::State;

const SYMBOLS: [&str; 2] = ["AAA", "BBB"];

#[derive(Debug, Clone)]
enum Op {
    Accept(usize, usize, bool),
    Transfer(usize, usize, usize, u64),
    Mint(usize, usize, u64),
    Burn(usize, usize, u64),
    Freeze(usize, Option<usize>, bool),
    Clawback(usize, usize, u64),
    Renounce(usize, bool, bool, bool),
    Deal(usize, usize, usize, u64),
    Settle(usize, usize, bool),
    Mandate(usize, usize, usize, u64),
    Revoke(usize, usize, usize),
    Collect(usize, usize, usize, u64),
    Vinx(usize, usize, u64),
}

/// Montants : petits, moyens, et parfois énormes pour viser les débordements.
fn amount() -> impl Strategy<Value = u64> {
    prop_oneof![0u64..50, 50u64..5_000, Just(u64::MAX), Just(u64::MAX / 2)]
}

fn op() -> impl Strategy<Value = Op> {
    let who = 0usize..4;
    let sym = 0usize..2;
    prop_oneof![
        (who.clone(), sym.clone(), any::<bool>()).prop_map(|(a, s, y)| Op::Accept(a, s, y)),
        (who.clone(), who.clone(), sym.clone(), amount())
            .prop_map(|(a, b, s, n)| Op::Transfer(a, b, s, n)),
        (who.clone(), sym.clone(), amount()).prop_map(|(b, s, n)| Op::Mint(b, s, n)),
        (who.clone(), sym.clone(), amount()).prop_map(|(a, s, n)| Op::Burn(a, s, n)),
        (sym.clone(), prop::option::of(who.clone()), any::<bool>())
            .prop_map(|(s, a, f)| Op::Freeze(s, a, f)),
        (sym.clone(), who.clone(), amount()).prop_map(|(s, a, n)| Op::Clawback(s, a, n)),
        (sym.clone(), any::<bool>(), any::<bool>(), any::<bool>())
            .prop_map(|(s, m, f, c)| Op::Renounce(s, m, f, c)),
        (who.clone(), who.clone(), sym.clone(), amount())
            .prop_map(|(a, b, s, n)| Op::Deal(a, b, s, n)),
        (who.clone(), 0usize..8, any::<bool>()).prop_map(|(a, d, r)| Op::Settle(a, d, r)),
        (who.clone(), who.clone(), sym.clone(), amount())
            .prop_map(|(a, b, s, n)| Op::Mandate(a, b, s, n)),
        (who.clone(), who.clone(), sym.clone()).prop_map(|(a, b, s)| Op::Revoke(a, b, s)),
        (who.clone(), who.clone(), sym.clone(), amount())
            .prop_map(|(a, b, s, n)| Op::Collect(a, b, s, n)),
        (who.clone(), who, amount()).prop_map(|(a, b, n)| Op::Vinx(a, b, n)),
    ]
}

struct World {
    net: Net,
    /// 0 : émetteur des deux jetons ; 1 à 3 : détenteurs.
    people: Vec<KeyPair>,
    deals: Vec<Hash>,
}

impl World {
    fn new() -> Self {
        let mut net = Net::current();
        let people: Vec<KeyPair> = (0..4).map(|_| net.funded(10_000 * VINX)).collect();
        let mut w = Self {
            net,
            people,
            deals: Vec::new(),
        };
        for s in SYMBOLS {
            assert!(w.send(
                0,
                TxBody::RegisterName {
                    name: s.to_lowercase()
                }
            ));
            assert!(w.send(
                0,
                TxBody::CreateAsset {
                    symbol: s.into(),
                    name: format!("Jeton {s}"),
                    decimals: 2,
                    initial_supply: 1_000,
                    max_supply: Some(1_000_000),
                    powers: AssetPowers {
                        mint: true,
                        freeze: true,
                        clawback: true
                    },
                }
            ));
        }
        w
    }

    fn tx(&self, who: usize, body: TxBody) -> SignedTransaction {
        let a = addr(&self.people[who]);
        let acc = self.net.state.account(&a);
        let nonce = acc.map_or(0, |x| x.nonce) + 1;
        let first = acc.is_none_or(|x| x.key.is_none());
        user_tx(&self.people[who], nonce, body, first)
    }

    /// Envoie la transaction dans un bloc ; `false` si elle est refusée.
    fn send(&mut self, who: usize, body: TxBody) -> bool {
        let tx = self.tx(who, body);
        self.net.block(T0, &[], &[tx]).is_ok()
    }

    fn apply(&mut self, op: &Op) {
        let a = |i: usize| addr(&self.people[i]);
        let sym = |s: usize| SYMBOLS[s].to_owned();
        match *op {
            Op::Accept(w, s, yes) => {
                self.send(
                    w,
                    TxBody::AcceptAsset {
                        symbol: sym(s),
                        accept: yes,
                    },
                );
            }
            Op::Transfer(w, to, s, n) => {
                let body = TxBody::TransferAsset {
                    symbol: sym(s),
                    to: a(to),
                    amount: n,
                    memo: None,
                };
                self.send(w, body);
            }
            Op::Mint(to, s, n) => {
                self.send(
                    0,
                    TxBody::MintAsset {
                        symbol: sym(s),
                        to: a(to),
                        amount: n,
                    },
                );
            }
            Op::Burn(w, s, n) => {
                self.send(
                    w,
                    TxBody::BurnAsset {
                        symbol: sym(s),
                        amount: n,
                    },
                );
            }
            Op::Freeze(s, who, frozen) => {
                self.send(
                    0,
                    TxBody::FreezeAsset {
                        symbol: sym(s),
                        account: who.map(a),
                        frozen,
                    },
                );
            }
            Op::Clawback(s, from, n) => {
                self.send(
                    0,
                    TxBody::ClawbackAsset {
                        symbol: sym(s),
                        from: a(from),
                        amount: n,
                    },
                );
            }
            Op::Renounce(s, mint, freeze, clawback) => {
                let powers = AssetPowers {
                    mint,
                    freeze,
                    clawback,
                };
                self.send(
                    0,
                    TxBody::RenounceAssetPowers {
                        symbol: sym(s),
                        powers,
                    },
                );
            }
            Op::Deal(w, seller, s, n) => {
                let tx = self.tx(
                    w,
                    TxBody::OpenAssetDeal {
                        symbol: sym(s),
                        seller: a(seller),
                        arbiter: None,
                        amount: n,
                        deadline: T0 + 3_600,
                        memo: None,
                    },
                );
                let id = deal_id(&tx.tx.sender, tx.tx.nonce);
                if self.net.block(T0, &[], &[tx]).is_ok() {
                    self.deals.push(id);
                }
            }
            Op::Settle(w, d, release) => {
                if let Some(id) = self.deals.get(d).copied() {
                    let body = if release {
                        TxBody::ReleaseDeal { id }
                    } else {
                        TxBody::RefundDeal { id }
                    };
                    self.send(w, body);
                }
            }
            Op::Mandate(w, payee, s, n) => {
                let body = TxBody::GrantAssetMandate {
                    symbol: sym(s),
                    payee: a(payee),
                    max_per_period: n,
                    period: 86_400,
                    expires_at: None,
                };
                self.send(w, body);
            }
            Op::Revoke(w, payee, s) => {
                self.send(
                    w,
                    TxBody::RevokeAssetMandate {
                        symbol: sym(s),
                        payee: a(payee),
                    },
                );
            }
            Op::Collect(w, payer, s, n) => {
                let body = TxBody::CollectAsset {
                    symbol: sym(s),
                    payer: a(payer),
                    amount: n,
                    memo: None,
                };
                self.send(w, body);
            }
            Op::Vinx(w, to, n) => {
                let body = TxBody::Transfer {
                    to: a(to),
                    amount: n % (20_000 * VINX),
                    memo: None,
                };
                self.send(w, body);
            }
        }
    }

    /// Supply de chaque jeton = soldes + unités en séquestre.
    fn check(&self) -> Result<(), TestCaseError> {
        let st = &self.net.state;
        prop_assert!(st.check_invariant().is_ok(), "supply du VINX non conservée");
        for s in SYMBOLS {
            let asset = st.assets.assets.get(s).unwrap();
            let held: u128 = st
                .assets
                .holdings
                .iter()
                .filter(|((sym, _), _)| sym == s)
                .map(|(_, h)| u128::from(h.amount))
                .sum();
            let escrowed: u128 = st
                .assets
                .deals
                .values()
                .filter(|d| d.symbol == s)
                .map(|d| u128::from(d.deal.amount))
                .sum();
            prop_assert_eq!(
                u128::from(asset.supply),
                held + escrowed,
                "supply de {} incohérente",
                s
            );
            if let Some(max) = asset.max_supply {
                prop_assert!(asset.supply <= max, "plafond de {} dépassé", s);
            }
        }
        Ok(())
    }
}

/// L'état relu depuis ses lignes (comme au redémarrage) a la même racine.
fn reloads_identically(state: &State) -> Result<(), TestCaseError> {
    let root = state.state_root();
    let mut copy = state.clone();
    copy.mark_all_rows_dirty();
    let rows = copy.take_dirty_rows();
    let mut reloaded =
        State::from_parts(&copy.globals_bytes_light(), copy.accounts.clone()).unwrap();
    for (key, value) in rows {
        if let Some(v) = value {
            reloaded.load_row(&key, &v).unwrap();
        }
    }
    prop_assert_eq!(reloaded.state_root(), root, "état relu différent");
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn random_asset_operations_keep_every_invariant(ops in prop::collection::vec(op(), 1..40)) {
        let mut w = World::new();
        w.check()?;
        for op in &ops {
            w.apply(op);
            w.check()?;
        }
        reloads_identically(&w.net.state)?;
    }
}

/// Garde-fou du fuzzing : les opérations y réussissent vraiment (sinon il ne
/// testerait que des refus).
#[test]
fn the_fuzzing_world_really_moves_tokens() {
    let mut w = World::new();
    let bob = addr(&w.people[1]);
    w.apply(&Op::Accept(1, 0, true));
    w.apply(&Op::Transfer(0, 1, 0, 300));
    w.apply(&Op::Mint(1, 0, 50));
    w.apply(&Op::Deal(1, 0, 0, 100));
    assert_eq!(w.net.state.assets.balance("AAA", &bob), 250);
    assert_eq!(w.deals.len(), 1);
    w.apply(&Op::Settle(1, 0, true));
    assert_eq!(w.net.state.assets.balance("AAA", &addr(&w.people[0])), 800);
    w.check().unwrap();
}
