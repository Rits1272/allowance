#!/bin/sh
# Run from the agent's side of the boundary (see compose.yml: agent-side).
# Checks what something that can only reach the daemon's port can and can't do.
G=http://allowance:4020/mcp
fail=0
ok()  { echo "  ok    $1"; }
bad() { echo "  FAIL  $1"; fail=1; }
rpc() { wget -q -O- --header "Content-Type: application/json" --header "Authorization: Bearer $1" --post-data "$2" "$G" 2>/dev/null; }
call() { rpc "$ALLOWANCE_TOKEN" "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}"; }

echo "1. The wallet's files don't exist on this side"
for f in /run/secrets/lnd_macaroon /run/secrets/lnd_tls_cert /etc/allowance/policy.toml /data/ledger.jsonl; do
  [ -e "$f" ] && bad "$f is readable here" || ok "$f is not here"
done

echo "2. The daemon refuses callers without the token"
[ -z "$(rpc "" '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')" ] && ok "no token: refused" || bad "no token: answered"
[ -z "$(rpc "not-the-token-not-the-token-xx" '{"jsonrpc":"2.0","id":1,"method":"tools/list"}')" ] && ok "wrong token: refused" || bad "wrong token: answered"

echo "3. With the token, an agent gets four tools and none that changes a rule"
tools=$(rpc "$ALLOWANCE_TOKEN" '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | grep -o '"name":"[a-z_]*"' | cut -d'"' -f4 | sort | tr '\n' ' ')
[ "$tools" = "allowance_status pay_fetch pay_mcp_tool quote " ] && ok "tools: $tools" || bad "unexpected tools: $tools"

echo "4. No tool result carries a credential"
status=$(call allowance_status '{}')
echo "$status" | grep -q "spent_today_sat" && ok "status answers" || bad "status didn't answer"
echo "$status" | grep -q -i -E "macaroon|preimage|secret|/run/|token" && bad "status leaks something" || ok "status has no credential, path or token"

echo "5. The guard won't read local files or reach hosts the owner didn't list"
call pay_fetch '{"url":"file:///etc/allowance/policy.toml"}' | grep -q "only http and https" && ok "file:// refused" || bad "file:// not refused"
call pay_fetch '{"url":"http://unlisted.invalid/x"}' | grep -q "not on the list" && ok "unlisted host refused without contacting it" || bad "unlisted host not refused"

[ $fail = 0 ] && echo "All checks passed." || { echo "Some checks failed."; exit 1; }
