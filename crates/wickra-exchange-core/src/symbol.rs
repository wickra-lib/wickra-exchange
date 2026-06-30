//! The unified [`Symbol`] type.
//!
//! Every venue spells the same market differently — `BTCUSDT` (Binance),
//! `BTC-USDT` (OKX), `XBT/USD` (Kraken). `Symbol` is the canonical, venue-neutral
//! form: a base asset traded against a quote asset. Each exchange module maps
//! `Symbol` to and from its own wire format; consumers only ever see the
//! canonical form, which is what makes the API typed rather than stringly.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// A canonical, venue-neutral market: a base asset against a quote asset.
///
/// Assets are upper-cased on construction so `Symbol` comparisons and hashing
/// are case-insensitive. The canonical text form is `BASE/QUOTE` (e.g.
/// `BTC/USDT`); [`FromStr`] also accepts a `-` separator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    base: String,
    quote: String,
}

impl Symbol {
    /// Build a symbol from a base and quote asset. Both are upper-cased.
    pub fn new(base: impl Into<String>, quote: impl Into<String>) -> Self {
        Self {
            base: base.into().to_ascii_uppercase(),
            quote: quote.into().to_ascii_uppercase(),
        }
    }

    /// The base asset (e.g. `BTC` in `BTC/USDT`).
    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The quote asset (e.g. `USDT` in `BTC/USDT`).
    #[must_use]
    pub fn quote(&self) -> &str {
        &self.quote
    }

    /// Render as a concatenated pair with no separator, e.g. `BTCUSDT`. This is
    /// the most common venue wire form; modules that need a different spelling
    /// (a separator, an alias such as `XBT`) map from the base/quote parts.
    #[must_use]
    pub fn to_concatenated(&self) -> String {
        format!("{}{}", self.base, self.quote)
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.base, self.quote)
    }
}

impl FromStr for Symbol {
    type Err = Error;

    /// Parse `BASE/QUOTE` or `BASE-QUOTE`. The separator is required because a
    /// bare concatenation (`BTCUSDT`) cannot be split unambiguously without the
    /// venue's asset list.
    fn from_str(s: &str) -> Result<Self> {
        let (base, quote) = s
            .split_once(['/', '-'])
            .ok_or_else(|| Error::InvalidSymbol(s.to_string()))?;
        if base.is_empty() || quote.is_empty() {
            return Err(Error::InvalidSymbol(s.to_string()));
        }
        Ok(Self::new(base, quote))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_upper_cases_assets() {
        let symbol = Symbol::new("btc", "usdt");
        assert_eq!(symbol.base(), "BTC");
        assert_eq!(symbol.quote(), "USDT");
    }

    #[test]
    fn display_is_canonical_slash_form() {
        assert_eq!(Symbol::new("BTC", "USDT").to_string(), "BTC/USDT");
    }

    #[test]
    fn concatenated_form_has_no_separator() {
        assert_eq!(Symbol::new("BTC", "USDT").to_concatenated(), "BTCUSDT");
    }

    #[test]
    fn parses_slash_and_dash_separators() {
        assert_eq!(
            "BTC/USDT".parse::<Symbol>().unwrap(),
            Symbol::new("BTC", "USDT")
        );
        assert_eq!(
            "eth-usd".parse::<Symbol>().unwrap(),
            Symbol::new("ETH", "USD")
        );
    }

    #[test]
    fn rejects_missing_or_empty_parts() {
        assert!("BTCUSDT".parse::<Symbol>().is_err());
        assert!("BTC/".parse::<Symbol>().is_err());
        assert!("/USDT".parse::<Symbol>().is_err());
    }

    #[test]
    fn round_trips_through_json() {
        let symbol = Symbol::new("BTC", "USDT");
        let json = serde_json::to_string(&symbol).unwrap();
        assert_eq!(serde_json::from_str::<Symbol>(&json).unwrap(), symbol);
    }
}
