//! Where the sats come from. The wallet's credentials live only in the
//! daemon; nothing here is reachable from an MCP tool.

use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};

pub struct Paid {
    pub preimage_hex: String,
    pub fee_msat: u64,
}

pub struct PayError {
    /// True when the wallet says nothing was sent. False when we can't tell
    /// (a timeout, a dropped connection): the budget stays spent.
    pub definite: bool,
    pub message: String,
}

#[async_trait]
pub trait Wallet: Send + Sync {
    fn name(&self) -> &'static str;
    async fn pay(&self, invoice: &str, max_fee_sat: u64) -> Result<Paid, PayError>;
}

/// An LND node over its REST API. A macaroon limited to paying invoices is
/// enough; the guard needs no other permission.
pub struct LndRest {
    url: String,
    macaroon_hex: String,
    http: reqwest::Client,
}

impl LndRest {
    pub fn from_env() -> Result<LndRest, String> {
        let url = std::env::var("LND_REST_URL")
            .map_err(|_| "set LND_REST_URL (e.g. https://127.0.0.1:8080)")?;
        let mac_path = std::env::var("LND_MACAROON_PATH").map_err(|_| "set LND_MACAROON_PATH")?;
        let macaroon =
            std::fs::read(&mac_path).map_err(|e| format!("can't read {mac_path}: {e}"))?;
        // LND's certificate is self-signed and is its own CA, which rustls
        // refuses as a server certificate; the platform TLS stack accepts it
        // once it is added as a root.
        let mut builder = reqwest::Client::builder()
            .use_native_tls()
            .timeout(std::time::Duration::from_secs(60));
        if let Ok(cert_path) = std::env::var("LND_TLS_CERT_PATH") {
            let pem =
                std::fs::read(&cert_path).map_err(|e| format!("can't read {cert_path}: {e}"))?;
            let cert =
                reqwest::Certificate::from_pem(&pem).map_err(|e| format!("{cert_path}: {e}"))?;
            builder = builder.add_root_certificate(cert);
        }
        let http = builder.build().map_err(|e| e.to_string())?;
        Ok(LndRest {
            url: url.trim_end_matches('/').to_string(),
            macaroon_hex: hex::encode(macaroon),
            http,
        })
    }
}

#[async_trait]
impl Wallet for LndRest {
    fn name(&self) -> &'static str {
        "lnd"
    }

    async fn pay(&self, invoice: &str, max_fee_sat: u64) -> Result<Paid, PayError> {
        let unsure = |message: String| PayError {
            definite: false,
            message,
        };
        let resp = self
            .http
            .post(format!("{}/v1/channels/transactions", self.url))
            .header("Grpc-Metadata-macaroon", &self.macaroon_hex)
            .json(&json!({ "payment_request": invoice, "fee_limit": { "fixed": max_fee_sat.to_string() } }))
            .send()
            .await
            .map_err(|e| {
                // A connection that never opened sent nothing.
                let message = format!("wallet didn't answer: {e}");
                PayError { definite: e.is_connect(), message }
            })?;
        let status = resp.status();
        let body: Value = resp
            .json()
            .await
            .map_err(|e| unsure(format!("wallet answer unreadable: {e}")))?;
        if let Some(err) = body
            .get("payment_error")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        {
            return Err(PayError {
                definite: true,
                message: format!("payment failed: {err}"),
            });
        }
        if !status.is_success() {
            let msg = body
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("wallet refused");
            // 4xx: LND rejected the request before trying to pay.
            return Err(PayError {
                definite: status.is_client_error(),
                message: format!("wallet: {msg}"),
            });
        }
        let preimage_b64 = body
            .get("payment_preimage")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| unsure("wallet returned no preimage".into()))?;
        let preimage = base64::engine::general_purpose::STANDARD
            .decode(preimage_b64)
            .map_err(|_| unsure("wallet returned an unreadable preimage".into()))?;
        let fee_msat = body
            .pointer("/payment_route/total_fees_msat")
            .and_then(|v| v.as_str().and_then(|s| s.parse().ok()).or(v.as_u64()))
            .unwrap_or(0);
        Ok(Paid {
            preimage_hex: hex::encode(preimage),
            fee_msat,
        })
    }
}
