//! Wire-format normalisation helpers.
//!
//! Venues send numbers as JSON strings (`"0.00010000"`) to avoid float loss;
//! mirroring that, the order layer parses them into exact [`Decimal`]s and
//! formats them back without scientific notation or spurious trailing zeros.
//! These helpers keep that conversion in one tested place.

use crate::error::{Error, Result};
use rust_decimal::Decimal;
use std::str::FromStr;

/// Parse a venue-supplied decimal string into an exact [`Decimal`].
///
/// # Errors
///
/// Returns [`Error::Deserialization`] if the string is not a valid decimal.
pub fn parse_decimal(s: &str) -> Result<Decimal> {
    Decimal::from_str(s.trim())
        .map_err(|e| Error::Deserialization(format!("invalid decimal {s:?}: {e}")))
}

/// Parse an optional decimal: `None`/empty/`"null"` map to `None`.
///
/// # Errors
///
/// Returns [`Error::Deserialization`] if a present value is not a valid decimal.
pub fn parse_opt_decimal(s: Option<&str>) -> Result<Option<Decimal>> {
    match s.map(str::trim) {
        None | Some("" | "null") => Ok(None),
        Some(value) => parse_decimal(value).map(Some),
    }
}

/// Format a [`Decimal`] for the wire: no scientific notation, trailing zeros
/// stripped (`1.2300` becomes `1.23`, `5.0` becomes `5`).
#[must_use]
pub fn format_decimal(value: Decimal) -> String {
    value.normalize().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn parses_clean_and_padded_strings() {
        assert_eq!(parse_decimal("0.00010000").unwrap(), dec!(0.0001));
        assert_eq!(parse_decimal("  42 ").unwrap(), dec!(42));
        assert_eq!(parse_decimal("-1.5").unwrap(), dec!(-1.5));
    }

    #[test]
    fn rejects_invalid_decimals() {
        assert!(matches!(
            parse_decimal("not-a-number").unwrap_err(),
            Error::Deserialization(_)
        ));
        assert!(parse_decimal("").is_err());
    }

    #[test]
    fn optional_decimal_handles_absent_and_null() {
        assert_eq!(parse_opt_decimal(None).unwrap(), None);
        assert_eq!(parse_opt_decimal(Some("")).unwrap(), None);
        assert_eq!(parse_opt_decimal(Some("null")).unwrap(), None);
        assert_eq!(parse_opt_decimal(Some("1.25")).unwrap(), Some(dec!(1.25)));
        assert!(parse_opt_decimal(Some("bad")).is_err());
    }

    #[test]
    fn formats_without_scientific_notation_or_trailing_zeros() {
        assert_eq!(format_decimal(dec!(1.2300)), "1.23");
        assert_eq!(format_decimal(dec!(5.0)), "5");
        assert_eq!(format_decimal(dec!(0.00010000)), "0.0001");
        assert_eq!(format_decimal(dec!(20000)), "20000");
    }
}
