# allowance

**A wallet with limits for AI agents.**

An agent asks `allowance` to fetch a paid API or call a paid MCP tool.
`allowance` pays the Lightning invoice only if the owner's rules allow it. The
agent never holds the wallet, the rules, or the proof of payment.

> **Status: prototype.** It pays L402 challenges over Lightning from an LND
> wallet and is tested on regtest. Don't point it at a wallet you can't afford
> to lose. [What works today](#what-works-today) · [Roadmap](#roadmap)

## Why

Paid APIs and MCP servers answer `402 Payment Required`. For an agent to use
them, it needs money, and today that means one of two things:

- **Hand the agent a wallet credential.** It can then spend everything, and so
  can a prompt injection that takes it over.
- **Put a budget inside the agent's own process.** The same process that can be
  talked into overspending is the one keeping count.

Neither is how you'd give money to anyone else you don't fully trust. You'd
give them an allowance: a fixed amount, for agreed things, with a record.

## What we aim to achieve

1. **Any agent can pay any 402, safely.** One guard that speaks the payment
   schemes agents meet (L402 today; x402 over Lightning, MPP and Cashu next),
   so an agent doesn't need a wallet per scheme.
2. **Limits the agent can't override.** Spending rules enforced in a separate
   process that a person controls, not in the agent.
3. **Bitcoin as the agent's money.** Lightning and ecash settle in seconds,
   need no account or card, and work for a 21-sat API call. Agents should have
   a wallet, not a corporate expense account.
4. **Works with the agent you already use.** It is an MCP server, so Goose,
   Claude Code, Cursor or any MCP host can use it without changes.

## How it works

```mermaid
sequenceDiagram
    autonumber
    participant A as Agent (MCP host)
    participant G as allowance daemon
    participant W as Lightning wallet
    participant S as Paid API / MCP server

    A->>G: pay_fetch(url) or pay_mcp_tool(server, tool, args)
    G->>S: request
    S-->>G: 402 + L402 challenge (token, invoice)
    Note over G: read the price from the invoice<br/>check policy, reserve budget
    alt policy allows
        G->>W: pay invoice (fee capped)
        W-->>G: preimage
        G->>S: request + Authorization: L402 token:preimage
        S-->>G: 200 + result
        Note over G: record in ledger
        G-->>A: result + receipt id
    else policy refuses
        Note over G: record refusal
        G-->>A: not paid, and why
    end
```

```
 agent's side                                   owner's side
┌──────────────────────────┐   MCP only   ┌────────────────────────────────┐
│ agent ── tools ──────────┼─────────────►│ allowance daemon               │
│  pay_fetch               │              │  policy.toml   limits, hosts   │
│  pay_mcp_tool            │              │  ledger.jsonl  every decision  │
│  quote                   │              │  wallet credentials            │
│  allowance_status        │◄─────────────┤                                │
└──────────────────────────┘  results,    └────────────────────────────────┘
                              never keys
```

The daemon is started by a person, with the wallet's credentials in its
environment. Agents reach it over MCP only: Streamable HTTP at `/mcp`, or the
`allowance shim` stdio bridge for hosts that launch extensions as commands (the
shim holds no credentials).

### The tools an agent gets

| Tool | What it does |
|---|---|
| `pay_fetch` | Fetches a URL. Pays an L402 challenge if the rules allow. |
| `pay_mcp_tool` | Calls a tool on a remote paid MCP server. Pays if the rules allow. |
| `quote` | Says what a URL would cost and whether it would be allowed. Pays nothing. |
| `allowance_status` | The limits, what's spent today, what's left, recent payments. |

No tool can change a limit, read the policy, or return a wallet credential,
macaroon or preimage.

### The rules an owner writes

```toml
per_payment_sat = 50      # the most one payment may cost, fee included
per_day_sat     = 200     # the most all payments may cost per UTC day
max_fee_sat     = 2       # routing fee the wallet may add to one payment
unlisted_hosts  = "deny"  # only the hosts below can be paid

[[host]]
name = "chessforsats.in"
per_payment_sat = 25
per_day_sat = 100

[[host]]
name = "*.example.com"
```

## Design principles

1. **The limit lives outside the agent.** Policy and credentials belong to a
   process a person starts. No tool reads or changes them.
2. **Trust the invoice, not the server.** Limits are checked against the amount
   encoded in the invoice, never the price a server claims.
3. **Reserve before paying.** Budget is held under one lock before the wallet
   is asked to pay, so concurrent calls can't overshoot a limit.
4. **Fail closed.** Unlisted hosts are refused by default. An invoice with no
   amount is refused. A payment whose outcome is unknown stays counted against
   the budget. A ledger that can't be read stops the daemon.
5. **A refusal is an answer.** The agent is told why, in words it can pass to
   its human.
6. **Everything is on the record.** Every decision is one line in a file the
   owner can read, with the proof of payment.
7. **One wallet, many schemes.** Each scheme is a different challenge and
   proof around the same check-reserve-pay-record flow.

## Quick start

```bash
cargo build --release

ALLOWANCE_POLICY=examples/policy.toml \
LND_REST_URL=https://127.0.0.1:8080 \
LND_MACAROON_PATH=/path/to/pay-only.macaroon \
LND_TLS_CERT_PATH=/path/to/tls.cert \
./target/release/allowance daemon
# allowance: MCP at http://127.0.0.1:4020/mcp | 50 sats/payment, 200 sats/day | ...
```

Then point an MCP host at it.

**Goose** (extensions are MCP servers, so nothing in Goose changes; not yet
tested inside Goose):

```yaml
# ~/.config/goose/config.yaml
extensions:
  allowance:
    type: streamable_http
    uri: http://127.0.0.1:4020/mcp
    enabled: true
```

**Hosts that launch extensions as commands:** use `allowance shim` (set
`ALLOWANCE_URL` if the daemon isn't on the default address).

| Variable | Default | |
|---|---|---|
| `ALLOWANCE_POLICY` | `policy.toml` | The owner's rules |
| `ALLOWANCE_LEDGER` | `ledger.jsonl` | Where decisions are recorded |
| `ALLOWANCE_LISTEN` | `127.0.0.1:4020` | Where the daemon listens |
| `LND_REST_URL`, `LND_MACAROON_PATH`, `LND_TLS_CERT_PATH` | | The LND wallet |

### A worked example

[`examples/demo.sh`](examples/demo.sh) plays the agent against a local
[chessforsats](https://chessforsats.in) stack, a chess game where a move costs
21 sats behind [ngx-l402](https://github.com/ngx-l402/ngx-l402). With a 50-sat
daily limit for the host:

| The agent asks to | Outcome |
|---|---|
| quote a move | 21 sats, allowed. Nothing paid. |
| play a move through the paid MCP tool `make_move` | paid 21 sats, move played |
| play a move over plain HTTP | paid 21 sats, move played |
| play a third move | **refused**: "today's 50-sat limit for localhost would be passed (42 sats already spent there)" |
| quote a host that isn't listed | **refused**: "satboard.bar is not on the list of hosts this wallet may pay" |

## What works today

- L402 over Lightning: plain HTTP, and `tools/call` on Streamable HTTP MCP
  servers that answer 402.
- LND wallet over REST, with a fee cap per payment.
- Per-payment, per-day and per-host limits; host allowlist with `*.` wildcards.
- Budget reservation under concurrency; fee-inclusive accounting.
- Append-only ledger with proof of payment.
- MCP over Streamable HTTP and a stdio shim.
- Tests that run the whole flow against a local paywall with a mock wallet
  (`cargo test`), including eight concurrent calls against a budget for two.

## Threat model

**Covered:** an agent, or a prompt injection driving it, that tries to
overspend, pay a host the owner didn't list, or obtain wallet credentials
through its tools.

**Not covered yet:**

- An agent with a shell, running as the same OS user as the daemon, can read
  the daemon's files. Run the daemon as another user, in a container, or on
  another machine.
- The daemon has no authentication of its own. It listens on loopback only;
  anything on the machine can ask it to pay, within the limits.
- It checks how much is paid and to which host, not whether the thing bought
  was worth it.

Give the wallet a macaroon that can only pay invoices.

## Roadmap

- **More schemes:** x402 with the Lightning scheme, MPP over Lightning, Cashu
  ecash (payments that can't be linked to each other).
- **More wallets:** NWC, Core Lightning, LDK.
- **Ask the human:** approval for payments over a threshold instead of a flat
  refusal.
- **Token reuse** for L402 tokens that are good for more than one request.
- **Proxy a whole MCP server,** so its paid tools appear natively in the host.
- **Isolation by default:** daemon authentication, a container image.
- **Agent hosts:** a tested Goose extension and setup guides for other hosts.

## Related work

- [ngx-l402](https://github.com/ngx-l402/ngx-l402): an nginx module that puts a
  Lightning paywall in front of any API or MCP server. It asks for payment;
  `allowance` pays.
- [L402sdk](https://github.com/lightninglabs/L402sdk) (Lightning Labs): an L402
  client library with a budget inside the agent's process. `allowance` is a
  separate process, aims at several schemes, and speaks MCP.
- [x402](https://github.com/coinbase/x402) payment proxies with spending caps
  exist for stablecoins. `allowance` is the same idea for Lightning.

## Contributing

Issues and pull requests are welcome, especially from people who run agents
with real money: what do you cap, and what went wrong?

```bash
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```

## License

MIT
