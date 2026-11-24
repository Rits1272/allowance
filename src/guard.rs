//! The one path money takes: request, 402, check the invoice against the
//! policy, pay, retry with proof, record.

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::ledger::{now, Entry, Ledger};
use crate::policy::Policy;
use crate::wallet::Wallet;
use crate::{bolt11, l402};

const MAX_BODY: usize = 200_000;

pub struct Guard {
    pub policy: Policy,
    pub ledger: Mutex<Ledger>,
    pub wallet: Box<dyn Wallet>,
    pub http: reqwest::Client,
}

pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// What to call this in the ledger (a tool name, or the method).
    pub what: String,
}

struct Response {
    status: u16,
    challenges: Vec<String>,
    body: String,
}

impl Guard {
    async fn send(&self, req: &Request, auth: Option<&str>) -> Result<Response, String> {
        let method = reqwest::Method::from_bytes(req.method.to_ascii_uppercase().as_bytes())
            .map_err(|_| "bad HTTP method")?;
        let mut b = self.http.request(method, &req.url);
        for (k, v) in &req.headers {
            // The guard alone decides what goes in Authorization.
            if !k.eq_ignore_ascii_case("authorization") {
                b = b.header(k, v);
            }
        }
        if let Some(a) = auth {
            b = b.header("Authorization", a);
        }
        if let Some(body) = &req.body {
            b = b.body(body.clone());
        }
        let resp = b.send().await.map_err(|e| format!("request failed: {e}"))?;
        let status = resp.status().as_u16();
        let challenges = resp
            .headers()
            .get_all("www-authenticate")
            .iter()
            .filter_map(|v| v.to_str().ok().map(str::to_string))
            .collect();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("reading the answer failed: {e}"))?;
        let mut body = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_BODY)]).into_owned();
        if bytes.len() > MAX_BODY {
            body.push_str("\n[truncated]");
        }
        Ok(Response {
            status,
            challenges,
            body,
        })
    }

    fn host(url: &str) -> Result<String, String> {
        let u = reqwest::Url::parse(url).map_err(|_| format!("not a URL: {url}"))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err("only http and https URLs can be fetched".into());
        }
        u.host_str()
            .map(|h| h.to_ascii_lowercase())
            .ok_or_else(|| "URL has no host".into())
    }

    /// What a request would cost and whether the policy would allow it.
    /// Nothing is paid and nothing is reserved.
    pub async fn quote(&self, req: Request) -> Result<Value, String> {
        let host = Self::host(&req.url)?;
        let first = self.send(&req, None).await?;
        if first.status != 402 {
            return Ok(json!({ "payment_required": false, "status": first.status }));
        }
        let Some(ch) = l402::parse(&first.challenges) else {
            return Ok(json!({ "payment_required": true, "payable": false,
                "reason": "the server asks for payment in a way this wallet can't pay yet (only L402 over Lightning)" }));
        };
        let Some(amount_msat) = bolt11::amount_msat(&ch.invoice)? else {
            return Ok(
                json!({ "payment_required": true, "payable": false, "reason": "the invoice names no amount" }),
            );
        };
        let cost = amount_msat + self.policy.max_fee_sat * 1000;
        let verdict = self
            .policy
            .check(&host, cost, self.ledger.lock().await.spent_today(&host));
        Ok(json!({
            "payment_required": true,
            "payable": true,
            "host": host,
            "price_sat": amount_msat.div_ceil(1000),
            "allowed": verdict.is_ok(),
            "reason": verdict.err(),
        }))
    }

    /// Fetches `req`, paying an L402 challenge if the policy allows it.
    pub async fn fetch(&self, req: Request) -> Result<Value, String> {
        let host = Self::host(&req.url)?;
        let first = self.send(&req, None).await?;
        if first.status != 402 {
            return Ok(json!({ "status": first.status, "paid_sat": 0, "body": first.body }));
        }
        let Some(ch) = l402::parse(&first.challenges) else {
            return Ok(json!({ "status": 402, "paid_sat": 0, "paid": false,
                "reason": "the server asks for payment in a way this wallet can't pay yet (only L402 over Lightning)",
                "body": first.body }));
        };
        let entry = |status: &str, amount_msat: u64| Entry {
            id: String::new(),
            at: now(),
            host: host.clone(),
            url: req.url.clone(),
            what: req.what.clone(),
            scheme: "L402".into(),
            amount_msat,
            fee_msat: 0,
            status: status.into(),
            reason: String::new(),
            preimage: String::new(),
            served: 0,
        };
        let refused = |reason: String, id: String, amount_msat: u64| {
            json!({ "status": 402, "paid": false, "paid_sat": 0, "price_sat": amount_msat.div_ceil(1000),
                "reason": reason, "receipt": id,
                "note": "Not paid. Limits are set by the wallet's owner and can't be changed from here; tell them if this payment matters." })
        };

        // The limit is checked against the invoice itself, never the server's word.
        let amount_msat = match bolt11::amount_msat(&ch.invoice) {
            Ok(Some(a)) => a,
            Ok(None) | Err(_) => {
                let reason = "the invoice names no readable amount, so its cost can't be checked"
                    .to_string();
                let id = self.ledger.lock().await.deny(Entry {
                    reason: reason.clone(),
                    ..entry("denied", 0)
                })?;
                return Ok(refused(reason, id, 0));
            }
        };
        let cost = amount_msat + self.policy.max_fee_sat * 1000;
        let id = {
            let mut ledger = self.ledger.lock().await;
            match ledger.authorize(&self.policy, &host, cost) {
                Ok(id) => id,
                Err(reason) => {
                    let id = ledger.deny(Entry {
                        reason: reason.clone(),
                        ..entry("denied", amount_msat)
                    })?;
                    return Ok(refused(reason, id, amount_msat));
                }
            }
        };

        let paid = match self.wallet.pay(&ch.invoice, self.policy.max_fee_sat).await {
            Ok(p) => p,
            Err(e) => {
                let status = if e.definite { "failed" } else { "unknown" };
                self.ledger.lock().await.settle(Entry {
                    id: id.clone(),
                    reason: e.message.clone(),
                    ..entry(status, amount_msat)
                })?;
                return Ok(
                    json!({ "status": 402, "paid": false, "paid_sat": 0, "receipt": id,
                    "reason": if e.definite { e.message } else { format!("{} (outcome unknown: the amount stays counted against the budget)", e.message) } }),
                );
            }
        };

        // Paid: the retry must not be able to lose the record.
        let auth = format!("L402 {}:{}", ch.token, paid.preimage_hex);
        let second = self.send(&req, Some(&auth)).await;
        let served = second.as_ref().map(|r| r.status).unwrap_or(0);
        self.ledger.lock().await.settle(Entry {
            id: id.clone(),
            fee_msat: paid.fee_msat,
            preimage: paid.preimage_hex,
            served,
            ..entry("paid", amount_msat)
        })?;
        let paid_sat = (amount_msat + paid.fee_msat).div_ceil(1000);
        match second {
            Ok(r) => Ok(
                json!({ "status": r.status, "paid": true, "paid_sat": paid_sat, "receipt": id, "body": r.body }),
            ),
            Err(e) => Ok(
                json!({ "status": 0, "paid": true, "paid_sat": paid_sat, "receipt": id,
                "reason": format!("paid, but the request after payment failed: {e}. The wallet's owner has the proof of payment.") }),
            ),
        }
    }

    pub async fn status(&self) -> Value {
        let ledger = self.ledger.lock().await;
        let spent = ledger.spent_today("");
        let recent: Vec<Value> = ledger
            .recent(8)
            .into_iter()
            .map(|e| {
                json!({ "receipt": e.id, "host": e.host, "what": e.what, "status": e.status,
                "sat": (e.amount_msat + e.fee_msat).div_ceil(1000), "reason": e.reason })
            })
            .collect();
        json!({
            "wallet": self.wallet.name(),
            "per_payment_sat": self.policy.per_payment_sat,
            "per_day_sat": self.policy.per_day_sat,
            "spent_today_sat": spent.total_msat.div_ceil(1000),
            "left_today_sat": (self.policy.per_day_sat * 1000).saturating_sub(spent.total_msat) / 1000,
            "hosts": self.policy.host.iter().map(|h| json!({ "name": h.name, "per_payment_sat": h.per_payment_sat, "per_day_sat": h.per_day_sat })).collect::<Vec<_>>(),
            "unlisted_hosts": format!("{:?}", self.policy.unlisted_hosts).to_lowercase(),
            "recent": recent,
        })
    }
}

#[cfg(test)]
mod tests {
    //! The whole flow against a local paywall and a wallet that pays nothing.

    use super::*;
    use crate::wallet::{Paid, PayError};
    use async_trait::async_trait;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    const PREIMAGE: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    // 21 sats: only the amount in the prefix is read.
    const INVOICE: &str = "lnbc210n1ptestinvoice";

    struct MockWallet {
        paid: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Wallet for MockWallet {
        fn name(&self) -> &'static str {
            "mock"
        }
        async fn pay(&self, _invoice: &str, _max_fee_sat: u64) -> Result<Paid, PayError> {
            self.paid.fetch_add(1, Ordering::SeqCst);
            Ok(Paid {
                preimage_hex: PREIMAGE.into(),
                fee_msat: 1_000,
            })
        }
    }

    async fn paywall(headers: HeaderMap) -> (StatusCode, HeaderMap, &'static str) {
        let mut out = HeaderMap::new();
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if auth == format!("L402 tok:{PREIMAGE}") {
            return (StatusCode::OK, out, "the paid answer");
        }
        let challenge = format!(r#"L402 macaroon="tok", invoice="{INVOICE}""#);
        out.insert("www-authenticate", challenge.parse().unwrap());
        (StatusCode::PAYMENT_REQUIRED, out, "pay first")
    }

    /// A paywalled server on a free port, and a guard with `policy`.
    async fn setup(policy: &str) -> (Guard, String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://127.0.0.1:{}/paid",
            listener.local_addr().unwrap().port()
        );
        tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/paid", get(paywall))).await
        });
        let paid = Arc::new(AtomicUsize::new(0));
        let ledger = std::env::temp_dir().join(format!(
            "allowance-test-{}-{}.jsonl",
            std::process::id(),
            url.len() + paid.as_ref() as *const _ as usize
        ));
        let _ = std::fs::remove_file(&ledger);
        let guard = Guard {
            policy: toml::from_str(policy).unwrap(),
            ledger: Mutex::new(Ledger::open(ledger.to_str().unwrap()).unwrap()),
            wallet: Box::new(MockWallet { paid: paid.clone() }),
            http: reqwest::Client::new(),
        };
        (guard, url, paid)
    }

    fn get_req(url: &str) -> Request {
        Request {
            method: "GET".into(),
            url: url.into(),
            headers: vec![],
            body: None,
            what: "GET".into(),
        }
    }

    const ALLOW_LOCAL: &str = r#"
        per_payment_sat = 50
        per_day_sat = 60
        max_fee_sat = 2
        [[host]]
        name = "127.0.0.1"
    "#;

    #[tokio::test]
    async fn pays_within_limits_and_returns_the_paid_answer() {
        let (guard, url, paid) = setup(ALLOW_LOCAL).await;
        let out = guard.fetch(get_req(&url)).await.unwrap();
        assert_eq!(out["status"], 200);
        assert_eq!(out["paid"], true);
        assert_eq!(out["paid_sat"], 22); // 21 + 1 sat fee
        assert_eq!(out["body"], "the paid answer");
        assert_eq!(paid.load(Ordering::SeqCst), 1);
        // The agent's view carries no credential.
        let seen = out.to_string();
        assert!(!seen.contains(PREIMAGE) && !seen.contains("tok"));
    }

    #[tokio::test]
    async fn refuses_once_the_day_is_spent_without_paying() {
        let (guard, url, paid) = setup(ALLOW_LOCAL).await;
        // 22 + 22 = 44 spent; a third needs 21 + 2 reserved = 67 > 60.
        guard.fetch(get_req(&url)).await.unwrap();
        guard.fetch(get_req(&url)).await.unwrap();
        let out = guard.fetch(get_req(&url)).await.unwrap();
        assert_eq!(out["paid"], false);
        assert!(out["reason"].as_str().unwrap().contains("budget"));
        assert_eq!(paid.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refuses_an_unlisted_host() {
        let (guard, url, paid) = setup("per_payment_sat = 50\nper_day_sat = 60").await;
        let out = guard.fetch(get_req(&url)).await.unwrap();
        assert_eq!(out["paid"], false);
        assert!(out["reason"].as_str().unwrap().contains("not on the list"));
        assert_eq!(paid.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn refuses_a_price_over_the_per_payment_limit() {
        let (guard, url, paid) =
            setup("per_payment_sat = 20\nper_day_sat = 500\nunlisted_hosts = \"allow\"").await;
        let out = guard.fetch(get_req(&url)).await.unwrap();
        assert_eq!(out["paid"], false);
        assert_eq!(paid.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn quote_pays_nothing() {
        let (guard, url, paid) = setup(ALLOW_LOCAL).await;
        let q = guard.quote(get_req(&url)).await.unwrap();
        assert_eq!(q["price_sat"], 21);
        assert_eq!(q["allowed"], true);
        assert_eq!(paid.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn concurrent_calls_cannot_overshoot_the_budget() {
        let (guard, url, paid) = setup(ALLOW_LOCAL).await;
        let guard = Arc::new(guard);
        let calls = (0..8).map(|_| {
            let (g, u) = (guard.clone(), url.clone());
            tokio::spawn(async move { g.fetch(get_req(&u)).await.unwrap() })
        });
        let mut ok = 0;
        for c in calls {
            if c.await.unwrap()["paid"] == true {
                ok += 1;
            }
        }
        // 60 sats a day, 23 reserved per payment: two fit, never three.
        assert_eq!(ok, 2);
        assert_eq!(paid.load(Ordering::SeqCst), 2);
    }
}
