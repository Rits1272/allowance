//! Every payment decision, appended to a JSONL file the human can read.
//!
//! Budget is *reserved* before the wallet is asked to pay, under one lock,
//! so concurrent calls can't overshoot a limit. A payment whose outcome is
//! unknown (a timeout, say) stays counted: better to under-spend than to
//! pay twice past the budget.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::policy::{Policy, Spent};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub at: u64,
    pub host: String,
    pub url: String,
    /// What the agent asked for: a tool name on a paid MCP server, or the HTTP method.
    pub what: String,
    pub scheme: String,
    /// The invoice amount.
    pub amount_msat: u64,
    /// The routing fee actually paid (0 until known).
    pub fee_msat: u64,
    /// "paid", "denied", "failed" (nothing left the wallet) or "unknown".
    pub status: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    /// Proof of payment, kept for the human. Never shown to the agent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preimage: String,
    /// The HTTP status of the request once paid.
    #[serde(default)]
    pub served: u16,
}

pub struct Ledger {
    path: String,
    entries: Vec<Entry>,
    /// Budget held for payments in flight: (id, host, msat).
    reserved: Vec<(String, String, u64)>,
    next: u64,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn day(at: u64) -> u64 {
    at / 86_400
}

impl Ledger {
    pub fn open(path: &str) -> Result<Ledger, String> {
        let mut entries = Vec::new();
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                // A ledger we can't read must stop the daemon: guessing "nothing
                // spent" would hand the agent a fresh budget.
                entries.push(
                    serde_json::from_str::<Entry>(line)
                        .map_err(|e| format!("ledger {path}: {e}"))?,
                );
            }
        }
        let next = entries.len() as u64 + 1;
        Ok(Ledger {
            path: path.to_string(),
            entries,
            reserved: Vec::new(),
            next,
        })
    }

    fn counts(e: &Entry) -> bool {
        e.status == "paid" || e.status == "unknown"
    }

    pub fn spent_today(&self, host: &str) -> Spent {
        let today = day(now());
        let mut s = Spent::default();
        for e in self
            .entries
            .iter()
            .filter(|e| day(e.at) == today && Self::counts(e))
        {
            s.total_msat += e.amount_msat + e.fee_msat;
            if e.host == host {
                s.host_msat += e.amount_msat + e.fee_msat;
            }
        }
        for (_, h, msat) in &self.reserved {
            s.total_msat += msat;
            if h == host {
                s.host_msat += msat;
            }
        }
        s
    }

    fn id(&mut self) -> String {
        let id = format!("r-{}", self.next);
        self.next += 1;
        id
    }

    /// Checks the policy and, if it allows, holds `cost_msat` of budget.
    /// Returns the receipt id to settle later, or the refusal.
    pub fn authorize(
        &mut self,
        policy: &Policy,
        host: &str,
        cost_msat: u64,
    ) -> Result<String, String> {
        policy.check(host, cost_msat, self.spent_today(host))?;
        let id = self.id();
        self.reserved
            .push((id.clone(), host.to_string(), cost_msat));
        Ok(id)
    }

    /// Records the outcome of an authorized payment and frees its reservation.
    pub fn settle(&mut self, entry: Entry) -> Result<(), String> {
        self.reserved.retain(|(id, _, _)| *id != entry.id);
        self.append(entry)
    }

    /// Records a refusal (nothing was reserved).
    pub fn deny(&mut self, mut entry: Entry) -> Result<String, String> {
        entry.id = self.id();
        let id = entry.id.clone();
        self.append(entry)?;
        Ok(id)
    }

    fn append(&mut self, entry: Entry) -> Result<(), String> {
        let line = serde_json::to_string(&entry).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("can't write ledger {}: {e}", self.path))?;
        writeln!(f, "{line}").map_err(|e| format!("can't write ledger {}: {e}", self.path))?;
        self.entries.push(entry);
        Ok(())
    }

    pub fn recent(&self, n: usize) -> Vec<&Entry> {
        self.entries.iter().rev().take(n).collect()
    }
}
