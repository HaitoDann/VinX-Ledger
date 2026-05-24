use crate::error::WalletError;
use vinx_core::amount::{Amount, DECIMAL_FACTOR};

/// Parses a human-readable VINX amount string (e.g. "100", "99.50", "0.01")
/// into an internal Amount (10^18 atoms).
pub fn parse_amount(s: &str) -> Result<Amount, WalletError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(WalletError::InvalidAmount(s.to_string()));
    }

    let (whole_str, frac_str) = match s.split_once('.') {
        None => (s, ""),
        Some((w, f)) => (w, f),
    };

    let whole: u128 = whole_str
        .parse()
        .map_err(|_| WalletError::InvalidAmount(s.to_string()))?;

    let frac_atoms: u128 = if frac_str.is_empty() {
        0
    } else {
        // Truncate to 18 decimals, then right-pad to exactly 18 chars
        let truncated = &frac_str[..frac_str.len().min(18)];
        let padded = format!("{:0<18}", truncated);
        padded
            .parse()
            .map_err(|_| WalletError::InvalidAmount(s.to_string()))?
    };

    let whole_atoms = whole
        .checked_mul(DECIMAL_FACTOR)
        .ok_or_else(|| WalletError::InvalidAmount(format!("{} overflows u128", s)))?;

    let total = whole_atoms
        .checked_add(frac_atoms)
        .ok_or_else(|| WalletError::InvalidAmount(format!("{} overflows u128", s)))?;

    Ok(Amount::from_atoms(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_whole_number() {
        assert_eq!(parse_amount("100").unwrap().atoms(), 100 * DECIMAL_FACTOR);
    }

    #[test]
    fn test_parse_with_cents() {
        // 100.50 = 100 * 10^18 + 0.5 * 10^18
        let expected = 100 * DECIMAL_FACTOR + DECIMAL_FACTOR / 2;
        assert_eq!(parse_amount("100.50").unwrap().atoms(), expected);
    }

    #[test]
    fn test_parse_small() {
        // 0.01 = 10^16 atoms
        assert_eq!(parse_amount("0.01").unwrap().atoms(), DECIMAL_FACTOR / 100);
    }

    #[test]
    fn test_parse_one_decimal() {
        // "0.1" = 10^17 atoms
        assert_eq!(parse_amount("0.1").unwrap().atoms(), DECIMAL_FACTOR / 10);
    }

    #[test]
    fn test_parse_full_precision() {
        // 18 decimals
        let s = "1.000000000000000001";
        assert_eq!(parse_amount(s).unwrap().atoms(), DECIMAL_FACTOR + 1);
    }

    #[test]
    fn test_parse_truncates_excess_decimals() {
        // More than 18 decimal places — extra digits are dropped
        let s = "1.0000000000000000019"; // digit 19 ignored
        assert_eq!(parse_amount(s).unwrap().atoms(), DECIMAL_FACTOR + 1);
    }

    #[test]
    fn test_parse_zero() {
        assert_eq!(parse_amount("0").unwrap(), Amount::ZERO);
        assert_eq!(parse_amount("0.0").unwrap(), Amount::ZERO);
    }

    #[test]
    fn test_parse_invalid() {
        assert!(parse_amount("abc").is_err());
        assert!(parse_amount("").is_err());
        assert!(parse_amount("1.2.3").is_err());
    }
}
