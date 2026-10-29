//! Portefeuille mobile : phrase de récupération, dérivation des comptes et
//! signature des transactions, compilés en WebAssembly pour l'application
//! Android. Les types et l'encodage sont ceux de la chaîne (`vinx-core`) : une
//! transaction signée ici est octet pour octet celle du portefeuille de bureau.
//!
//! Dérivation : `clé du compte i = derive_key(DERIVATION, graine BIP-39 ‖ i)`,
//! la même que celle du portefeuille de bureau : les 12 mots donnent les mêmes
//! comptes sur le téléphone et sur l'ordinateur.

use serde::Deserialize;
use vinx_core::{Transaction, TxBody, encode};
use vinx_crypto::{Address, KeyPair};
use wasm_bindgen::prelude::*;

/// Contexte de dérivation des comptes, commun à tous les portefeuilles.
pub const DERIVATION: &str = "VinX Ledger portefeuille v1 compte ed25519";

/// Nouvelle phrase de 12 mots, tirée du générateur aléatoire du système.
pub fn new_phrase() -> Result<String, String> {
    let mut entropy = [0u8; 16];
    getrandom::getrandom(&mut entropy).map_err(|e| format!("aléa indisponible : {e}"))?;
    bip39::Mnemonic::from_entropy(&entropy)
        .map(|m| m.to_string())
        .map_err(|e| e.to_string())
}

/// Graine d'une phrase de 12 mots ; refuse une phrase mal recopiée.
pub fn seed_of(words: &str) -> Result<[u8; 64], String> {
    let normalized = words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mnemonic = bip39::Mnemonic::parse_normalized(&normalized)
        .map_err(|_| "phrase invalide : vérifie l'orthographe et l'ordre des 12 mots".to_owned())?;
    if mnemonic.word_count() != 12 {
        return Err("la phrase doit compter 12 mots".into());
    }
    Ok(mnemonic.to_seed(""))
}

/// Clé du compte `index` dérivée de la graine.
pub fn derive_account(seed: &[u8; 64], index: u32) -> KeyPair {
    let mut input = seed.to_vec();
    input.extend_from_slice(&index.to_le_bytes());
    KeyPair::from_secret_bytes(&blake3::derive_key(DERIVATION, &input))
}

fn seed_from_hex(seed: &str) -> Result<[u8; 64], String> {
    hex::decode(seed.trim())
        .ok()
        .and_then(|b| <[u8; 64]>::try_from(b).ok())
        .ok_or_else(|| "graine invalide".to_owned())
}

/// Montant saisi (`"12,5"` ou `"12.5"`) → unités, pour `decimals` décimales.
pub fn parse_units(s: &str, decimals: u32) -> Result<u64, String> {
    let s = s.trim().replace(',', ".");
    let (whole, frac) = s.split_once('.').unwrap_or((&s, ""));
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (s.contains('.') && !digits(frac)) {
        return Err(format!("montant invalide : « {s} »"));
    }
    let places = usize::try_from(decimals).map_err(|_| "décimales invalides".to_owned())?;
    if frac.len() > places {
        return Err(format!("au plus {decimals} décimales"));
    }
    let scale = 10u64
        .checked_pow(decimals)
        .ok_or_else(|| "trop de décimales".to_owned())?;
    let whole: u64 = whole.parse().map_err(|_| "montant trop grand".to_owned())?;
    let frac: u64 = if places == 0 {
        0
    } else {
        format!("{frac:0<places$}")
            .parse()
            .map_err(|_| "montant invalide".to_owned())?
    };
    whole
        .checked_mul(scale)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| "montant trop grand".to_owned())
}

/// Corps des transactions que le portefeuille mobile sait signer.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Body {
    Transfer {
        to: String,
        amount: String,
        #[serde(default)]
        memo: String,
    },
    TransferAsset {
        symbol: String,
        to: String,
        amount: String,
        #[serde(default)]
        memo: String,
    },
    AcceptAsset {
        symbol: String,
    },
}

#[derive(Deserialize)]
struct Request {
    chain_id: String,
    nonce: u64,
    /// Frais, en unités.
    fee: String,
    /// Première transaction du compte : la clé publique accompagne la signature.
    include_key: bool,
    body: Body,
}

fn memo(m: &str) -> Option<Vec<u8>> {
    let m = m.trim();
    (!m.is_empty()).then(|| m.as_bytes().to_vec())
}

fn address(s: &str) -> Result<Address, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("adresse invalide : {s}"))
}

fn units(s: &str) -> Result<u64, String> {
    s.trim()
        .parse()
        .map_err(|_| format!("montant en unités invalide : {s}"))
}

/// Signe la transaction décrite par `request` (JSON) pour le compte `index` :
/// renvoie `{ "tx": hex de la transaction signée, "id": identifiant }`.
pub fn sign_request(seed: &[u8; 64], index: u32, request: &str) -> Result<String, String> {
    let r: Request =
        serde_json::from_str(request).map_err(|e| format!("requête invalide : {e}"))?;
    let keypair = derive_account(seed, index);
    let body = match r.body {
        Body::Transfer {
            to,
            amount,
            memo: m,
        } => TxBody::Transfer {
            to: address(&to)?,
            amount: units(&amount)?,
            memo: memo(&m),
        },
        Body::TransferAsset {
            symbol,
            to,
            amount,
            memo: m,
        } => TxBody::TransferAsset {
            symbol: symbol.trim().to_ascii_uppercase(),
            to: address(&to)?,
            amount: units(&amount)?,
            memo: memo(&m),
        },
        Body::AcceptAsset { symbol } => TxBody::AcceptAsset {
            symbol: symbol.trim().to_ascii_uppercase(),
            accept: true,
        },
    };
    let tx = Transaction {
        chain_id: r.chain_id,
        sender: Address::from_public_key(&keypair.public_key()),
        nonce: r.nonce,
        fee: units(&r.fee)?,
        body,
    };
    let signed = tx.sign(&keypair, r.include_key);
    Ok(serde_json::json!({
        "tx": hex::encode(encode(&signed)),
        "id": hex::encode(signed.id()),
    })
    .to_string())
}

fn js(e: String) -> JsError {
    JsError::new(&e)
}

/// Nouvelle phrase de récupération de 12 mots.
#[wasm_bindgen]
pub fn phrase() -> Result<String, JsError> {
    new_phrase().map_err(js)
}

/// Graine (hexadécimal) d'une phrase de 12 mots.
#[wasm_bindgen]
pub fn seed(words: &str) -> Result<String, JsError> {
    seed_of(words).map(hex::encode).map_err(js)
}

/// Adresse `vinx1…` du compte `index`.
#[wasm_bindgen]
pub fn account_address(seed: &str, index: u32) -> Result<String, JsError> {
    let seed = seed_from_hex(seed).map_err(js)?;
    Ok(Address::from_public_key(&derive_account(&seed, index).public_key()).to_string())
}

/// Signe une transaction : voir [`sign_request`].
#[wasm_bindgen]
pub fn sign(seed: &str, index: u32, request: &str) -> Result<String, JsError> {
    let seed = seed_from_hex(seed).map_err(js)?;
    sign_request(&seed, index, request).map_err(js)
}

/// Montant saisi → unités (chaîne : les entiers de plus de 2⁵³ perdent leur
/// précision en JavaScript).
#[wasm_bindgen]
pub fn to_units(amount: &str, decimals: u32) -> Result<String, JsError> {
    parse_units(amount, decimals)
        .map(|u| u.to_string())
        .map_err(js)
}

/// L'adresse est-elle valide (format et somme de contrôle) ?
#[wasm_bindgen]
pub fn valid_address(s: &str) -> bool {
    address(s).is_ok()
}

/// QR code de `text`, en SVG.
#[wasm_bindgen]
pub fn qr_svg(text: &str) -> Result<String, JsError> {
    let code = qrcode::QrCode::new(text.as_bytes()).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build())
}

/// Cherche un QR code dans une image RGBA (`width × height`, 4 octets par
/// pixel, comme `ImageData` d'un canvas) et renvoie son texte, ou `undefined`.
#[wasm_bindgen]
pub fn scan_qr(width: u32, height: u32, rgba: &[u8]) -> Option<String> {
    let (w, h) = (usize::try_from(width).ok()?, usize::try_from(height).ok()?);
    if rgba.len() < w.checked_mul(h)?.checked_mul(4)? {
        return None;
    }
    let mut img = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let i = y.saturating_mul(w).saturating_add(x).saturating_mul(4);
        let px = |k: usize| rgba.get(i.saturating_add(k)).copied().map_or(0, u16::from);
        // Luminance approchée (0,30 R + 0,59 G + 0,11 B).
        let luma = px(0)
            .saturating_mul(77)
            .saturating_add(px(1).saturating_mul(150))
            .saturating_add(px(2).saturating_mul(29))
            >> 8;
        u8::try_from(luma).unwrap_or(u8::MAX)
    });
    img.detect_grids()
        .into_iter()
        .find_map(|g| g.decode().ok().map(|(_, text)| text))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)] // code de test
mod tests {
    use super::*;
    use vinx_core::{SignedTransaction, decode};

    const WORDS: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    #[test]
    fn phrase_round_trips_and_typos_are_refused() {
        let p = new_phrase().unwrap();
        assert_eq!(p.split_whitespace().count(), 12);
        assert!(seed_of(&p).is_ok());
        assert!(seed_of("abandon abandon").is_err());
        assert!(seed_of(&WORDS.replace("about", "abort")).is_err());
        // Casse et espaces n'importent pas.
        assert_eq!(
            seed_of(WORDS).unwrap(),
            seed_of(&format!("  {}  ", WORDS.to_uppercase())).unwrap()
        );
    }

    #[test]
    fn signed_transfer_decodes_and_verifies() {
        let seed = seed_of(WORDS).unwrap();
        let kp = derive_account(&seed, 1);
        let to = Address::from_public_key(&derive_account(&seed, 2).public_key());
        let req = serde_json::json!({
            "chain_id": "vinx-testnet-2", "nonce": 1, "fee": "1000000", "include_key": true,
            "body": { "kind": "transfer", "to": to.to_string(), "amount": "2500000000", "memo": "FAC-1" },
        });
        let out: serde_json::Value =
            serde_json::from_str(&sign_request(&seed, 1, &req.to_string()).unwrap()).unwrap();
        let bytes = hex::decode(out["tx"].as_str().unwrap()).unwrap();
        let stx: SignedTransaction = decode(&bytes).unwrap();
        assert_eq!(stx.tx.sender, Address::from_public_key(&kp.public_key()));
        assert_eq!(stx.public_key, Some(kp.public_key()));
        assert!(stx.verify_signature(&kp.public_key()).is_ok());
        assert_eq!(out["id"].as_str().unwrap(), hex::encode(stx.id()));
        match stx.tx.body {
            TxBody::Transfer {
                to: t,
                amount,
                memo,
            } => {
                assert_eq!(
                    (t, amount, memo),
                    (to, 2_500_000_000, Some(b"FAC-1".to_vec()))
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn asset_bodies_and_bad_input() {
        let seed = seed_of(WORDS).unwrap();
        let req = |body: serde_json::Value| {
            serde_json::json!({ "chain_id": "c", "nonce": 3, "fee": "1", "include_key": false, "body": body })
                .to_string()
        };
        let accept = sign_request(
            &seed,
            0,
            &req(serde_json::json!({ "kind": "accept_asset", "symbol": "eurv" })),
        );
        let v: serde_json::Value = serde_json::from_str(&accept.unwrap()).unwrap();
        let stx: SignedTransaction =
            decode(&hex::decode(v["tx"].as_str().unwrap()).unwrap()).unwrap();
        assert_eq!(
            stx.tx.body,
            TxBody::AcceptAsset {
                symbol: "EURV".into(),
                accept: true
            }
        );
        assert!(stx.public_key.is_none());
        let bad = req(serde_json::json!({ "kind": "transfer", "to": "vinx1abc", "amount": "1" }));
        assert!(
            sign_request(&seed, 0, &bad)
                .unwrap_err()
                .contains("adresse invalide")
        );
        assert!(sign_request(&seed, 0, "{}").is_err());
    }

    #[test]
    fn qr_round_trip() {
        let link = "vinx:boulangerie.vinx?amount=4.5&memo=FAC-7";
        let code = qrcode::QrCode::new(link.as_bytes()).unwrap();
        let module = 4usize;
        let n = code.width();
        let size = (n + 8) * module;
        let mut rgba = vec![255u8; size * size * 4];
        for y in 0..n {
            for x in 0..n {
                if code[(x, y)] == qrcode::Color::Dark {
                    for dy in 0..module {
                        for dx in 0..module {
                            let (px, py) = ((x + 4) * module + dx, (y + 4) * module + dy);
                            let i = (py * size + px) * 4;
                            rgba[i..i + 3].copy_from_slice(&[0, 0, 0]);
                        }
                    }
                }
            }
        }
        let s = u32::try_from(size).unwrap();
        assert_eq!(scan_qr(s, s, &rgba).as_deref(), Some(link));
        assert_eq!(scan_qr(s, s, &vec![255u8; size * size * 4]), None);
        assert_eq!(scan_qr(10, 10, &[0; 3]), None);
    }

    #[test]
    fn amounts() {
        assert_eq!(parse_units("12,5", 9).unwrap(), 12_500_000_000);
        assert_eq!(parse_units("12.5", 2).unwrap(), 1_250);
        assert_eq!(parse_units("7", 0).unwrap(), 7);
        for bad in ["", "1.2.3", "-1", "1e3", "1.234", "99999999999999999999"] {
            assert!(parse_units(bad, 2).is_err(), "{bad}");
        }
    }
}
