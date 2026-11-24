# Contributing

Thanks for looking. This is an early prototype, and the most useful thing right
now is feedback from people who let agents spend money: what you cap, how, and
what went wrong. An issue describing that is as welcome as code.

## Development

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test          # runs the whole flow against a local paywall and a mock wallet
```

CI runs the same three commands.

## Guidelines

- **Money paths fail closed.** If a check can't be made, the payment is refused.
- **Nothing reaches the agent that it could spend.** No credential, macaroon,
  token or preimage in any tool result.
- **Every payment decision is tested.** A change to the policy, the ledger or
  the payment flow comes with a test.
- Keep changes small and say what you tested in the pull request.

## Layout

| File | What it holds |
|---|---|
| `src/guard.rs` | The payment flow: request, 402, check, reserve, pay, retry, record |
| `src/policy.rs` | The owner's rules and the check against them |
| `src/ledger.rs` | The record of decisions and the budget reservations |
| `src/wallet.rs` | The wallet trait and the LND backend |
| `src/l402.rs`, `src/bolt11.rs` | Reading challenges and invoice amounts |
| `src/mcp.rs` | The MCP tools an agent sees |
| `src/main.rs` | The daemon and the stdio shim |
