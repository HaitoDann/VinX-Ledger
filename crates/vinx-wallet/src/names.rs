//! Payment addresses `name@domain` (ADR 0085): resolved **off-chain**, like Lightning
//! Addresses or Stellar federation — nothing is stored on VinX.
//!
//! The wallet fetches `https://<domain>/.well-known/vinx.json?name=<name>`, which answers
//! `{"names": {"<name>": "vinx1…"}}`, shows the resolved address and pays **that address**.
//! Trust rests on the domain (the same as for e-mail); anyone can serve names for their
//! own domain.

use vinx_crypto::Address;

/// Splits `name@domain` into its parts; `None` for anything else (e.g. a `vinx1…` address).
pub fn parse_handle(s: &str) -> Option<(String, String)> {
    let (name, domain) = s.split_once('@')?;
    let name_ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
    let domain_ok = !domain.is_empty()
        && domain.len() <= 253
        && domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c))
        && !domain.starts_with('.')
        && !domain.contains("..");
    (name_ok && domain_ok).then(|| (name.to_string(), domain.to_ascii_lowercase()))
}

/// URL of the name document. HTTPS always, except for a local development domain.
pub fn well_known_url(name: &str, domain: &str) -> String {
    let local = domain.starts_with("localhost") || domain.starts_with("127.0.0.1");
    let scheme = if local { "http" } else { "https" };
    format!("{scheme}://{domain}/.well-known/vinx.json?name={name}")
}

/// Extracts `name`'s address from a name document.
pub fn address_from_doc(doc: &serde_json::Value, name: &str) -> Option<Address> {
    doc["names"][name].as_str()?.parse().ok()
}

/// Resolves `name@domain` to an address.
pub async fn resolve(handle: &str) -> Result<Address, String> {
    let (name, domain) = parse_handle(handle).ok_or("not a name@domain address")?;
    let url = well_known_url(&name, &domain);
    let doc: serde_json::Value = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("{domain} unreachable: {e}"))?
        .error_for_status()
        .map_err(|e| format!("{domain}: {e}"))?
        .json()
        .await
        .map_err(|e| format!("{domain}: invalid name document: {e}"))?;
    address_from_doc(&doc, &name).ok_or_else(|| format!("{handle} is not known by {domain}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles() {
        assert_eq!(
            parse_handle("julie@vinxpay.com"),
            Some(("julie".into(), "vinxpay.com".into()))
        );
        assert!(parse_handle("vinx1qqqq").is_none());
        assert!(parse_handle("Julie@x.com").is_none(), "lowercase only");
        assert!(parse_handle("jülie@x.com").is_none(), "ASCII only");
        assert!(parse_handle("a@").is_none());
        assert!(parse_handle("a@x..com").is_none());
        assert!(parse_handle("a@x.com/evil").is_none(), "no path injection");
        assert!(well_known_url("a", "x.com").starts_with("https://"));
    }

    #[test]
    fn documents() {
        let a = Address::from_bytes([7; 20]);
        let doc = serde_json::json!({ "names": { "julie": a.to_string() } });
        assert_eq!(address_from_doc(&doc, "julie"), Some(a));
        assert_eq!(address_from_doc(&doc, "bob"), None);
        let bad = serde_json::json!({ "names": { "julie": "not-an-address" } });
        assert_eq!(address_from_doc(&bad, "julie"), None);
    }
}
