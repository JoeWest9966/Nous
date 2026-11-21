#!/usr/bin/env bash
# Desk 闸门判据（#60 判据 8）：真起 nous-app → 真 HTTP → 真落盘。
#
#   ./scripts/check-desk.sh
#
# 断言三件事（缺一不可）：
#   1. GET /v1/capabilities 里 presets.enabled == true 且 reason 消失；
#   2. PUT 一条模板后 GET /v1/presets/Session-Template 200 且 items 非空；
#   3. #61：GET /v1/combos/Session-Template 200。
#
# SKIP 不是 PASS：任何一步跑不起来都打印 SKIP 到 stderr 并退出非 0。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1
export PATH="$HOME/.cargo/bin:$PATH"

BIN="${CARGO_TARGET_DIR:-./target}/debug/nous-app"
if [ ! -x "$BIN" ]; then
	printf 'SKIP: %s 不存在（先 cargo build -p nous-server）\n' "$BIN" >&2
	exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
	printf 'SKIP: curl 不在 PATH\n' >&2
	exit 1
fi

TMP=$(mktemp -d)
trap 'kill ${PID:-} 2>/dev/null || true; wait ${PID:-} 2>/dev/null || true; rm -rf "$TMP"' EXIT

mkdir -p "$TMP/home" "$TMP/presets"
cat >"$TMP/preset.json" <<'JSON'
{"name":"desk-gate","summary":"判据样本","description":"","category":["默认"],"spec":{"v":1},"requestId":"desk-gate-1"}
JSON

env HOME="$TMP/home" \
    NOUS_DB="$TMP/nous.db" \
    NOUS_PLUGINS_DB="$TMP/nous-plugins.db" \
    NOUS_PRESETS="$TMP/presets" \
    NOUS_LISTEN=127.0.0.1:0 \
    "$BIN" >"$TMP/out" 2>"$TMP/err" &
PID=$!

ADDR=""
for _ in $(seq 1 100); do
	ADDR=$(grep -o '127\.0\.0\.1:[0-9]*' "$TMP/out" 2>/dev/null | head -1 || true)
	[ -n "$ADDR" ] && break
	kill -0 "$PID" 2>/dev/null || break
	sleep 0.1
done
if [ -z "$ADDR" ]; then
	printf 'SKIP: 服务没打出监听地址（stderr 如下）\n' >&2
	cat "$TMP/err" >&2
	exit 1
fi

FAILED=0
check() { # $1=描述 $2=实际值 $3=期望值
	if [ "$2" = "$3" ]; then
		printf '  ✓ %s: %s\n' "$1" "$2"
	else
		printf '  ✗ %s: 实际 %s，期望 %s\n' "$1" "$2" "$3" >&2
		FAILED=$((FAILED + 1))
	fi
}

CAP=$(curl -sS --max-time 5 "http://$ADDR/v1/capabilities")
ENABLED=$(printf '%s' "$CAP" | python3 -c 'import json,sys; m=[m for m in json.load(sys.stdin)["modules"] if m["name"]=="presets"]; print("true" if m and m[0]["enabled"] is True else "false")')
HAS_REASON=$(printf '%s' "$CAP" | python3 -c 'import json,sys; m=[m for m in json.load(sys.stdin)["modules"] if m["name"]=="presets"]; print("true" if m and "reason" in m[0] else "false")')
check 'capabilities.presets.enabled' "$ENABLED" 'true'
check 'capabilities.presets.reason 消失' "$HAS_REASON" 'false'

CODE=$(curl -sS --max-time 5 -o "$TMP/put.json" -w '%{http_code}' \
	-X PUT -H 'content-type: application/json' \
	--data-binary @"$TMP/preset.json" \
	"http://$ADDR/v1/presets/Session-Template/desk-gate")
check 'PUT /v1/presets/Session-Template/desk-gate' "$CODE" '201'

CODE=$(curl -sS --max-time 5 -o "$TMP/roster.json" -w '%{http_code}' \
	"http://$ADDR/v1/presets/Session-Template")
check 'GET /v1/presets/Session-Template' "$CODE" '200'
COUNT=$(python3 -c 'import json,sys; print(len(json.load(open(sys.argv[1]))["items"]))' "$TMP/roster.json")
if [ "$COUNT" -ge 1 ]; then
	printf '  ✓ roster.items 非空: %s 条\n' "$COUNT"
else
	printf '  ✗ roster.items 是空的 —— Desk 模板页会白屏\n' >&2
	FAILED=$((FAILED + 1))
fi

CODE=$(curl -sS --max-time 5 -o "$TMP/combos.json" -w '%{http_code}' \
	"http://$ADDR/v1/combos/Session-Template")
check 'GET /v1/combos/Session-Template' "$CODE" '200'
COMBOS_OK=$(python3 -c 'import json,sys; print("true" if isinstance(json.load(open(sys.argv[1]))["combos"], list) else "false")' "$TMP/combos.json")
check 'combos 是数组' "$COMBOS_OK" 'true'

if [ "$FAILED" -ne 0 ]; then
	printf 'check-desk: 红（%d 条）\n' "$FAILED" >&2
	exit 1
fi
printf 'check-desk: 绿\n'
