//! allowance: a spending guard for AI agents.
//!
//!   allowance daemon   run the guard (holds the wallet, the policy, the ledger)
//!   allowance shim     stdio MCP server that forwards to the daemon
//!
//! The daemon is started by a person, with the wallet's credentials in its
//! environment. Agents reach it over MCP only: Streamable HTTP at /mcp, or
//! the stdio shim for hosts that launch extensions as commands.

mod bolt11;
mod guard;
mod l402;
mod ledger;
mod mcp;
mod policy;
mod wallet;

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn env(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

async fn mcp_post(State(guard): State<Arc<guard::Guard>>, Json(msg): Json<Value>) -> Response {
    match mcp::handle(&guard, msg).await {
        Some(reply) => Json(reply).into_response(),
        // A notification: accepted, nothing to say.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

async fn daemon() -> Result<(), String> {
    let policy = policy::Policy::load(&env("ALLOWANCE_POLICY", "policy.toml"))?;
    let ledger = ledger::Ledger::open(&env("ALLOWANCE_LEDGER", "ledger.jsonl"))?;
    let wallet = wallet::LndRest::from_env()?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())?;
    let guard = Arc::new(guard::Guard {
        policy,
        ledger: tokio::sync::Mutex::new(ledger),
        wallet: Box::new(wallet),
        http,
    });
    // Loopback by default: the daemon is for agents on this machine.
    let listen = env("ALLOWANCE_LISTEN", "127.0.0.1:4020");
    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .with_state(guard.clone());
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("can't listen on {listen}: {e}"))?;
    eprintln!(
        "allowance: MCP at http://{listen}/mcp | {} sats/payment, {} sats/day | {} host rule(s), unlisted hosts: {:?}",
        guard.policy.per_payment_sat,
        guard.policy.per_day_sat,
        guard.policy.host.len(),
        guard.policy.unlisted_hosts
    );
    axum::serve(listener, app).await.map_err(|e| e.to_string())
}

/// stdio <-> daemon: one JSON-RPC message per line each way. Holds no
/// credentials, so it is safe for an agent host to launch.
async fn shim() -> Result<(), String> {
    let url = env("ALLOWANCE_URL", "http://127.0.0.1:4020/mcp");
    let http = reqwest::Client::new();
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await.map_err(|e| e.to_string())? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match http
            .post(&url)
            .header("Content-Type", "application/json")
            .body(line.clone())
            .send()
            .await
        {
            Ok(r) if r.status() == StatusCode::ACCEPTED => continue,
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(e) => {
                // Tell the agent the daemon is down, if this was a request.
                let id = serde_json::from_str::<Value>(&line)
                    .ok()
                    .and_then(|v| v.get("id").cloned());
                match id {
                    Some(id) => serde_json::json!({ "jsonrpc": "2.0", "id": id,
                        "error": { "code": -32000, "message": format!("the allowance daemon isn't reachable at {url}: {e}") } })
                    .to_string(),
                    None => continue,
                }
            }
        };
        out.write_all(reply.trim().as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        out.write_all(b"\n").await.map_err(|e| e.to_string())?;
        out.flush().await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        Some("daemon") => daemon().await,
        Some("shim") => shim().await,
        _ => Err("usage: allowance daemon | allowance shim".into()),
    };
    if let Err(e) = result {
        eprintln!("allowance: {e}");
        std::process::exit(1);
    }
}
