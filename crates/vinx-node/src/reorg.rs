//! ADR 0031 — **mécanisme de réorganisation** (fork-choice, tranche 2b).
//!
//! Quand un bloc concurrent valide l'emporte au fork-choice (`canonical_head`), un nœud qui
//! avait retenu l'autre branche doit **basculer** : défaire les effets du bloc perdant sur
//! l'état et appliquer ceux du gagnant. VinX ne conserve qu'un seul état (celui de la branche
//! courante) ; on reconstruit donc l'état par **rejeu depuis un point sûr** — l'**option A**
//! (snapshot au point finalisé + rejeu), simple et manifestement correcte.
//!
//! Une réorg ne descend **jamais** sous `finalized_height` (plancher dur, ADR 0002) et reste
//! bornée par `MAX_UNFINALIZED_DEPTH` (refus de bâtir dans le vide) → le rejeu est court.
//!
//! **Ce module fournit la primitive pure de reconstruction.** Le déclenchement depuis le chemin
//! d'acceptation vivant (gossip/sync) + la maintenance du snapshot finalisé sont câblés par
//! l'appelant (node/p2p) et éprouvés au banc n=3.

use vinx_core::Block;
use vinx_state::WorldState;

use crate::chain::Chain;

/// Rejoue **un** bloc sur `state` via le chemin trusted (les signatures ne sont pas re-vérifiées
/// ici — le caller a déjà validé le bloc). L'horloge protocole (ADR 0005) est le MTP **indexé
/// par la hauteur du bloc** (`chain` fournit la fenêtre ; il doit contenir les timestamps
/// jusqu'à `height-1`, ce qui est le cas tant que la réorg n'a pas encore tronqué la chaîne).
///
/// Vérifie l'invariant de supply (ADR 0004) et le `state_root` du bloc : un désaccord signifie
/// que l'état reconstruit ne correspond pas à celui du producteur → la réorg est refusée
/// (sûreté). Ne touche **pas** à `chain`.
pub(crate) fn replay_block(
    state: &mut WorldState,
    chain: &Chain,
    block: &Block,
) -> Result<(), String> {
    let height = block.header.height;
    let protocol_ts = chain.median_time_past_ending_at(height, block.header.timestamp);
    state.set_block_context(protocol_ts);
    for tx in &block.transactions {
        state
            .apply_transaction_trusted(tx)
            .map_err(|e| format!("réorg: tx échouée au rejeu (h={height}): {e}"))?;
    }
    state.block_height = height;
    state.check_upgrade_activation();
    let _ = state.settle_block(&block.header.validator, height, protocol_ts);
    if !state.supply_invariant_holds() {
        return Err(format!(
            "réorg: invariant de supply rompu au rejeu (h={height})"
        ));
    }
    let root = state.compute_state_root();
    if root != block.header.state_root {
        return Err(format!(
            "réorg: state_root incohérent au rejeu (h={height}) — branche reconstruite ≠ producteur"
        ));
    }
    Ok(())
}

/// Reconstruit l'état de la **branche canonique** jusqu'à `contested_height` inclus, à partir de
/// `finalized_state` (l'état exact à `finalized_height`, snapshot sûr) :
/// 1. clone le snapshot finalisé (base immuable) ;
/// 2. rejoue les blocs **partagés** `finalized_height+1 .. contested_height-1` (encore présents
///    dans `chain`, communs aux deux branches) ;
/// 3. applique le **bloc canonique** à `contested_height` (`canonical_block`, potentiellement un
///    concurrent différent du bloc actuellement retenu).
///
/// `chain` n'est **pas** modifié (le caller appelle ensuite `Chain::reorg_replace` puis installe
/// l'état retourné). Renvoie `Err` si un rejeu échoue (supply/state_root) — la réorg est alors
/// abandonnée et l'état courant reste intact (sûreté : on ne bascule que sur une branche
/// entièrement re-vérifiée).
pub fn rebuild_canonical_state(
    finalized_state: &WorldState,
    finalized_height: u64,
    chain: &Chain,
    contested_height: u64,
    canonical_block: &Block,
) -> Result<WorldState, String> {
    if contested_height <= finalized_height {
        return Err(format!(
            "réorg interdite sous la finalité (contesté h={contested_height} ≤ finalisé {finalized_height})"
        ));
    }
    if canonical_block.header.height != contested_height {
        return Err("réorg: hauteur du bloc canonique incohérente".into());
    }
    let mut state = finalized_state.clone();
    // 2. Blocs partagés (communs aux deux branches) entre la finalité et la hauteur contestée.
    for h in (finalized_height + 1)..contested_height {
        let shared = chain
            .get_block(h)
            .ok_or_else(|| format!("réorg: bloc partagé manquant à h={h}"))?;
        replay_block(&mut state, chain, shared)?;
    }
    // 3. Bloc canonique à la hauteur contestée.
    replay_block(&mut state, chain, canonical_block)?;
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NodeConfig;
    use crate::mempool::Mempool;
    use crate::producer::produce_block;
    use vinx_crypto::{Address, KeyPair};
    use vinx_state::{create_genesis_state, GenesisConfig};

    /// Construit un état de genèse mono-validateur + une chaîne, et produit `n` blocs réels
    /// (donc avec des `state_root` valides), en avançant la finalité (n=1 → finalité immédiate).
    fn chain_with_blocks(n: u64) -> (WorldState, Chain, NodeConfig, WorldState) {
        let kp = KeyPair::generate();
        let addr = Address::from_public_key(&kp.public_key());
        let state = create_genesis_state(&GenesisConfig {
            admin_address: addr,
            validator_address: addr,
            chain_id: vinx_core::CHAIN_ID_DEVNET,
        });
        let (chain, genesis) = Chain::new_with_genesis(addr, 0);
        let _ = genesis;
        let config = NodeConfig::new(kp);
        let mut state = state;
        let mut chain = chain;
        let mut mempool = Mempool::default();
        // Snapshot de l'état finalisé au départ = état à la genèse (hauteur 0).
        let finalized_snapshot = state.clone();
        for i in 1..=n {
            produce_block(
                &mut state,
                &mut chain,
                &mut mempool,
                &config,
                &config.validator_set,
                i * 10,
            )
            .unwrap();
        }
        (state, chain, config, finalized_snapshot)
    }

    #[test]
    fn test_replay_reproduces_same_state_root() {
        // Rejouer la même branche depuis la genèse doit reproduire exactement l'état du tip
        // (identité) — valide toute la machinerie de rejeu (MTP par hauteur, settle, supply).
        let (mut live, chain, _config, genesis_state) = chain_with_blocks(4);
        let tip = chain.tip_height();
        let canonical = chain.get_block(tip).unwrap().clone();

        let mut rebuilt = rebuild_canonical_state(&genesis_state, 0, &chain, tip, &canonical)
            .expect("rejeu de la branche existante réussit");

        assert_eq!(
            rebuilt.compute_state_root(),
            live.compute_state_root(),
            "l'état reconstruit par rejeu doit être identique à l'état vivant"
        );
        assert_eq!(rebuilt.block_height, tip);
    }

    #[test]
    fn test_replay_rejects_tampered_state_root() {
        // Un bloc dont le state_root ne correspond pas à l'application réelle est refusé.
        let (_live, chain, _config, genesis_state) = chain_with_blocks(2);
        let tip = chain.tip_height();
        let mut bad = chain.get_block(tip).unwrap().clone();
        bad.header.state_root = [0xEE; 32]; // falsifié

        let err = rebuild_canonical_state(&genesis_state, 0, &chain, tip, &bad).unwrap_err();
        assert!(
            err.contains("state_root"),
            "un state_root falsifié doit faire échouer la reconstruction, obtenu: {err}"
        );
    }

    #[test]
    fn test_rebuild_refuses_below_finality() {
        let (_live, chain, _config, genesis_state) = chain_with_blocks(3);
        let canonical = chain.get_block(2).unwrap().clone();
        // finalité prétendue à 5, hauteur contestée 2 → refus (réorg sous la finalité).
        let err = rebuild_canonical_state(&genesis_state, 5, &chain, 2, &canonical).unwrap_err();
        assert!(err.contains("finalité"), "obtenu: {err}");
    }
}
