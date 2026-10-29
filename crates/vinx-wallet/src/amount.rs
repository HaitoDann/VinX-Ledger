//! Montants saisis et affichés en VINX, avec 9 décimales.

use anyhow::{Result, bail};
use vinx_core::Amount;

const DECIMALS: usize = 9;

/// `"12.5"` → 12 500 000 000 unités. Refuse plus de 9 décimales, les signes et
/// tout dépassement.
pub fn parse_vinx(s: &str) -> Result<Amount> {
    parse_units(s, DECIMALS)
}

/// 12 500 000 000 unités → `"12.5"`.
pub fn format_vinx(units: Amount) -> String {
    format_units(units, DECIMALS)
}

/// Montant d'un actif à `decimals` décimales : `"12.5"` → 1 250 pour 2 décimales.
pub fn parse_units(s: &str, decimals: usize) -> Result<Amount> {
    let s = s.trim();
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (s.contains('.') && !digits(frac)) {
        bail!("montant invalide : « {s} » (exemple : 12.5)");
    }
    if frac.len() > decimals {
        bail!("au plus {decimals} décimales");
    }
    let whole: Amount = whole
        .parse()
        .map_err(|_| anyhow::anyhow!("montant trop grand"))?;
    let padded = format!("{frac:0<decimals$}");
    let frac: Amount = if padded.is_empty() {
        0
    } else {
        padded
            .parse()
            .map_err(|_| anyhow::anyhow!("montant invalide"))?
    };
    whole
        .checked_mul(scale(decimals)?)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| anyhow::anyhow!("montant trop grand"))
}

/// Unités d'un actif à `decimals` décimales → `"12.5"`.
pub fn format_units(units: Amount, decimals: usize) -> String {
    let Ok(one) = scale(decimals) else {
        return units.to_string();
    };
    let whole = units.checked_div(one).unwrap_or(0);
    let frac = units.checked_rem(one).unwrap_or(0);
    if frac == 0 {
        return whole.to_string();
    }
    let frac = format!("{frac:0>decimals$}");
    format!("{whole}.{}", frac.trim_end_matches('0'))
}

fn scale(decimals: usize) -> Result<Amount> {
    u32::try_from(decimals)
        .ok()
        .and_then(|d| 10u64.checked_pow(d))
        .ok_or_else(|| anyhow::anyhow!("trop de décimales"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::UNITS_PER_VINX;

    #[test]
    fn parse_and_format() {
        assert_eq!(parse_vinx("12.5").unwrap(), 12_500_000_000);
        assert_eq!(parse_vinx("0.000000001").unwrap(), 1);
        assert_eq!(parse_vinx("1000000000").unwrap(), 10u64.pow(18));
        assert_eq!(format_vinx(12_500_000_000), "12.5");
        assert_eq!(format_vinx(1), "0.000000001");
        assert_eq!(format_vinx(3 * UNITS_PER_VINX), "3");
        assert_eq!(parse_units("12.5", 2).unwrap(), 1_250);
        assert_eq!(parse_units("7", 0).unwrap(), 7);
        assert!(parse_units("7.1", 0).is_err());
        assert_eq!(format_units(1_250, 2), "12.5");
        assert_eq!(format_units(7, 0), "7");
        for bad in [
            "",
            ".5",
            "5.",
            "-1",
            "1e3",
            "1.0000000001",
            "1,5",
            "99999999999999999999",
        ] {
            assert!(parse_vinx(bad).is_err(), "{bad}");
        }
    }
}
