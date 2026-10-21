//! Unités monétaires (SPEC §1, ADR 0001 §2).

/// Montant en unités : 1 VINX = 10⁹ unités.
pub type Amount = u64;

/// Nombre d'unités dans un VINX (9 décimales).
pub const UNITS_PER_VINX: Amount = 1_000_000_000;

/// Supply totale : 1 000 000 000 VINX, soit 10¹⁸ unités.
pub const TOTAL_SUPPLY: Amount = 1_000_000_000 * UNITS_PER_VINX;

// La supply doit tenir dans un u64 avec de la marge : vérifié à la compilation.
const _: () = assert!(TOTAL_SUPPLY < u64::MAX / 18);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supply_is_ten_to_the_eighteen() {
        assert_eq!(TOTAL_SUPPLY, 10u64.pow(18));
    }
}
