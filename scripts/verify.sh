#!/usr/bin/env bash
# 本库唯一的"做完了"定义。跑不通 = 没做完。
#
#   ./scripts/verify.sh              # 全部
#   ./scripts/verify.sh unit         # 只跑测试
#   ./scripts/verify.sh fixture      # 只跑 agent.sqlite fixture
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

FAIL=0
step() { printf '\n\033[1m── %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
skip() { printf '  \033[33m–\033[0m %s\n' "$*"; }

WHAT="${1:-all}"

# 0. 提货体检：crates 一个都没有时，别让人误以为"全都过了"
step "0. 提货体检"
if [ -z "$(ls -A crates 2>/dev/null)" ]; then
	skip "crates/ 是空的 —— 还没提货，下面三项全部跳过"
	bad "SOURCES.md 里仍有「待提货」的行，本库还不构成可验收状态"
	printf '\n\033[31m未通过：尚未提货\033[0m\n'
	exit 2
fi
ok "crates/ 已存在 $(ls crates | wc -l) 个 crate"

# 1. 单元测试 + 一致性套件
if [ "$WHAT" = all ] || [ "$WHAT" = unit ]; then
	step "1. cargo test --workspace（含改写自 pi testing 的一致性套件）"
	if cargo test --workspace --quiet; then ok "全绿"; else bad "有失败"; fi

	step "1b. conformance 套件确实跑到了（不是被 --exclude 掉）"
	if cargo test -p nous-durable --test conformance --quiet 2>/dev/null; then
		ok "nous-durable 一致性套件存在且通过"
	else
		bad "找不到 nous-durable 的 conformance 测试 —— 验收的核心缺了"
	fi
fi

# 2. fixture：线上那份 agent.sqlite 读得出 /v1/sessions 同形的 JSON
if [ "$WHAT" = all ] || [ "$WHAT" = fixture ]; then
	step "2. agent.sqlite fixture"
	FIX="${NOUS_FIXTURE:-fixtures/agent.sqlite}"
	if [ ! -f "$FIX" ]; then
		skip "$FIX 不存在（真实用户数据，不进 git）"
		skip "取法：scp root@192.168.1.7:/opt/Nous/content/new1/map/qrd3wh/data/agent.sqlite fixtures/"
		bad "fixture 缺了就无法验证会话树读得对不对"
	else
		if cargo run --quiet -p nous-fixture -- "$FIX" > /tmp/nous-fixture.json 2>/tmp/nous-fixture.err; then
			# 关键字段一个不能少
			for k in id name parent model updatedAt last; do
				grep -q "\"$k\"" /tmp/nous-fixture.json || bad "输出里缺字段 $k"
			done
			N=$(grep -o '"id"' /tmp/nous-fixture.json | wc -l)
			[ "$N" -ge 189 ] && ok "读出 $N 条会话（≥189）" || bad "只读出 $N 条，期望 ≥189"
		else
			bad "fixture 跑不起来：$(head -3 /tmp/nous-fixture.err)"
		fi
	fi
fi

# 3. 冷启动
if [ "$WHAT" = all ] || [ "$WHAT" = boot ]; then
	step "3. 冷启动不 panic"
	BIN=$(cargo metadata --no-deps --format-version 1 2>/dev/null |
		python3 -c 'import json,sys; d=json.load(sys.stdin); print(next((t["name"] for p in d["packages"] for t in p["targets"] if t["kind"]==["bin"]),""))' 2>/dev/null)
	if [ -z "$BIN" ]; then
		skip "还没有 bin target"
	else
		if cargo build --release --quiet -p "$BIN" >/dev/null 2>&1; then
			timeout 5 "./target/release/$BIN" >/tmp/nous-boot.log 2>&1
			rc=$?
			# 124 = 被 timeout 杀掉 = 一直活着，这是好的
			if [ $rc -eq 124 ] || [ $rc -eq 0 ]; then ok "起得来（rc=$rc）"; else bad "启动失败 rc=$rc：$(head -3 /tmp/nous-boot.log)"; fi
		else
			bad "编译失败"
		fi
	fi
fi

# 4. 台账体检：不许残留「待提货」的路径却已经在 crates/ 里出现
if [ "$WHAT" = all ]; then
	step "4. 台账体检"
	STALE=$(python3 - <<'PY'
import re,os
rows=[l for l in open('SOURCES.md',encoding='utf-8') if l.startswith('| `crates/') or l.startswith('| `vendor/')]
n=0
for r in rows:
    cells=[c.strip() for c in r.strip().strip('|').split('|')]
    if len(cells)<5: continue
    path, status = cells[0].strip('`'), cells[-1]
    if status=='待提货' and os.path.exists(path.replace('/**','')):
        print(path); n+=1
print('' if n else 'clean')
PY
)
	if [ "$STALE" = clean ]; then ok "台账和代码一致"; else bad "这些路径已存在但台账还写「待提货」：$STALE"; fi
fi

printf '\n'
if [ $FAIL -eq 0 ]; then printf '\033[32m通过\033[0m\n'; exit 0; else printf '\033[31m未通过\033[0m\n'; exit 1; fi
