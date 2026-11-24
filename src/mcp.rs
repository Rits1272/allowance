//! The guard as an MCP server: the only surface an agent can touch.
//!
//! Four tools. None of them can change a limit, read the policy file, or
//! see a wallet credential, a macaroon or a preimage.

use serde_json::{json, Value};

use crate::guard::{Guard, Request};

const PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    json!([
        {
            "name": "pay_fetch",
            "description": "Fetch a URL. If it answers 402 Payment Required with a Lightning (L402) invoice, pay it from the owner's wallet and return the paid response. Payments are refused when they pass the owner's limits; a refusal says why. Use `quote` first if unsure of the price.",
            "inputSchema": { "type": "object", "required": ["url"], "properties": {
                "url": { "type": "string" },
                "method": { "type": "string", "description": "GET (default), POST, ..." },
                "headers": { "type": "object", "additionalProperties": { "type": "string" } },
                "body": { "type": "string", "description": "Request body, sent as is" }
            } }
        },
        {
            "name": "pay_mcp_tool",
            "description": "Call a tool on a remote, paid MCP server (Streamable HTTP). If the call answers 402 with a Lightning (L402) invoice, pay it within the owner's limits and return the tool's result.",
            "inputSchema": { "type": "object", "required": ["server_url", "tool"], "properties": {
                "server_url": { "type": "string", "description": "The MCP endpoint, e.g. https://chessforsats.in/mcp" },
                "tool": { "type": "string" },
                "arguments": { "type": "object" }
            } }
        },
        {
            "name": "quote",
            "description": "Ask what a URL would cost and whether the owner's limits allow it. Pays nothing.",
            "inputSchema": { "type": "object", "required": ["url"], "properties": {
                "url": { "type": "string" },
                "method": { "type": "string" },
                "body": { "type": "string" }
            } }
        },
        {
            "name": "allowance_status",
            "description": "The spending limits the owner set, what has been spent today, what is left, and recent payments.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn text(v: &Value) -> Value {
    json!({ "content": [{ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() }] })
}

fn s(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

async fn call(guard: &Guard, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "pay_fetch" | "quote" => {
            let url = s(args, "url").ok_or("url is required")?;
            let method = s(args, "method").unwrap_or_else(|| "GET".into());
            let headers = args
                .get("headers")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let req = Request {
                what: method.to_ascii_uppercase(),
                method,
                url,
                headers,
                body: s(args, "body"),
            };
            if name == "quote" {
                guard.quote(req).await
            } else {
                guard.fetch(req).await
            }
        }
        "pay_mcp_tool" => {
            let url = s(args, "server_url").ok_or("server_url is required")?;
            let tool = s(args, "tool").ok_or("tool is required")?;
            let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
            // A stateless tools/call in the 2026-07-28 Streamable HTTP shape:
            // the method and tool are mirrored in headers, which is what a
            // paywall in front of the server prices the call by.
            let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": tool, "arguments": arguments,
                "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                           "io.modelcontextprotocol/clientCapabilities": {} } } });
            let headers = vec![
                ("Content-Type".into(), "application/json".into()),
                (
                    "Accept".into(),
                    "application/json, text/event-stream".into(),
                ),
                ("MCP-Protocol-Version".into(), "2026-07-28".into()),
                ("Mcp-Method".into(), "tools/call".into()),
                ("Mcp-Name".into(), tool.clone()),
            ];
            let req = Request {
                method: "POST".into(),
                url,
                headers,
                body: Some(body.to_string()),
                what: tool,
            };
            let mut out = guard.fetch(req).await?;
            // Unwrap the upstream's JSON-RPC (plain JSON or one SSE event).
            if let Some(raw) = out.get("body").and_then(Value::as_str) {
                let payload = raw
                    .lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .unwrap_or(raw);
                if let Ok(v) = serde_json::from_str::<Value>(payload) {
                    let result = v
                        .get("result")
                        .or_else(|| v.get("error"))
                        .cloned()
                        .unwrap_or(v);
                    out["result"] = result;
                    out.as_object_mut().map(|o| o.remove("body"));
                }
            }
            Ok(out)
        }
        "allowance_status" => Ok(guard.status().await),
        _ => Err(format!("no such tool: {name}")),
    }
}

/// One JSON-RPC message in, one out (`None` for a notification).
pub async fn handle(guard: &Guard, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));
    let ok = |result: Value| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    match method {
        "initialize" => {
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL);
            ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "allowance", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "A wallet with limits. Use pay_fetch for paid HTTP APIs and pay_mcp_tool for paid MCP tools; both pay Lightning (L402) invoices within limits the wallet's owner set. You cannot change the limits. If a payment is refused, tell the user the reason."
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call(guard, name, &args).await {
                Ok(v) => ok(text(&v)),
                Err(e) => {
                    ok(json!({ "isError": true, "content": [{ "type": "text", "text": e }] }))
                }
            }
        }
        _ => Some(
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": format!("method not found: {method}") } }),
        ),
    }
}
