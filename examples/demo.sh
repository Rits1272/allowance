#!/usr/bin/env bash
# Plays the part of an agent: talks MCP to the guard, nothing else.
#   examples/demo.sh            (daemon on 127.0.0.1:4020, chessforsats dev stack on :8404)
set -euo pipefail
G=${ALLOWANCE_URL:-http://127.0.0.1:4020/mcp}
CHESS=${CHESS_URL:-http://localhost:8404}
call() { curl -s -X POST "$G" -H 'Content-Type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"$1\",\"arguments\":$2}}" | python3 -c "import json,sys; print(json.load(sys.stdin)['result']['content'][0]['text'])"; }
next() { curl -s "$CHESS/api/state" | python3 -c "import json,sys; s=json.load(sys.stdin); print(s['team_to_move'], s['legal_san'][0])"; }

echo "## tools the agent sees"; curl -s -X POST "$G" -H 'Content-Type: application/json' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | python3 -c "import json,sys; [print(' -', t['name']) for t in json.load(sys.stdin)['result']['tools']]"
echo; echo "## 1. what would a chess move cost? (quote, pays nothing)"
call quote "{\"url\":\"$CHESS/api/agent/move\",\"method\":\"POST\",\"body\":\"{}\"}"
read -r team move < <(next)
echo; echo "## 2. play $move for $team through the paid MCP tool"
call pay_mcp_tool "{\"server_url\":\"$CHESS/mcp\",\"tool\":\"make_move\",\"arguments\":{\"move\":\"$move\",\"team\":\"$team\",\"name\":\"guarded-agent\"}}"
read -r team move < <(next)
echo; echo "## 3. play $move for $team over plain HTTP (L402)"
call pay_fetch "{\"url\":\"$CHESS/api/agent/move\",\"method\":\"POST\",\"headers\":{\"Content-Type\":\"application/json\"},\"body\":\"{\\\"move\\\":\\\"$move\\\",\\\"team\\\":\\\"$team\\\",\\\"name\\\":\\\"guarded-agent\\\"}\"}"
read -r team move < <(next)
echo; echo "## 4. a third move: today's limit for this host is spent"
call pay_mcp_tool "{\"server_url\":\"$CHESS/mcp\",\"tool\":\"make_move\",\"arguments\":{\"move\":\"$move\",\"team\":\"$team\",\"name\":\"guarded-agent\"}}"
echo; echo "## 5. a host the owner never listed"
call quote '{"url":"https://satboard.bar/api/takeover","method":"POST","body":"{}"}'
echo; echo "## 6. where the allowance stands"
call allowance_status '{}'
