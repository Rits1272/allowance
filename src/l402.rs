//! Reading an L402 challenge out of `WWW-Authenticate`.

#[derive(Debug, Clone, PartialEq)]
pub struct Challenge {
    /// The macaroon (or opaque token) to send back with the preimage.
    pub token: String,
    pub invoice: String,
}

/// Finds an L402 (or legacy LSAT) challenge among the header values. Servers
/// send `macaroon="…"` or the newer `token="…"`, each with `invoice="…"`.
pub fn parse(headers: &[String]) -> Option<Challenge> {
    for h in headers {
        let lower = h.to_ascii_lowercase();
        let Some(start) = lower.find("l402 ").or_else(|| lower.find("lsat ")) else {
            continue;
        };
        let params = &h[start + 5..];
        let token = field(params, "macaroon").or_else(|| field(params, "token"));
        let invoice = field(params, "invoice");
        if let (Some(token), Some(invoice)) = (token, invoice) {
            return Some(Challenge { token, invoice });
        }
    }
    None
}

fn field(params: &str, name: &str) -> Option<String> {
    let lower = params.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find(name) {
        let at = from + i;
        let boundary = at == 0 || matches!(lower.as_bytes()[at - 1], b' ' | b',');
        let after = &params[at + name.len()..];
        if boundary && after.starts_with('=') {
            let v = &after[1..];
            return Some(if let Some(q) = v.strip_prefix('"') {
                q.split('"').next().unwrap_or("").to_string()
            } else {
                v.split([',', ' ']).next().unwrap_or("").to_string()
            });
        }
        from = at + name.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn macaroon_form() {
        let c = parse(&[r#"L402 macaroon="AbC", invoice="lnbc210n1x""#.into()]).unwrap();
        assert_eq!(
            (c.token.as_str(), c.invoice.as_str()),
            ("AbC", "lnbc210n1x")
        );
    }

    #[test]
    fn token_form_and_other_schemes() {
        let hs = [
            r#"Bearer realm="x""#.to_string(),
            r#"L402 version="0", token="T", invoice="lnbc1x""#.to_string(),
        ];
        let c = parse(&hs).unwrap();
        assert_eq!((c.token.as_str(), c.invoice.as_str()), ("T", "lnbc1x"));
    }

    #[test]
    fn none_without_invoice() {
        assert!(parse(&[r#"L402 macaroon="AbC""#.into()]).is_none());
        assert!(parse(&[r#"Payment id="1", method="lightning""#.into()]).is_none());
    }
}
