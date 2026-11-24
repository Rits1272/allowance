# allowance with Goose

Checked with Goose 1.60.2 on macOS, against a local chessforsats stack on
regtest (a move costs 21 sats, charged by ngx-l402).

## 1. Start the daemon

```bash
ALLOWANCE_POLICY=examples/policy.toml \
LND_REST_URL=https://127.0.0.1:8080 \
LND_MACAROON_PATH=/path/to/pay-only.macaroon \
LND_TLS_CERT_PATH=/path/to/tls.cert \
./target/release/allowance daemon
```

`examples/policy.toml` allows `localhost` up to 50 sats a day.

## 2. Start Goose with the extension

```bash
goose session --with-streamable-http-extension "http://127.0.0.1:4020/mcp"
```

Goose lists four tools from `allowance`: `pay_fetch`, `pay_mcp_tool`, `quote`
and `allowance_status`.

## 3. Ask it to spend

> Play three chess moves at http://localhost:8404, one at a time, with
> pay_mcp_tool (server_url http://localhost:8404/mcp, tool make_move). Read the
> game first with pay_fetch on http://localhost:8404/api/state. Report what
> happened on each attempt.

What Goose did in our run, from the daemon's ledger:

```
r-1  paid    make_move  21 sat
r-2  paid    make_move  21 sat
r-3  denied  make_move  21 sat   today's 50-sat limit for localhost would be passed (42 sats already spent there)
r-4  denied  make_move  21 sat   today's 50-sat limit for localhost would be passed (42 sats already spent there)
```

And what it told its user (abridged):

> One of the three moves went through. The other two were refused because they
> would pass the wallet's daily limit. [...] I can't change that limit, so I
> didn't try to get around it. [...] The wallet's owner needs to raise the
> localhost daily limit or wait for it to reset.

(`r-1` was a move from an earlier Goose session the same day; Goose noticed it
in the status and said so.)

## Notes

- The agent's tools carry no wallet credential, macaroon or preimage; the
  ledger on the owner's side keeps the proof of payment.
- This run used Goose with no other extensions. An agent that also has a shell
  on the same machine and user can read the daemon's files: see the threat
  model in the README.
