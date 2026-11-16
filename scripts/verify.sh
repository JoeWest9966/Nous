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
	if [ "$WHAT" = ledger ]; then
		skip "crates/ 是空的 —— 只跑台账体检（ledger）"
	else
		skip "crates/ 是空的 —— 还没提货，下面三项全部跳过"
		bad "SOURCES.md 里仍有「待提货」的行，本库还不构成可验收状态"
		printf '\n\033[31m未通过：尚未提货\033[0m\n'
		exit 2
	fi
else
	ok "crates/ 已存在 $(ls crates | wc -l) 个 crate"
fi

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

	step "1c. conformance 用例数下限（防止套件被瘦成三行）"
	FLOOR=crates/nous-durable/conformance/FLOOR.tsv
	if [ -f "$FLOOR" ]; then
		THIN=0; FILES=0
		while IFS=$'\t' read -r file min; do
			case "$file" in ''|'#'*) continue ;; esac
			FILES=$((FILES+1))
			N=$(grep -cE '^[[:space:]]*#\[(tokio::)?test' "crates/nous-durable/conformance/$file" 2>/dev/null || true)
			N=${N:-0}
			if [ "$N" -ge "$min" ] 2>/dev/null; then
				ok "$file：$N ≥ $min"
			else
				bad "$file：只有 $N 条测试，下限 $min"; THIN=1
			fi
		 done < "$FLOOR"
		[ $THIN -eq 0 ] && ok "$FILES 个 conformance 文件都过了下限"
	else
		bad "缺 $FLOOR —— 用例数下限没写下来，套件可以悄悄瘦身"
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

# 4. 台账体检：四件事机器判，不靠自觉
#   4a 「待提货」的行不许已经有代码
#   4b 每一行的上游路径都要能用 git cat-file 在 pin 的那个 tag 上对上
#   4c 占位桩：todo!()/unimplemented!() 必须 0；注释里的 占位/stub/FIXME 只报数
#   4d 台账的四列 + 状态值不许出现别的取值
if [ "$WHAT" = all ] || [ "$WHAT" = ledger ]; then
	step "4. 台账体检"

	# 4a
	STALE=$(python3 - <<'PY'
import os
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
	if [ "$STALE" = clean ]; then ok "台账状态和代码一致"; else bad "这些路径已存在但台账还写「待提货」：$STALE"; fi

	# 4b + 4d：把每行拆成 TSV 交给 bash 逐行 cat-file
	ROWS=$(python3 - <<'PY'
import re,sys
src_map={'rpi@v0.24.2':('/tmp/pi-rust','v0.24.2'),'pi@v1.5.2':('/tmp/pi-check','v1.5.2')}
STATUS={'待提货','已提货','已改','自写'}
bad_status=[]
for raw in open('SOURCES.md',encoding='utf-8'):
    if not raw.startswith('| `'): continue
    cells=[c.strip() for c in raw.strip().strip('|').split('|')]
    if len(cells)!=5: continue
    local, source, upstream, change, status = cells
    if status not in STATUS:
        bad_status.append(f'{local}\t{status}'); continue
    if status=='自写':
        print(f'OWN\t{local}'); continue
    if status=='待提货' and not re.search(r'#\d+', change):
        print(f'NOTICKET\t{local}')
    if source not in src_map:
        print(f'SKIP\t{source}\t{upstream}'); continue
    repo,tag = src_map[source]
    upstream = upstream.strip('`')
    # {a,b} 展开；/** 收到父目录
    m=re.search(r'\{([^}]*)\}', upstream)
    paths=[upstream[:m.start()]+alt+upstream[m.end():] for alt in m.group(1).split(',')] if m else [upstream]
    for p in paths:
        if p.endswith('/**'): p=p[:-3]
        print(f'CHECK\t{repo}\t{tag}\t{p}')
for b in bad_status: print('BADSTATUS\t'+b)
PY
)
	MISSING=0; CHECKED=0; SKIPPED=0; OWNED=0; NOTICKET=0
	while IFS=$'\t' read -r kind a b2 c; do
		case "$kind" in
		CHECK)
			if [ ! -d "$a" ]; then SKIPPED=$((SKIPPED+1)); continue; fi
			CHECKED=$((CHECKED+1))
			if ! git -C "$a" cat-file -e "$b2:$c" 2>/dev/null; then
				bad "台账对不上上游：$b2:$c（$a）"; MISSING=$((MISSING+1))
			fi
			;;
		SKIP) SKIPPED=$((SKIPPED+1)) ;;
		OWN) OWNED=$((OWNED+1)) ;;
		NOTICKET) NOTICKET=$((NOTICKET+1)); bad "待提货 但没有票号（改动列里没有 #N）：$a" ;;
		BADSTATUS) bad "状态值不在四个值里（待提货/已提货/已改/自写）：$a → $b2" ;;
		esac
	done <<< "$ROWS"
	if [ $MISSING -eq 0 ]; then ok "$CHECKED 行上游路径全部对上（$SKIPPED 行跳过：来源不是 pi/pi-rust，或本地 checkout 不在；$OWNED 行自写，无上游）"; fi
	if [ $CHECKED -eq 0 ] && [ $SKIPPED -gt 0 ]; then skip "没有可核对的行 —— /tmp/pi-rust 或 /tmp/pi-check 不在"; fi

	# 4d not-ported.md：决定不做的上游东西，不许躺在提货台账里
	if [ -f docs/not-ported.md ]; then
		NP=$(python3 - <<'PY'
import re
src_map={'rpi@v0.24.2':('/tmp/pi-rust','v0.24.2'),'pi@v1.5.2':('/tmp/pi-check','v1.5.2')}
n=0
for raw in open('docs/not-ported.md',encoding='utf-8'):
    if not raw.startswith('| `'): continue
    cells=[c.strip() for c in raw.strip().strip('|').split('|')]
    if len(cells)<3: continue
    up, source = cells[0].strip('`'), cells[1]
    import os
    if os.path.exists(up.replace('/**','')):
        print(f'LOCAL\t{up}'); continue
    if not re.search(r'#\d+', cells[2]):
        print(f'NOREASON\t{up}'); continue
    if source not in src_map:
        print(f'NPSKIP\t{source}\t{up}'); continue
    repo,tag = src_map[source]
    m=re.search(r'\{([^}]*)\}', up)
    paths=[up[:m.start()]+alt+up[m.end():] for alt in m.group(1).split(',')] if m else [up]
    for p in paths:
        print(f'NPCHECK\t{repo}\t{tag}\t{p[:-3] if p.endswith("/**") else p}')
PY
)
		NPMISS=0; NPOK=0
		while IFS=$'\t' read -r kind a b2 c; do
			case "$kind" in
			NPCHECK) if [ -d "$a" ]; then
					if git -C "$a" cat-file -e "$b2:$c" 2>/dev/null; then NPOK=$((NPOK+1)); else bad "not-ported.md 编造了上游路径：$b2:$c"; NPMISS=$((NPMISS+1)); fi
				fi ;;
			LOCAL) bad "not-ported.md 里出现本地路径（不做的就不该有本地文件）：$a" ;;
			NOREASON) bad "not-ported.md 行没有理由 + 票号：$a" ;;
			esac
		done <<< "$NP"
		if [ $NPMISS -eq 0 ]; then ok "not-ported.md：$NPOK 行上游路径全部对上（不做的也编不了）"; fi
	else
		skip "没有 docs/not-ported.md（决定不做的上游东西记在哪里？）"
	fi

	# 4c 占位桩
	if [ -d crates ]; then
		if command -v rg >/dev/null 2>&1; then RG=rg; else RG="grep -rnE"; fi
		if [ "$RG" = rg ]; then
			TODO=$($RG -n 'todo!\(|unimplemented!\(' crates 2>/dev/null | wc -l)
			NOTES=$($RG -n '^\s*//.*(占位|stub|FIXME)' crates 2>/dev/null | wc -l)
		else
			TODO=$(grep -rnE 'todo!\(|unimplemented!\(' crates 2>/dev/null | wc -l)
			NOTES=$(grep -rnE '^\s*//.*(占位|stub|FIXME)' crates 2>/dev/null | wc -l)
		fi
		[ "$TODO" -eq 0 ] && ok "没有 todo!()/unimplemented!() 占位桩" || bad "有 $TODO 行 todo!()/unimplemented!() —— 占位桩不算实现"
		ok "注释里的 占位/stub/FIXME：$NOTES 行（只报数，逐条看）"
	fi
fi

printf '\n'
if [ $FAIL -eq 0 ]; then printf '\033[32m通过\033[0m\n'; exit 0; else printf '\033[31m未通过\033[0m\n'; exit 1; fi
