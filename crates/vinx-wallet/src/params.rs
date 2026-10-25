//! Noms des paramètres de gouvernance, tels qu'on les écrit dans
//! `admin propose set-param --param …` et dans le cockpit.

use anyhow::{Result, bail};
use vinx_core::ParamId;

/// Tous les paramètres, dans l'ordre de la SPEC.
pub const ALL: [(&str, ParamId); 10] = [
    ("min_fee", ParamId::MinFee),
    ("min_reserve", ParamId::MinReserve),
    ("max_memo_bytes", ParamId::MaxMemoBytes),
    ("max_block_txs", ParamId::MaxBlockTxs),
    ("max_block_bytes", ParamId::MaxBlockBytes),
    ("timeout_propose", ParamId::TimeoutPropose),
    ("timeout_propose_delta", ParamId::TimeoutProposeDelta),
    ("timeout_vote", ParamId::TimeoutVote),
    ("timeout_vote_delta", ParamId::TimeoutVoteDelta),
    ("min_block_interval", ParamId::MinBlockInterval),
];

pub fn param_id(name: &str) -> Result<ParamId> {
    match ALL.iter().find(|(n, _)| *n == name) {
        Some((_, id)) => Ok(*id),
        None => bail!(
            "paramètre inconnu : {name}. Paramètres valides : {} \
             (durées en millisecondes, montants en unités : 1 VINX = 1000000000)",
            ALL.map(|(n, _)| n).join(", ")
        ),
    }
}

pub fn param_name(id: ParamId) -> &'static str {
    ALL.iter()
        .find(|(_, p)| *p == id)
        .map_or("inconnu", |(n, _)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for (name, id) in ALL {
            assert_eq!(param_id(name).unwrap(), id);
            assert_eq!(param_name(id), name);
        }
        assert!(
            param_id("timeout_propose_ms")
                .unwrap_err()
                .to_string()
                .contains("timeout_propose,")
        );
    }
}
