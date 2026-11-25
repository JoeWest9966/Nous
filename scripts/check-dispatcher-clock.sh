#!/usr/bin/env bash
# #73 零直接时钟判据：dispatcher 只留 Clock trait 与纯逻辑，
# 生产时钟实现唯一落在 app 装配处（crates/Nous/Server/src/clock.rs）。
# 命中即红（不是 SKIP）：没有 rg 就没法判，按仓规算未完成。
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

if ! command -v rg >/dev/null 2>&1; then
	printf 'RED: 找不到 rg，零时钟判据无法执行（不是 SKIP）\n' >&2
	exit 1
fi

HITS=$(rg -n 'SystemTime|Instant::now' crates/Nous/Dispatcher || true)
if [ -n "$HITS" ]; then
	printf 'RED: dispatcher 直接读时钟（应为 0 命中）：\n%s\n' "$HITS" >&2
	exit 1
fi

# 正向哨兵：唯一的生产时钟必须在 app 装配处，不能只有 trait 没有实现。
rg -q 'impl Clock for SystemClock' crates/Nous/Server/src/clock.rs || {
	printf 'RED: crates/Nous/Server/src/clock.rs 里没有生产 Clock 实现\n' >&2
	exit 1
}
rg -q 'nous_server::clock::system_clock\(\)' crates/Nous/Server/src/main.rs || {
	printf 'RED: main.rs 没有从 app 装配处注入生产 Clock\n' >&2
	exit 1
}

printf 'dispatcher 零直接时钟 ✓（rg 0 命中）；生产时钟唯一在 app 装配处 ✓\n'
exit 0
