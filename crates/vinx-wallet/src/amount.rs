//! Montants saisis et affichés en VINX, avec 9 décimales.

use anyhow::{Result, bail};
use vinx_core::{Amount, UNITS_PER_VINX};

const DECIMALS: usize = 9;

/// `"12.5"` → 12 500 000 000 unités. Refuse plus de 9 décimales, les signes et
/// tout dépassement.
pub fn parse_vinx(s: &str) -> Result<Amount> {
    let s = s.trim();
    let (whole, frac) = s.split_once('.').unwrap_or((s, ""));
    let digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (s.contains('.') && !digits(frac)) {
        bail!("montant invalide : « {s} » (exemple : 12.5)");
    }
    if frac.len() > DECIMALS {
        bail!("au plus {DECIMALS} décimales");
    }
    let whole: Amount = whole
        .parse()
        .map_err(|_| anyhow::anyhow!("montant trop grand"))?;
    let padded = format!("{frac:0<DECIMALS$}");
    let frac: Amount = padded
        .parse()
        .map_err(|_| anyhow::anyhow!("montant invalide"))?;
    whole
        .checked_mul(UNITS_PER_VINX)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| anyhow::anyhow!("montant trop grand"))
}

/// 12 500 000 000 unités → `"12.5"`.
pub fn format_vinx(units: Amount) -> String {
    let whole = units / UNITS_PER_VINX;
    let frac = units % UNITS_PER_VINX;
    if frac == 0 {
        return whole.to_string();
    }
    let frac = format!("{frac:0>DECIMALS$}");
    format!("{whole}.{}", frac.trim_end_matches('0'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format() {
        assert_eq!(parse_vinx("12.5").unwrap(), 12_500_000_000);
        assert_eq!(parse_vinx("0.000000001").unwrap(), 1);
        assert_eq!(parse_vinx("1000000000").unwrap(), 10u64.pow(18));
        assert_eq!(format_vinx(12_500_000_000), "12.5");
        assert_eq!(format_vinx(1), "0.000000001");
        assert_eq!(format_vinx(3 * UNITS_PER_VINX), "3");
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
