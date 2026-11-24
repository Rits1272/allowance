//! The human's rules. The agent cannot read or change them through the
//! guard: there is no tool that edits policy, and the file lives with the
//! daemon, not with the agent.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Policy {
    /// The most one payment may cost, fee included.
    pub per_payment_sat: u64,
    /// The most all payments may cost per UTC day, fees included.
    pub per_day_sat: u64,
    /// The routing fee the wallet may add to one payment.
    #[serde(default = "default_fee")]
    pub max_fee_sat: u64,
    /// What to do with a host that has no `[[host]]` entry.
    #[serde(default)]
    pub unlisted_hosts: Unlisted,
    #[serde(default)]
    pub host: Vec<HostRule>,
}

fn default_fee() -> u64 {
    10
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Unlisted {
    /// Only listed hosts can be paid. The safe default.
    #[default]
    Deny,
    /// Any host can be paid, within the global limits.
    Allow,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HostRule {
    /// Exact host, or `*.example.com` for its subdomains.
    pub name: String,
    pub per_payment_sat: Option<u64>,
    pub per_day_sat: Option<u64>,
}

/// What has already been spent (or reserved) today, in msat.
#[derive(Debug, Clone, Copy, Default)]
pub struct Spent {
    pub total_msat: u64,
    pub host_msat: u64,
}

impl Policy {
    pub fn load(path: &str) -> Result<Policy, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("can't read policy {path}: {e}"))?;
        toml::from_str(&text).map_err(|e| format!("policy {path}: {e}"))
    }

    fn rule(&self, host: &str) -> Option<&HostRule> {
        let host = host.to_ascii_lowercase();
        self.host.iter().find(|r| {
            let name = r.name.to_ascii_lowercase();
            match name.strip_prefix("*.") {
                Some(base) => host.ends_with(&format!(".{base}")),
                None => host == name,
            }
        })
    }

    /// Whether a payment of `cost_msat` (fee cap included) to `host` may go
    /// ahead. `Err` is the reason, in words the agent can pass to its human.
    pub fn check(&self, host: &str, cost_msat: u64, spent: Spent) -> Result<(), String> {
        let rule = self.rule(host);
        if rule.is_none() && self.unlisted_hosts == Unlisted::Deny {
            return Err(format!(
                "{host} is not on the list of hosts this wallet may pay"
            ));
        }
        let per_payment = rule
            .and_then(|r| r.per_payment_sat)
            .unwrap_or(self.per_payment_sat)
            .min(self.per_payment_sat);
        if cost_msat > per_payment * 1000 {
            return Err(format!(
                "this costs up to {} sats, over the {per_payment}-sat limit per payment{}",
                cost_msat.div_ceil(1000),
                if rule.is_some() {
                    format!(" for {host}")
                } else {
                    String::new()
                }
            ));
        }
        if let Some(day) = rule.and_then(|r| r.per_day_sat) {
            if spent.host_msat + cost_msat > day * 1000 {
                return Err(format!(
                    "today's {day}-sat limit for {host} would be passed ({} sats already spent there)",
                    spent.host_msat / 1000
                ));
            }
        }
        if spent.total_msat + cost_msat > self.per_day_sat * 1000 {
            return Err(format!(
                "today's {}-sat budget would be passed ({} sats already spent)",
                self.per_day_sat,
                spent.total_msat / 1000
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        toml::from_str(
            r#"
            per_payment_sat = 100
            per_day_sat = 500
            [[host]]
            name = "chess.example"
            per_payment_sat = 30
            per_day_sat = 60
            [[host]]
            name = "*.news.example"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn unlisted_hosts_are_refused_by_default() {
        assert!(policy()
            .check("evil.example", 1_000, Spent::default())
            .is_err());
    }

    #[test]
    fn host_limits_apply() {
        let p = policy();
        assert!(p.check("chess.example", 21_000, Spent::default()).is_ok());
        assert!(p.check("chess.example", 31_000, Spent::default()).is_err());
        let spent = Spent {
            total_msat: 42_000,
            host_msat: 42_000,
        };
        assert!(p.check("chess.example", 21_000, spent).is_err());
    }

    #[test]
    fn wildcard_and_global_limits() {
        let p = policy();
        assert!(p.check("a.news.example", 100_000, Spent::default()).is_ok());
        assert!(p
            .check("a.news.example", 100_001, Spent::default())
            .is_err());
        assert!(p.check("news.example", 1_000, Spent::default()).is_err());
        let spent = Spent {
            total_msat: 450_000,
            host_msat: 0,
        };
        assert!(p.check("a.news.example", 60_000, spent).is_err());
    }
}
