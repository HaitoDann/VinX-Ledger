//! Vérifications avant d'ajouter des validateurs (`vinx-wallet preflight`).
//!
//! Ajouter un validateur qui ne vote pas aussitôt peut arrêter la chaîne : avec
//! 2 ou 3 validateurs, tous doivent voter. Avant d'envoyer les `add-validator`,
//! on vérifie donc que chaque futur validateur est sur la bonne chaîne, à jour,
//! connecté, et qu'il a chargé sa clé.

use crate::client::Client;
use anyhow::Result;
use serde_json::Value;

/// Retard maximal toléré, en blocs, pour considérer un nœud à jour.
pub const MAX_LAG: u64 = 3;

/// Ce qu'on sait d'un nœud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeView {
    pub url: String,
    pub chain_id: String,
    pub height: u64,
    pub peers: u64,
    /// Clé de validateur chargée par le nœud.
    pub validator_key: Option<String>,
    /// Le nœud vote déjà.
    pub votes: bool,
    /// Hash de son dernier bloc.
    pub last_block_hash: Option<String>,
    /// Validateurs actifs vus par le nœud.
    pub validators: Vec<String>,
}

impl NodeView {
    pub fn from_status(url: &str, status: &Value) -> Self {
        let str_of = |k: &str| status.get(k).and_then(Value::as_str).map(str::to_owned);
        Self {
            url: url.to_owned(),
            chain_id: str_of("chain_id").unwrap_or_default(),
            height: status.get("height").and_then(Value::as_u64).unwrap_or(0),
            peers: status.get("peers").and_then(Value::as_u64).unwrap_or(0),
            validator_key: str_of("validator_key"),
            votes: status.get("validator_index").is_some_and(|v| !v.is_null()),
            last_block_hash: str_of("last_block_hash"),
            validators: status
                .get("validators")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Problèmes bloquants, par nœud.
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Clés à ajouter, dans l'ordre des nœuds.
    pub keys_to_add: Vec<String>,
    /// Nombre de validateurs après l'ajout.
    pub final_validators: usize,
}

/// Validateurs devant voter pour finaliser, sur `n` (SPEC §7.2).
pub fn quorum(n: usize) -> usize {
    (n.saturating_mul(2) / 3).saturating_add(1)
}

/// Évalue les futurs validateurs par rapport au nœud de référence.
///
/// `same_block` indique, pour chaque nœud, si son dernier bloc est identique à
/// celui de la référence à la même hauteur (`None` : impossible à vérifier).
pub fn evaluate(reference: &NodeView, nodes: &[(NodeView, Option<bool>)]) -> Report {
    let mut r = Report::default();
    for (n, same_block) in nodes {
        let who = &n.url;
        if n.chain_id != reference.chain_id {
            r.errors.push(format!(
                "{who} : chaîne « {} » au lieu de « {} »",
                n.chain_id, reference.chain_id
            ));
            continue;
        }
        if *same_block == Some(false) {
            r.errors.push(format!(
                "{who} : son bloc {} diffère de celui de la référence (autre genèse ?)",
                n.height
            ));
        }
        let lag = reference.height.saturating_sub(n.height);
        if lag > MAX_LAG {
            r.errors.push(format!(
                "{who} : en retard de {lag} blocs (hauteur {} contre {}) ; attendre qu'il rattrape",
                n.height, reference.height
            ));
        }
        if n.peers == 0 {
            r.errors.push(format!("{who} : aucun pair connecté"));
        }
        match &n.validator_key {
            None => r.errors.push(format!(
                "{who} : aucune clé de validateur chargée ; le démarrer avec --key"
            )),
            Some(k) if n.votes || reference.validators.contains(k) => r
                .warnings
                .push(format!("{who} : déjà validateur, rien à ajouter")),
            Some(k) if r.keys_to_add.contains(k) => r.errors.push(format!(
                "{who} : même clé qu'un autre nœud ; une clé ne doit tourner que sur un seul nœud"
            )),
            Some(k) => r.keys_to_add.push(k.clone()),
        }
    }
    r.final_validators = reference
        .validators
        .len()
        .saturating_add(r.keys_to_add.len());
    let n = r.final_validators;
    let tolerated = n.saturating_sub(quorum(n));
    if !r.keys_to_add.is_empty() && n > 1 && tolerated == 0 {
        r.warnings.push(format!(
            "avec {n} validateurs, tous doivent voter : la panne d'un seul arrête la chaîne \\
             (il en faut 4 pour en tolérer une)"
        ));
    }
    r
}

/// Interroge la référence et les nœuds, puis évalue.
pub fn run(reference: &Client, nodes: &[Client]) -> Result<(NodeView, Report)> {
    let reference_view = NodeView::from_status(reference.base_url(), &reference.status_json()?);
    let mut views = Vec::new();
    for client in nodes {
        let view = match client.status_json() {
            Ok(s) => NodeView::from_status(client.base_url(), &s),
            Err(e) => {
                let mut report = Report::default();
                report
                    .errors
                    .push(format!("{} : injoignable ({e})", client.base_url()));
                views.push((None, report));
                continue;
            }
        };
        let same_block = match (&view.last_block_hash, view.height) {
            (Some(hash), h) if h > 0 => reference.block_hash(h)?.map(|r| &r == hash),
            _ => None,
        };
        views.push((Some((view, same_block)), Report::default()));
    }
    let reachable: Vec<(NodeView, Option<bool>)> =
        views.iter().filter_map(|(v, _)| v.clone()).collect();
    let mut report = evaluate(&reference_view, &reachable);
    for (_, r) in views {
        report.errors.extend(r.errors);
    }
    Ok((reference_view, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(url: &str, height: u64, key: Option<&str>) -> NodeView {
        NodeView {
            url: url.into(),
            chain_id: "vinx-testnet-1".into(),
            height,
            peers: 1,
            validator_key: key.map(Into::into),
            votes: false,
            last_block_hash: Some("h".into()),
            validators: vec!["01v1".into()],
        }
    }

    #[test]
    fn three_ready_nodes_give_four_validators() {
        let reference = view("ref", 100, Some("01v1"));
        let nodes: Vec<_> = ["01a", "01b", "01c"]
            .iter()
            .enumerate()
            .map(|(i, k)| (view(&format!("n{i}"), 99, Some(k)), Some(true)))
            .collect();
        let r = evaluate(&reference, &nodes);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.keys_to_add, ["01a", "01b", "01c"]);
        assert_eq!(r.final_validators, 4);
    }

    #[test]
    fn adding_one_at_a_time_is_flagged() {
        let reference = view("ref", 100, Some("01v1"));
        let r = evaluate(&reference, &[(view("n", 100, Some("01a")), Some(true))]);
        assert!(r.errors.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("tous doivent voter")));
    }

    #[test]
    fn problems_are_reported() {
        let reference = view("ref", 100, Some("01v1"));
        let mut other_chain = view("chaine", 100, Some("01a"));
        other_chain.chain_id = "vinx-devnet-1".into();
        let mut isolated = view("isole", 100, Some("01c"));
        isolated.peers = 0;
        let nodes = vec![
            (other_chain, None),
            (view("retard", 50, Some("01b")), Some(true)),
            (isolated, Some(true)),
            (view("sans-cle", 100, None), Some(true)),
            (view("fourche", 100, Some("01d")), Some(false)),
            (view("doublon", 100, Some("01d")), Some(true)),
        ];
        let r = evaluate(&reference, &nodes);
        for needle in [
            "chaîne",
            "en retard",
            "aucun pair",
            "aucune clé",
            "diffère",
            "même clé",
        ] {
            assert!(
                r.errors.iter().any(|e| e.contains(needle)),
                "manque : {needle}"
            );
        }
    }
}
