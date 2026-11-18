#!/usr/bin/env bash
# 存储分离判据（issue #56）：Nous 自己的库（`crates/nous/store` / `nous.db`）跟 pi 的语义层
# （`crates/nous/durable` / `durable.db`）互不相干。
#
#   ./scripts/check-separation.sh
#
# 四条，每条都能红；空扫不算绿（被扫的路径必须存在、必须命中过东西）。
# 末尾打印 `checked=4 mismatched=N`。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

DURABLE="crates/nous/durable"
STORE="crates/nous/store"
STORE_SRC="$STORE/src"
STORE_MANIFEST="$STORE/Cargo.toml"

CHECKED=0
MISMATCHED=0
ok()  { printf '  \033[32m✓\033[0m %s\n' "$*"; CHECKED=$((CHECKED + 1)); }
bad() { printf '  \033[31m✗\033[0m %s\n' "$*"; CHECKED=$((CHECKED + 1)); MISMATCHED=$((MISMATCHED + 1)); }

if [ ! -d "$DURABLE/src" ] || [ ! -d "$STORE_SRC" ]; then
	printf '  \033[31m✗\033[0m 判据载体不在（%s/src 或 %s 缺失）—— 判据不是 SKIP，是红\n' "$DURABLE" "$STORE_SRC"
	printf '\nchecked=4 mismatched=4\n'
	exit 1
fi

rs_count() { find "$1" -name '*.rs' -type f | wc -l | tr -d ' '; }

# 只看代码行（`//` / `///` / `//!` 开头的注释不算）：命中行里那些是**说明**，
# 比如 lib.rs 里写「不许 ATTACH」的那段文档。排掉多少行要打印出来，否则规则可能悄悄是死的。
code_hits() {  # $1 = pattern（grep -E），$2… = 路径
	local pattern="$1"; shift
	grep -rnE "$pattern" "$@" --include=*.rs 2>/dev/null \
		| grep -vE ':[0-9]+:[[:space:]]*(//|///|//!)' || true
}

# ── 判据 1a：durable 里没有 Nous 触发器的踪迹 ────────────────────────────────
#
# pattern **不扫光秃秃的 `trigger`**：durable 里那个 `trigger: CompactionReason`
# （`harness/live.rs`）是 pi 的压缩触发，不是我们的触发器。扫它只会假红，
# 或者逼人去改 pi 的字段名 —— 两种都是判据在指挥代码。只扫**只有我们才会写的**字样。
NOUS_IN_DURABLE='trigger_code|plugin_snapshot|\bfires\b|\btriggers\b|nous_store'
HITS=$(code_hits "$NOUS_IN_DURABLE" "$DURABLE/src")
# 正向对照：同一个 pattern 在 store 里必须命中（≥1 行）。命中 0 = pattern 或路径写错了，
# 那样「durable 里 0 命中」是假绿。
CONTROL=$(code_hits "$NOUS_IN_DURABLE" "$STORE_SRC" | grep -c . || true)
if [ -z "$HITS" ] && [ "${CONTROL:-0}" -ge 1 ]; then
	ok "判据 1a：$DURABLE/src 的 $(rs_count "$DURABLE/src") 个 .rs 里没有触发器痕迹（对照：同一 pattern 在 store 命中 $CONTROL 行）"
elif [ -z "$HITS" ]; then
	bad "判据 1a 的对照是死的：同一 pattern 在 $STORE_SRC 里 0 命中 —— pattern 或路径写错了"
else
	bad "判据 1a：$DURABLE/src 里出现了 Nous 触发器的字样："
	printf '%s\n' "$HITS" | sed 's/^/       /'
fi

# ── 判据 1b：store 的依赖里没有 nous-durable ─────────────────────────────────
# 只看**非注释行**：Cargo.toml 的注释里明写「不许出现 nous-durable」是说明，不是依赖。
if [ ! -f "$STORE_MANIFEST" ]; then
	bad "判据 1b：$STORE_MANIFEST 缺失"
elif ! grep -q '^rusqlite' "$STORE_MANIFEST"; then
	bad "判据 1b 的对照是死的：$STORE_MANIFEST 里找不到 rusqlite 依赖行"
else
	DUPLICATES=$(grep -n 'nous-durable' "$STORE_MANIFEST" | grep -vE '^[0-9]+:[[:space:]]*#' || true)
	if [ -n "$DUPLICATES" ]; then
		bad "判据 1b：$STORE_MANIFEST 依赖了 nous-durable（触发器是 Nous 的实体，pi 的语义层里不该有它）："
		printf '%s\n' "$DUPLICATES" | sed 's/^/       /'
	else
		ok "判据 1b：$STORE_MANIFEST 的依赖里没有 nous-durable（只有 rusqlite）"
	fi
fi

# ── 判据 6：零直接时钟 ──────────────────────────────────────────────────────
CLOCK_HITS=$(code_hits 'SystemTime::now|Instant::now' "$STORE_SRC")
# 正向对照：时间一律由调用方传 —— `now_ms` 必须在场且不止一处（否则「零时钟」是空话）。
NOW_MS=$(grep -rc 'now_ms' "$STORE_SRC" --include=*.rs 2>/dev/null | awk -F: '{s += $2} END {print s + 0}')
if [ -z "$CLOCK_HITS" ] && [ "${NOW_MS:-0}" -ge 3 ]; then
	ok "判据 6：$STORE_SRC 里零直接时钟（SystemTime::now / Instant::now 0 命中；对照：now_ms $NOW_MS 处传参）"
elif [ -z "$CLOCK_HITS" ]; then
	bad "判据 6 的对照是死的：$STORE_SRC 里 now_ms 只出现 ${NOW_MS:-0} 次 —— 时间没在由调用方传？"
else
	bad "判据 6：$STORE_SRC 里有直接时钟（时间必须由调用方传，才能穷举测）："
	printf '%s\n' "$CLOCK_HITS" | sed 's/^/       /'
fi

# ── 判据 7：没有 ATTACH ────────────────────────────────────────────────────
ATTACH_HITS=$(code_hits '\bATTACH\b|\battach\b' "$STORE_SRC")
# 正向对照：lib.rs 的文档里说明「为什么没有 ATTACH」的那几行必须被排掉（规则活着）。
ATTACH_DROPPED=$(grep -rnE '\bATTACH\b|\battach\b' "$STORE_SRC" --include=*.rs 2>/dev/null \
	| grep -cE ':[0-9]+:[[:space:]]*(//|///|//!)' || true)
if [ -z "$ATTACH_HITS" ] && [ "${ATTACH_DROPPED:-0}" -ge 1 ]; then
	ok "判据 7：$STORE_SRC 的代码里没有 ATTACH（WAL 下跨库事务不原子；注释里的说明 $ATTACH_DROPPED 行已排掉）"
elif [ -z "$ATTACH_HITS" ]; then
	bad "判据 7 的对照是死的：注释里关于 ATTACH 的说明一行都没命中 —— 排除规则可能已失效"
else
	bad "判据 7：$STORE_SRC 里出现了 ATTACH（WAL 下跨库事务不原子，没有「一个事务写两个库」这条路）："
	printf '%s\n' "$ATTACH_HITS" | sed 's/^/       /'
fi

printf '\nchecked=%d mismatched=%d\n' "$CHECKED" "$MISMATCHED"
[ "$MISMATCHED" -eq 0 ]
