//! API credentials, with secret hygiene.
//!
//! [`Credentials`] is the single, uniform input across every venue: a key and a
//! secret, plus an optional passphrase (OKX, Bitget, KuCoin) and an optional
//! private key (Coinbase's per-request JWT). The signing scheme behind each
//! field is venue-specific, but the input shape is identical.
//!
//! Secret material is wiped from memory on drop (`zeroize`) and never appears in
//! a `Debug` rendering — the manual [`Debug`] impl redacts every field, so a
//! `Credentials` accidentally logged or embedded in an error reveals nothing.

use crate::error::{Error, Result};
use std::fmt;
use zeroize::ZeroizeOnDrop;

/// API credentials for one account on one venue.
///
/// Construct with [`Credentials::new`] and, where the venue requires it, add a
/// passphrase or private key. The secret fields are crate-private: exchange
/// modules read them to sign requests, but they are never exposed to consumers.
#[derive(Clone, ZeroizeOnDrop)]
pub struct Credentials {
    pub(crate) api_key: String,
    pub(crate) api_secret: String,
    pub(crate) passphrase: Option<String>,
    pub(crate) private_key: Option<String>,
}

impl Credentials {
    /// Build credentials from an API key and secret.
    pub fn new(api_key: impl Into<String>, api_secret: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            api_secret: api_secret.into(),
            passphrase: None,
            private_key: None,
        }
    }

    /// Attach the passphrase required by OKX, Bitget and KuCoin.
    #[must_use]
    pub fn with_passphrase(mut self, passphrase: impl Into<String>) -> Self {
        self.passphrase = Some(passphrase.into());
        self
    }

    /// Attach the private key used for Coinbase's per-request JWT signing.
    #[must_use]
    pub fn with_private_key(mut self, private_key: impl Into<String>) -> Self {
        self.private_key = Some(private_key.into());
        self
    }

    /// Validate that the non-secret invariants hold: a non-empty key and secret.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidCredentials`] if the key or secret is empty.
    pub fn validate(&self) -> Result<()> {
        if self.api_key.is_empty() {
            return Err(Error::InvalidCredentials("api key must not be empty"));
        }
        if self.api_secret.is_empty() {
            return Err(Error::InvalidCredentials("api secret must not be empty"));
        }
        Ok(())
    }

    /// Whether a passphrase is present (without revealing it).
    #[must_use]
    pub fn has_passphrase(&self) -> bool {
        self.passphrase.is_some()
    }

    /// Whether a private key is present (without revealing it).
    #[must_use]
    pub fn has_private_key(&self) -> bool {
        self.private_key.is_some()
    }
}

impl fmt::Debug for Credentials {
    /// Redacts every field. Presence of the optional fields is shown, their
    /// contents never are.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("api_key", &"<redacted>")
            .field("api_secret", &"<redacted>")
            .field(
                "passphrase",
                &self.passphrase.as_ref().map(|_| "<redacted>"),
            )
            .field(
                "private_key",
                &self.private_key.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_attaches_optional_fields() {
        let creds = Credentials::new("key", "secret");
        assert!(!creds.has_passphrase());
        assert!(!creds.has_private_key());

        let creds = creds.with_passphrase("pass").with_private_key("pem");
        assert!(creds.has_passphrase());
        assert!(creds.has_private_key());
    }

    #[test]
    fn validate_rejects_empty_key_or_secret() {
        assert!(Credentials::new("key", "secret").validate().is_ok());
        assert_eq!(
            Credentials::new("", "secret").validate().unwrap_err(),
            Error::InvalidCredentials("api key must not be empty")
        );
        assert_eq!(
            Credentials::new("key", "").validate().unwrap_err(),
            Error::InvalidCredentials("api secret must not be empty")
        );
    }

    #[test]
    fn debug_never_leaks_secret_material() {
        let creds = Credentials::new("AK_PUBLIC_VALUE", "SK_SECRET_VALUE")
            .with_passphrase("PASS_SECRET")
            .with_private_key("-----BEGIN EC PRIVATE KEY-----");
        let rendered = format!("{creds:?}");
        assert!(!rendered.contains("AK_PUBLIC_VALUE"));
        assert!(!rendered.contains("SK_SECRET_VALUE"));
        assert!(!rendered.contains("PASS_SECRET"));
        assert!(!rendered.contains("BEGIN EC PRIVATE KEY"));
        // Presence is still observable for diagnostics.
        assert!(rendered.contains("api_key"));
        assert!(rendered.contains("Some(\"<redacted>\")"));
    }
}
