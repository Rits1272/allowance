# Security

`allowance` is a prototype that moves money. Please treat it that way: it has
only been run on regtest, and has not been audited.

## Reporting a problem

Please report anything that lets an agent spend past a limit, pay an unlisted
host, read wallet credentials or proofs of payment, or corrupt the ledger.

Use GitHub's private reporting: **Security → Report a vulnerability** on this
repository. Please don't open a public issue for something exploitable.

## What is in scope

The threat model in the README says what the guard is meant to stop and what it
does not stop yet. Known gaps listed there (no daemon authentication, no
isolation from an agent with a shell under the same OS user) are not news, but
better ideas for closing them are welcome.
