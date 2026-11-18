#!/usr/bin/env bash
# 本仓唯一的"做完了"定义。跑不通 = 没做完。
#
#   ./scripts/verify.sh              # 全部
#   ./scripts/verify.sh unit         # 只跑测试
#   ./scripts/verify.sh fixture      # 只跑 agent.sqlite fixture
#   ./scripts/verify.sh boundary     # 只跑 spec 第 1 章的边界判据（15 条）
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

FAIL=0
step() { printf '\n\033[1m── %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
skip() { printf '  \033[33m–\033[0m %s\n' "$*"; }

WHAT="${1:-all}"

# 分层不写在路径上（spec §3.0）：成员由 `scripts/layer-paths.sh` 从
# `docs/plugins.md` 的 layer 列算出来（$K / $A），本脚本不写死模块名。

# 0. 提货体检：crates 一个都没有时，别让人误以为"全都过了"
step "0. 提货体检"
if [ -z "$(ls -d crates/*/*/ 2>/dev/null)" ]; then
	bad "crates/ 是空的 —— #14 搬迁没做完，下面全部失去判据载体"
	printf '\n\033[31m未通过：等 #14 搬迁\033[0m\n'
	exit 2
else
	ok "crates/*/* 已存在 $(ls -d crates/*/*/ | wc -l) 个 crate（层见 docs/plugins.md）"
fi

# 0b. 清单体检（spec §3.0 的两条判据）：目录集合 == 清单模块集合（双向），
#     且 layer=kernel 的 crate 的 normal 依赖闭包里没有 layer=platform / app 的 crate。
step "0b. 清单体检（docs/plugins.md × crates/*/*）"
if [ ! -e docs/plugins.md ] || [ ! -x scripts/layer-paths.sh ]; then
	bad "docs/plugins.md 或 scripts/layer-paths.sh 缺失 —— 清单是判据的一部分（RED）"
else
	KERNEL_PATHS=$(scripts/layer-paths.sh kernel)
	PLATFORM_PATHS=$(scripts/layer-paths.sh platform)
	APP_PATHS=$(scripts/layer-paths.sh app)
	DIRS=$(ls -d crates/*/*/ 2>/dev/null | sed 's:/$::' | sort)
	DEV_PATHS=$(scripts/layer-paths.sh dev)
	LISTED=$(printf '%s %s %s %s' "$KERNEL_PATHS" "$PLATFORM_PATHS" "$APP_PATHS" "$DEV_PATHS" | tr ' ' '\n' | sed '/^$/d' | sort)
	if [ "$DIRS" = "$LISTED" ]; then
		ok "目录集合 == 清单模块集合（$(printf '%s\n' "$DIRS" | grep -c .) 个，双向逐行相等）"
	else
		bad "目录与清单不一一对应（< 目录有、清单没写；> 清单写了、目录不在）："
		diff <(printf '%s\n' "$DIRS") <(printf '%s\n' "$LISTED") | grep -E '^[<>]' | sed 's/^/       /'
	fi
	# dev 层正集哨兵：dev 为空会让所有"减 dev 层"的排除静默失效，所以这里钉死它的成员。
	DEV_SORTED=$(printf '%s\n' $DEV_PATHS | sed '/^$/d' | sort | tr '\n' ' ' | sed 's/ *$//')
	if [ "$DEV_SORTED" = "crates/nous/ext-probe crates/nous/fixture" ]; then
		ok "dev 层正集哨兵：$DEV_SORTED（探针 + 夹具；空 dev 会让排除静默失效）"
	else
		bad "dev 层集合不是「fixture + ext-probe」（实得：${DEV_PATHS:-空}）—— 减 dev 层的排除会跟着失效"
	fi
	LAYER_BAD=$(K="$KERNEL_PATHS" P="$PLATFORM_PATHS" A="$APP_PATHS" python3 - <<'PYLAYER'
import json, os, subprocess
meta = json.loads(subprocess.run(['cargo', 'metadata', '--format-version', '1'],
                                 capture_output=True, text=True).stdout)
byid = {p['id']: p for p in meta['packages']}
name2id = {p['name']: p['id'] for p in meta['packages']}
def under(path, dirs):
    d = os.path.dirname(path)
    return any(d == os.path.abspath(x) for x in dirs)
K = os.environ['K'].split(); P = os.environ['P'].split(); A = os.environ['A'].split()
def closure(pid, seen):
    for dep in byid[pid]['dependencies']:
        if dep.get('kind') not in (None, 'normal'):   # dev-/build- 边不是分层判据的对象
            continue
        tid = name2id.get(dep['name'])
        if not tid or tid in seen:
            continue
        seen.add(tid); closure(tid, seen)
bad = []
for pid, pkg in byid.items():
    if not under(pkg['manifest_path'], K):
        continue
    seen = set(); closure(pid, seen)
    for other in seen:
        op = byid[other]
        if under(op['manifest_path'], P) or under(op['manifest_path'], A):
            rel = os.path.relpath(os.path.dirname(op['manifest_path']))
            bad.append(f"{pkg['name']} → {op['name']}（{rel}）")
print('\n'.join(bad))
raise SystemExit(1 if bad else 0)
PYLAYER
	)
	if [ -z "$LAYER_BAD" ]; then
		ok "layer=kernel 的 crate 依赖闭包里没有 layer=platform / app 的 crate"
	else
		bad "kernel 层依赖了上层："
		printf '%s\n' "$LAYER_BAD" | sed 's/^/       /'
	fi
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
	FLOOR=crates/nous/durable/conformance/FLOOR.tsv
	if [ -f "$FLOOR" ]; then
		THIN=0; FILES=0
		while IFS=$'\t' read -r file min; do
			case "$file" in ''|'#'*) continue ;; esac
			FILES=$((FILES+1))
			N=$(grep -cE '^[[:space:]]*#\[(tokio::)?test' "crates/nous/durable/conformance/$file" 2>/dev/null || true)
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

# 1d. providers 门后那半：默认构建不带网络，测试不能跟着一起不跑
if [ "$WHAT" = all ] || [ "$WHAT" = test ]; then
	step "1d. nous-ai 在 --features providers 下重跑（不发真网络，全 mock）"
	PROV=$(cargo test -p nous-ai --features providers 2>&1) || {
		printf '%s\n' "$PROV" | tail -20; bad "providers 门后的测试挂了（默认构建看不见的那半）"
	}
	if [ -n "${PROV:-}" ]; then
		# 只跑不查等于没跑：钉两条锚点测试，名字改了/被删了就红
		for ANCHOR in leading_system_goes_to_the_system_parameter_for_anthropic \
		              collapse_carries_the_whole_current_tool_set_not_the_last_batch; do
			if printf '%s' "$PROV" | grep -q "test providers.*$ANCHOR\|$ANCHOR \.\.\." || \
			   printf '%s' "$PROV" | grep -q "$ANCHOR"; then
				ok "锚点跑到了：$ANCHOR"
			else
				bad "锚点 $ANCHOR 没跑 —— providers 这半被静默跳过了"
			fi
		done
	fi
fi

# 2. fixture：线上那份 agent.sqlite 读得出 /v1/sessions 同形的 JSON
if [ "$WHAT" = all ] || [ "$WHAT" = fixture ]; then
	step "2. agent.sqlite fixture"
	# 默认用**进仓的脱敏小样本**（#43）：干净 clone 也能真跑，CI 才不是永远 SKIP。
	# 真数据（190 条，不入库）用 NOUS_FIXTURE=fixtures/agent.sqlite 指过去做人工验。
	FIX="${NOUS_FIXTURE:-fixtures/agent.small.sqlite}"
	if [ ! -f "$FIX" ] && [ -z "${NOUS_FIXTURE:-}" ] && [ -f fixtures/agent.sqlite ]; then
		FIX=fixtures/agent.sqlite
		skip "小样本不在，退到真数据：$FIX"
	fi
	if [ ! -f "$FIX" ]; then
		skip "SKIP: $FIX 缺失（小样本应在仓里；真数据从 j1900 scp）"
		skip "取法：scp root@192.168.1.7:/opt/Nous/content/new1/map/qrd3wh/data/agent.sqlite fixtures/"
		skip "重做小样本：python3 scripts/make-small-fixture.py"
	else
		if cargo run --quiet -p nous-fixture -- "$FIX" > /tmp/nous-fixture.json 2>/tmp/nous-fixture.err; then
			# 判据必须是「**至少 100 条**、id 降序、字段齐、无重复」，不是数 "id" 字符串。
			# 下限 100 是哨兵：只剩 3 条的库照样「字段齐、降序、无重复」，但没有下限时
			# 删到 3 条也是绿的 —— 那条判据就成了摆设（#43）。
			CHECK=$(python3 - <<'PY'
import json
try:
    doc=json.load(open('/tmp/nous-fixture.json',encoding='utf-8'))
except Exception as e:
    print('BAD'); print(f'输出不是合法 JSON：{e}'); raise SystemExit
items=doc.get('items')
if not isinstance(items,list):
    print('BAD'); print('输出里没有 items 数组'); raise SystemExit
need=['id','name','parent','model','updatedAt','last']
missing=[k for k in need if any(k not in it for it in items)]
if missing:
    print('BAD'); print('有会话缺字段：'+','.join(missing)); raise SystemExit
ids=[it['id'] for it in items]
if len(ids)!=len(set(ids)):
    print('BAD'); print(f'id 有重复（{len(ids)} 条，{len(set(ids))} 个唯一）'); raise SystemExit
if ids!=sorted(ids,reverse=True):
    print('BAD'); print('不是 id 降序'); raise SystemExit
if len(ids)<100:
    print('BAD'); print(f'只有 {len(ids)} 条会话（下限 100 —— 小样本被截过头了？）'); raise SystemExit
print('OK'); print(len(ids))
PY
)
			if [ "$(head -1 <<<"$CHECK")" = OK ]; then
				ok "读出 $(tail -n +2 <<<"$CHECK") 条会话（id 降序、字段齐、无重复）"
			else
				bad "fixture 输出不对：$(tail -n +2 <<<"$CHECK")"
			fi
		else
			bad "fixture 跑不起来：$(head -3 /tmp/nous-fixture.err)"
		fi
	fi
fi

# 3. 冷启动
if [ "$WHAT" = all ] || [ "$WHAT" = boot ]; then
	step "3. 冷启动不 panic"
	# 冷启动只认这一个对象，**写死**：进 systemd 的那个 bin（spec §1：一个产物 nous-app）。
	# `nous-fixture` 的逻辑测试在 1 段里跑，不占这一格。
	# BIN 是产物名（进 systemd 的那个）；它住在 package nous-server 里（-p 认包名）。
	BIN=nous-app
	BIN_PKG=nous-server
	if ! cargo metadata --no-deps --format-version 1 2>/dev/null | grep -q "\"name\":\"$BIN\""; then
		bad "没有 $BIN 这个 bin target —— 冷启动对象被删了，先改这里再改代码（S4：进 systemd 的 bin 恰好 1 个）"
	else
		if cargo build --release --quiet -p "$BIN_PKG" --bin "$BIN" >/dev/null 2>&1; then
			# 端口用 0（临时端口）：固定端口会和「正在跑的那个服务」撞车。
			# 顺手把「到底 bind 上了没有」变成断言：只打日志不看 rc 的话，起不来也算过。
			timeout 5 env NOUS_LISTEN=127.0.0.1:0 "./target/release/$BIN" >/tmp/nous-boot.log 2>&1
			rc=$?
			if ! grep -q "listening on 127.0.0.1:" /tmp/nous-boot.log; then
				bad "没打出 listening 行 —— 起来了但没 bind，或启动日志变了：$(head -3 /tmp/nous-boot.log)"
			# 124 = 被 timeout 杀掉 = 一直活着，这是好的
			elif [ $rc -eq 124 ] || [ $rc -eq 0 ]; then ok "起得来并 bind 上了（rc=$rc）"; else bad "启动失败 rc=$rc：$(head -3 /tmp/nous-boot.log)"; fi
		else
			bad "编译失败"
		fi
	fi
fi

# 3c. #51 faux provider 启动闸门（判据 4：非回环拒绝启动）
#     cargo test 里验不了「进程拒绝启动」，只能用真进程 + 真 stderr。
if [ "$WHAT" = all ] || [ "$WHAT" = boot ] || [ "$WHAT" = faux ]; then
	step "3c. #51 faux 只允许绑回环"
	BIN=nous-app
	FAUX_BIN="${CARGO_TARGET_DIR:-/opt/Nous/target}/release/$BIN"
	if [ ! -x "$FAUX_BIN" ]; then
		bad "$FAUX_BIN 不存在 —— 先跑一次 boot 段构建"
	else
		FAUX_TMP=$(mktemp -d)
		NOUS_DB="$FAUX_TMP/refuse.db" NOUS_LISTEN=0.0.0.0:4792 NOUS_PROVIDER=faux \
			"$FAUX_BIN" >"$FAUX_TMP/out" 2>"$FAUX_TMP/err"
		FAUX_RC=$?
		if [ "$FAUX_RC" -ne 0 ] && grep -q '只允许绑回环' "$FAUX_TMP/err"; then
			ok "faux + 非回环 → 拒绝启动（rc=$FAUX_RC，stderr 含「只允许绑回环」）"
		else
			bad "faux + 非回环没拒绝（rc=$FAUX_RC，stderr=$(head -2 "$FAUX_TMP/err")）"
		fi
		rm -rf "$FAUX_TMP"
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

	# 4e 依赖边：只许契约图里那几条（机器版在这条命令旁边，改表同时改这里）
	ALLOWED=$(python3 - <<'PY'
import json,subprocess
allow={
 'nous-telemetry': set(),
 'nous-chord': set(),
 'nous-wire': set(),
 'nous-env': {'nous-chord'},
 'nous-ai': {'nous-telemetry','nous-ai-types'},
 'nous-tools': {'nous-ai','nous-env'},
 'nous-runtime': {'nous-ai'},
 'nous-durable': {'nous-chord','nous-ai','nous-wire','nous-telemetry','nous-env','nous-ai-types'},
 'nous-server': {'nous-durable','nous-wire','nous-telemetry'},
}
meta=json.loads(subprocess.run(['cargo','metadata','--no-deps','--format-version','1'],
                               capture_output=True,text=True).stdout)
bad=[]
for pkg in meta['packages']:
    name=pkg['name']
    if name not in allow: continue
    for dep in pkg['dependencies']:
        d=dep['name']
        if not d.startswith('nous-') or d==name: continue
        if d not in allow[name]: bad.append(f'{name} → {d}')
print('\n'.join(bad))
PY
)
	# durable 是语义层，永久不许绑传输
	if cargo tree -p nous-durable -e normal 2>/dev/null | grep -qE '\b(axum|hyper|tower-http|actix\w*|warp)\b'; then
		bad "nous-durable 依赖了 HTTP 框架 —— 语义层不绑传输，端点住 nous-server"
	fi
	if [ -z "$ALLOWED" ]; then ok "依赖边全在契约图表里（durable 无 HTTP 依赖）"; else
		while read -r e; do [ -n "$e" ] && bad "依赖边不在契约图表里：$e"; done <<< "$ALLOWED"
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
# ════════════════════════════════════════════════════════════════════════════
# 5. spec 第 1 章「边界与仲裁」的 15 条判据（Q18 / S1 / S2 / S4 / S8 / S12）
#
#    判据正文只住在 spec 里（`docs/spec/kernel-rust.md` §1），这里逐条实现，
#    不改口径。**层不写在路径上**（spec §3.0）：kernel / app 的成员由
#    `scripts/layer-paths.sh` 从 `docs/plugins.md` 的 layer 列算出来（$K / $A），
#    本脚本不写死任何模块名。
#
#    三段退化语义（spec 读法，逐字）：
#      docs/plugins.md 缺失 / 不可解析  → RED（清单是判据的一部分，缺了没法判）
#      scripts/layer-paths.sh 缺失      → RED（同上）
#      K 为空 或 crates/*/* 一个都没有   → SKIP（打印出来），因为 #14 还没搬
#    红/绿/skip 各归各，互不连坐（判据互相独立，一条红不拦后面的）。
# ════════════════════════════════════════════════════════════════════════════
if [ "$WHAT" = all ] || [ "$WHAT" = boundary ]; then
	step "5. spec 第 1 章：边界判据（15 条）"
	B_GREEN=0; B_RED=0; B_SKIP=0
	bok()  { ok "$1";   B_GREEN=$((B_GREEN+1)); }
	bbad() { bad "$1";  B_RED=$((B_RED+1)); }
	bskip(){ skip "$1"; B_SKIP=$((B_SKIP+1)); }

	# ── 守门块（spec §1 逐字）──
	# 只允许"打印出 SKIP 然后跳过这条判据"，不许静默 exit 0；配置缺失不算 SKIP。
	GUARD_MSG=""
	guard() {
		GUARD_MSG=""
		if [ ! -e docs/plugins.md ]; then
			GUARD_MSG="RED: docs/plugins.md 缺失（清单是判据的一部分）"; return 1
		fi
		if [ ! -x scripts/layer-paths.sh ]; then
			GUARD_MSG="RED: scripts/layer-paths.sh 缺失"; return 1
		fi
		K=$(scripts/layer-paths.sh kernel 2>/dev/null) && A=$(scripts/layer-paths.sh app 2>/dev/null) \
			&& D=$(scripts/layer-paths.sh dev 2>/dev/null) || {
			K=""; A=""; D=""; GUARD_MSG="RED: docs/plugins.md 不可解析（scripts/layer-paths.sh 读不出来）"; return 1
		}
		# `dev` 层（测试夹具与探针）是判据的**信号源**：空 dev 会让"减 dev 层"静默失效。
		if [ -z "$D" ]; then
			GUARD_MSG="RED: dev 层为空（docs/plugins.md 里没有 layer=dev 的行）—— 排除会静默失效"; return 1
		fi
		if ! ls -d crates/*/*/ >/dev/null 2>&1; then
			GUARD_MSG="SKIP: crates/*/* 一个都没有，等 #14 搬迁"; return 2
		fi
		if [ -z "$K" ]; then
			GUARD_MSG="SKIP: K 为空（清单里没有 layer=kernel 的行），等 #14 搬迁"; return 2
		fi
		return 0
	}

	# 判据出口统一走 judge：守门不过就按退化语义记 RED / SKIP，过了才真跑判据。
	judge() {
		local label="$1" fn="$2" out rc
		guard; rc=$?
		if [ $rc -eq 2 ]; then bskip "$label —— $GUARD_MSG"; return; fi
		if [ $rc -eq 1 ]; then bbad "$label —— $GUARD_MSG"; return; fi
		out=$("$fn" 2>&1); rc=$?
		if [ $rc -eq 2 ]; then bskip "$label —— $out"; return; fi
		if [ $rc -eq 0 ]; then
			bok "$label"
			[ -n "$out" ] && printf '%s\n' "$out" | sed 's/^/       /'
		else
			bbad "$label"
			printf '%s\n' "$out" | sed 's/^/       /'
		fi
	}

	# 排除必须可观测（spec 读法第 6 条：负向断言要配一个只有真实现才产生的字节）。
	# `grep -v` 的路径拼错会排掉 0 行，看上去和"排对了"一样绿 —— 所以每条判据都把
	# 「排掉了 N 行，来自：<路径×行数>」打进输出，并断言 N > 0。
	report_dropped() {  # $1=判据名 $2=被排掉的清单 $3=分隔符（默认 :）
		local who="$1" dropped="$2" sep="${3:-:}" n
		n=$(printf '%s\n' "$dropped" | grep -c . || true)
		if [ "$n" -eq 0 ]; then
			echo "$who 的排除项是死的：排掉 0 行（路径拼错？规则失效？）"
			return 1
		fi
		echo "$who 排掉了 $n 行，来自：$(printf '%s\n' "$dropped" | cut -d"$sep" -f1 | sort | uniq -c | awk '{printf "%s×%s ", $2, $1}')"
	}

	# 把属于 `dev` 层的行减掉：名单来自 `$D`（`scripts/layer-paths.sh dev`），**不点名任何目录**。
	# 输入/输出都是 "路径:行号:内容" 或纯路径；按 `<dev 模块路径>/` 前缀匹配整行。
	dev_reject() {
		local out p
		out=$(cat)
		for p in ${D:-}; do
			out=$(printf '%s\n' "$out" | grep -v "^${p}/" || true)
		done
		printf '%s\n' "$out"
	}

	# 1. 正向依赖：进 systemd 的那个 bin（`nous-app`，包名 `nous-server`）
	#    --depth 1 含 durable/wire/tools/runtime，不含 chord/rusqlite。
	#    spec 写的是产物名 `nous-app`；`cargo tree -p` 认的是**包名**，所以取它住的包。
	c1() {
		local tree
		tree=$(cargo tree -p nous-server --depth 1 -e normal 2>/dev/null) || {
			echo "cargo tree -p nous-server 失败（nous-app 的包不在了？）"; return 1; }
		local miss=""
		for need in durable wire tools runtime; do
			printf '%s\n' "$tree" | grep -q "nous-$need" || miss="$miss 缺:nous-$need"
		done
		for ban in chord rusqlite; do
			printf '%s\n' "$tree" | grep -qE "(nous-)?$ban" && miss="$miss 多:$ban"
		done
		if [ -n "$miss" ]; then
			echo "正向依赖:$miss"; printf '%s\n' "$tree" | head -12; return 1
		fi
	}

	# 2. 反向依赖：内核层任一 crate 的 --depth 1 依赖里没有应用层的 crate。
	#    "应用层" = `docs/plugins.md` 里 layer=app 的模块（$A）；platform 层不算上层（S2/6b）。
	c2() {
		K="$K" A="$A" python3 - <<'PY'
import json, os, subprocess
k = os.environ['K'].split(); a = os.environ['A'].split()
meta = json.loads(subprocess.run(['cargo', 'metadata', '--no-deps', '--format-version', '1'],
                                 capture_output=True, text=True).stdout)
kdirs = {os.path.abspath(d) for d in k}
adirs = {os.path.abspath(d) for d in a}
bad = []
for p in meta['packages']:
    here = os.path.dirname(p['manifest_path'])
    if here not in kdirs:
        continue
    for dep in p['dependencies']:
        if dep.get('path') and os.path.abspath(dep['path']) in adirs:
            bad.append(f"{p['name']} → {dep['name']}")
print('\n'.join(bad))
raise SystemExit(1 if bad else 0)
PY
	}

	# 3. 词表基线（S8：基线只减不增，新增一行即红）
	#
	# 基线按「路径 + 行文本」记，**不记行号**（#45 签字改的口径）：行号是缓存，
	# 任何人在上面插一行，下面所有基线条目都会漂移成"新增"，判据假红。误红比漏红更贵 ——
	# 红久了就有人重排基线把红变绿，S8 这条防线就是这么被磨掉的。
	# 比对是**多重集**：同一路径同一文本出现两次要算两次，少算一次就等于给了个白名单。
	c3() {
		local BASE=docs/boundary-baseline.txt
		[ -f "$BASE" ] || { echo "$BASE 不存在"; return 1; }
		local HITS
		HITS=$(git grep -nE '\b(skill|preset|prompt|template|board|block|resolve)\b' -- $K \
			| grep -v -i licenses || true)
		HITS="$HITS" BASE="$BASE" python3 - <<'PY'
import collections, os, re, sys

hits = [l for l in os.environ['HITS'].splitlines() if l.strip()]


def key(line):
    """`路径:行号:文本` → `路径: 文本`：键里不留行号，插入就不再误伤。"""
    m = re.match(r'^(.+?):\d+:(.*)$', line)
    return f'{m.group(1)}: {m.group(2)}' if m else line


keys = [key(line) for line in hits]
base = []
for raw in open(os.environ['BASE'], encoding='utf-8'):
    if raw.lstrip().startswith('#') or not raw.strip():
        continue
    text = re.split(r'  # ', raw.rstrip('\n'), maxsplit=1)[0].rstrip()
    # 只认 `crates/...` 开头的条目：文件头的说明行、`spawn_files=N` 之类指令都不算词表
    if not text.startswith('crates/'):
        continue
    base.append(text)

# 空集不许绿：命中为空 = pattern 或 $K 写错了；基线为空 = 文件被清空了
if not keys:
    print('NOHIT 命中 0 行 —— pattern 或 $K 写错了？判据不许空跑绿')
    sys.exit(1)
if not base:
    print('NOBASE 基线 0 条 —— 文件被清空了？判据不许空跑绿')
    sys.exit(1)

kh, bh = collections.Counter(keys), collections.Counter(base)
new, dropped = kh - bh, bh - kh
print(f'命中 {len(keys)} 行 / 基线 {len(base)} 条，按「路径 + 行文本」比对')

if new:
    print(f'词表基线被新增 {sum(new.values())} 行 —— 改基线必须单独一张票：')
    seen = collections.Counter()
    for line in hits:  # 打印带行号的原始命中行，定位用
        seen[key(line)] += 1
        if seen[key(line)] <= new.get(key(line), 0):
            print(f'       + {line}')
    sys.exit(1)
if dropped:
    print(f'（基线比命中多 {sum(dropped.values())} 条：只减不增，减少不红，顺手删掉）')
print('命中全部在基线里（只减不增）')
PY
	}
	# 4. 路由归属：内核层不许出现 HTTP 框架
	c4() {
		local F
		F=$(git grep -nE '\.route\(|axum::|tower::' -- $K || true)
		[ -z "$F" ] || { echo "$F"; return 1; }
	}

	# 5. 数据面唯一入口：$K 之外不许出现 rusqlite / Connection::open / .sqlite / PRAGMA / BEGIN IMMEDIATE。
	#    "之外"只算代码（crates/ 下的 .rs）；tests/ 与 *_test(s).rs 里的临时库名是夹具，不是第二入口。
	c5() {
		local ex=() p RAW KEPT DROPPED
		for p in $K; do ex+=(":(exclude)$p"); done
		RAW=$(git grep -nE 'rusqlite|Connection::open|\.sqlite|PRAGMA|BEGIN IMMEDIATE' -- crates "${ex[@]}" || true)
		# 只算**生产**路径：.rs 代码行、去掉注释、去掉 tests/，再**减 `dev` 层**
		# （测试夹具与探针；spec 判据 11 的「只许测试用」同样对待它们）。
		KEPT=$(printf '%s\n' "$RAW" | grep '\.rs:' | grep -v '/tests/' | grep -vE '_tests?\.rs:' \
			| grep -vE ':[0-9]+:\s*(//|///|//!)' \
			| grep -v 'DEFAULT_FIXTURE' | grep -v 'fixtures/agent.sqlite' | grep -v '<legacy.sqlite>' \
			| dev_reject || true)   # 排掉的是 `dev` 层（探针/夹具），名单来自清单
		DROPPED=$(comm -23 <(printf '%s\n' "$RAW" | sed '/^$/d' | sort) <(printf '%s\n' "$KEPT" | sed '/^$/d' | sort) || true)
		report_dropped "5." "$DROPPED" || return 1
		[ -z "$KEPT" ] || { echo "$KEPT"; return 1; }
	}

	# 6. 进程归应用层（S1 + S12）：内核层逐行为空；全仓进程创建点按**文件**数，
	#    集合必须与 `docs/boundary-baseline.txt` 的 `spawn_files=N` + 成员清单**逐行相等**
	#    （S12 已落地，不再打 SKIP）。成员与排除项都从 baseline 读，本脚本不写死模块名。
	c6() {
		local F
		F=$(git grep -nE 'std::process|tokio::process|Command::new|Child' -- $K \
			| grep -v 'ExitCode' | grep -v '/tests/' | grep -v '/conformance/' || true)
		if [ -n "$F" ]; then echo "内核层出现进程："; printf '%s\n' "$F"; return 1; fi

		local BASE=docs/boundary-baseline.txt
		[ -f "$BASE" ] || { echo "$BASE 不存在"; return 1; }
		local N_BASE MEMBERS EXCLUDED NOW DIFF
		N_BASE=$(grep -c '^spawn_files=' "$BASE" || true)
		[ "$N_BASE" -eq 1 ] || { echo "baseline 里 spawn_files 行有 $N_BASE 条（应为 1）"; return 1; }
		local N_VAL
		N_VAL=$(sed -n 's/^spawn_files=\([0-9]*\).*/\1/p' "$BASE")
		# 成员 = spawn_files 行之后、第一个空行之前的 `#   crates/…` 行（取第一个字段 = 路径）
		MEMBERS=$(awk '/^spawn_files=/{f=1;next}
			f && /^[[:space:]]*$/ {exit}
			f && /^#[[:space:]]+crates\// {sub(/^#[[:space:]]+/,""); print $1}' "$BASE")
		# 排除项 = 「排除项」小节里的 `#   crates/…` 行（目录以 / 结尾；文件可带 :行号）
		EXCLUDED=$(awk '/^# 排除项/{g=1;next}
			g && /^#[[:space:]]+crates\// {sub(/^#[[:space:]]+/,""); print $1}' "$BASE")

		# 数**构造点**（谁建了进程），与判据 12 共用一条 pattern。`.spawn()` 数不到
		# `os_env.rs:443` 的 `cmd.status()` —— 它也在创建进程，只是不叫 spawn。
		NOW=$(grep -rlE 'Command::new\(|::fork\(|posix_spawn\(|execve\(' crates --include=*.rs \
			| grep -vE '(/tests/|tests\.rs$)' | sort || true)
		local PRE="$NOW" DROPPEDF
		local ex
		while IFS= read -r ex; do
			[ -z "$ex" ] && continue
			ex=${ex%:*}
			case "$ex" in
			*/) NOW=$(printf '%s\n' "$NOW" | grep -v "^${ex}" || true) ;;
			*)  NOW=$(printf '%s\n' "$NOW" | grep -vx -- "$ex" || true) ;;
			esac
		done <<< "$EXCLUDED"
		# 减 `dev` 层（探针/夹具），名单来自清单；不点名目录。
		NOW=$(printf '%s\n' "$NOW" | dev_reject | sed '/^$/d' | sort || true)
		DROPPEDF=$(comm -23 <(printf '%s\n' "$PRE" | sed '/^$/d' | sort) <(printf '%s\n' "$NOW" | sed '/^$/d' | sort) || true)
		report_dropped "6." "$DROPPEDF" || return 1

		DIFF=$(printf '%s\n%s\n' "$NOW" "$MEMBERS" | grep -v '^$' | sort | uniq -u || true)
		if [ -n "$DIFF" ]; then
			echo "spawn 文件集合与 baseline 不相等（± = 差集）："
			printf '%s\n' "$DIFF" | sed 's/^/       ± /'
			echo "实测集合（$(printf '%s\n' "$NOW" | grep -c .) 个）：$(printf '%s' "$NOW" | tr '\n' ' ')"
			return 1
		fi
		echo "spawn 文件集合逐行相等（$(printf '%s\n' "$NOW" | grep -c .) 个 == baseline 的 spawn_files=$N_VAL）：$(printf '%s' "$NOW" | tr '\n' ' ')"
	}

	# 6b. 依赖方向：真相层不许依赖会 sleep 的那个 crate —— `cargo tree -p nous-durable --depth 2 | grep providers` 为空。
	c6b() {
		local F
		F=$(cargo tree -p nous-durable --depth 2 2>/dev/null | grep providers || true)
		[ -z "$F" ] || { echo "$F"; return 1; }
	}

	# 7. 端点索引唯一：`^### GET /v1/` 恰好 1 个文件（wire.md），content/ 是已死面板的数据镜像，不算。
	c7() {
		local F N
		F=$(git grep -l '^### GET /v1/' -- ':!content/**' || true)
		N=$(printf '%s\n' "$F" | grep -c . || true)
		[ "$N" -eq 1 ] && [ "$F" = "wire.md" ] || { echo "端点索引 $N 个文件：$F"; return 1; }
	}

	# 7b. 索引不抄字段：wire.md 里出现 `"type":` 或 camelCase 字段名清单的段落数 = 0。
	c7b() {
		python3 - <<'PY'
import re
lines = open('wire.md', encoding='utf-8').read().splitlines()
paras = []; start = None; buf = []
for i, line in enumerate(lines, 1):
    if not line.strip():
        if buf: paras.append((start, i - 1, '\n'.join(buf))); buf = []; start = None
    else:
        if start is None: start = i
        buf.append(line)
if buf: paras.append((start, len(lines), '\n'.join(buf)))
CAM = re.compile(r'[a-z][A-Za-z0-9]*[A-Z][A-Za-z0-9]*')
def field_list(seg):
    if ',' not in seg and '，' not in seg: return False
    return len(set(CAM.findall(seg))) >= 2
hits = []
for s, e, p in paras:
    groups = [m.group(0) for m in re.finditer(r'\{[^\{\}\n]*\}', p)]
    in_fence = False; fence = []; fenced = []
    for line in p.splitlines():
        if line.lstrip().startswith('```'):
            if in_fence and fence: fenced.append('\n'.join(fence)); fence = []
            in_fence = not in_fence
        elif in_fence: fence.append(line)
    if fence: fenced.append('\n'.join(fence))
    why = []
    if re.search(r'"type"\s*:', p): why.append('"type":')
    if any(field_list(g) for g in groups + fenced): why.append('camelCase 字段清单')
    if why: hits.append(f'{s}-{e}: {"/".join(why)}')
if hits:
    print(f'wire.md 抄了字段清单（{len(hits)} 个段落，抄一遍就会漂）：')
    print('\n'.join(hits))
    raise SystemExit(1)
PY
	}

	# 8. 代码不引文档（S8）：内核 .rs 里 wire.md 去掉注释行后为空
	c8() {
		local F
		F=$(git grep -n 'wire\.md' -- $K | grep '\.rs:' \
			| grep -vE ':[0-9]+:\s*(//|///|//!)' || true)
		[ -z "$F" ] || { echo "$F"; return 1; }
	}

	# 9. 一个 workspace 根；进 systemd 的 bin target 恰好 1 个（减 `dev` 层，S4）
	c9() {
		local NROOT RAW RAWN KEPT DROPPEDN DROPPED NBINS
		NROOT=$(git ls-files | grep -xc 'Cargo.toml' || true)
		# name<TAB>目录：名字用来判，目录用来在「排掉了什么」里指认
		RAW=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
			| python3 -c "import json,os,sys
m=json.load(sys.stdin)
for p in m['packages']:
    d=os.path.relpath(os.path.dirname(p['manifest_path']))
    for t in p['targets']:
        if 'bin' in t['kind']: print(t['name']+'\\t'+d)")
		RAWN=$(printf '%s\n' "$RAW" | cut -f1 | sed '/^$/d' | sort)
		# 「进 systemd 的」= 清单里**非 dev 层**的模块的 bin：`dev` 层（探针/夹具）不进产品产物，
		# 名单来自 `$D`；`exts/*` 不是本表里的模块（运行时外挂，按需加载），不在 `crates/*/*` 里。
		KEPT=$(printf '%s\n' "$RAW" | awk -F'\t' -v dev="$D" '
			BEGIN { n=split(dev, a, " "); for (i=1;i<=n;i++) if (a[i] != "") isdev[a[i]]=1 }
			{ if ($2 ~ /^crates\// && !($2 in isdev)) print $1 }' | sed '/^$/d' | sort)
		DROPPEDN=$(comm -23 <(printf '%s\n' "$RAWN") <(printf '%s\n' "$KEPT") || true)
		DROPPED=$(printf '%s\n' "$DROPPEDN" | while IFS= read -r n; do
			[ -z "$n" ] && continue
			printf '%s\n' "$RAW" | awk -F'\t' -v n="$n" '$1==n {print $2"/"n}'
		done)
		report_dropped "9." "$DROPPED" || return 1
		# 哨兵：部署产物 `nous-app` 必须在集合里（排干净之后不能把真的也排掉）
		printf '%s\n' "$KEPT" | grep -qx 'nous-app' \
			|| { echo "哨兵丢了：bin 集合里没有 nous-app（$(printf '%s' "$KEPT" | tr '\n' ' ')）"; return 1; }
		echo "       哨兵：部署产物在集合里 —— nous-app（排完只剩 $(printf '%s\n' "$KEPT" | grep -c .) 个 bin）"
		NBINS=$(printf '%s\n' "$KEPT" | grep -c . || true)
		[ "$NROOT" -eq 1 ] && [ "$NBINS" -eq 1 ] \
			|| { echo "根 Cargo.toml 数=$NROOT（应为 1）；systemd bin 数=$NBINS（应为 1）：$(printf '%s' "$KEPT" | tr '\n' ' ')"; return 1; }
	}

	# 10. 旧仓清空：代码与文档里 'Nous-Package' 为 0（检查者自己那一行不算，content/ 是已死面板的数据）
	c10() {
		local F
		# 排除：检查者自己（它就是那条判据的字符串）；已死面板的 content/ 数据；
		# 词表里的 `_Avoid_:` 行（那是"别用这个词"的清单，不是引用）。
		# spec 的正文不在本仓（`docs/spec/README.md` 只留一句指针），所以不再需要排除 docs/spec/。
		F=$(git grep -n 'Nous-Package' -- ':!content/**' ':!scripts/verify.sh' | grep -v '_Avoid_' || true)
		[ -z "$F" ] || { echo "$F"; return 1; }
	}

	# 11. 发现唯一：生产路径不许出现 NOUS_SERVER_ADDR（只许测试用；注释里提一句不算配置）
	c11() {
		local RAW KEPT DROPPED
		# 扫 `应用层 ∪ dev 层`：dev 层是**信号源**（探针给子进程设环境变量那一行必须在里面），
		# 减掉之后才是"生产路径"。名单全部来自清单，不点名目录。
		RAW=$(git grep -n 'NOUS_SERVER_ADDR' -- $A $D || true)
		KEPT=$(printf '%s\n' "$RAW" | dev_reject | grep -v '/tests/' \
			| grep -vE ':[0-9]+:\s*(//|///|//!)' || true)
		DROPPED=$(comm -23 <(printf '%s\n' "$RAW" | sed '/^$/d' | sort) <(printf '%s\n' "$KEPT" | sed '/^$/d' | sort) || true)
		report_dropped "11." "$DROPPED" || return 1
		# 哨兵：dev 层里那条"测试用配置"必须被命中（排除活着，且排的正是测试用途）
		printf '%s\n' "$DROPPED" | grep -q '^crates/nous/ext-probe/src/server.rs:36:' \
			|| { echo "哨兵丢了：dev 层的 ext-probe/src/server.rs:36 没命中（减 dev 层的规则失效，或探针被挪走）"; return 1; }
		echo "       哨兵：$(printf '%s\n' "$DROPPED" | grep '^crates/nous/ext-probe/src/server.rs:36:' | sed 's/^ *//')"
		[ -z "$KEPT" ] || { echo "$KEPT"; return 1; }
	}

	# 12. 进程只有一处：应用层里 spawn 恰好 1 处，且位于扩展监管（spec 字面）
	c12() {
		local RAW KEPT DROPPED NF
		# 与判据 6 共用一条 pattern：数**构造点**的文件集合，不数 `.spawn()`（`cmd.status()` 也是进程）。
		# 扫 `应用层 ∪ dev 层` 再减 `dev` 层：探针是信号源，不是要放宽的对象；不点名目录。
		RAW=$(grep -rlE 'Command::new\(|::fork\(|posix_spawn\(|execve\(' $A $D --include=*.rs 2>/dev/null | sort || true)
		KEPT=$(printf '%s\n' "$RAW" | dev_reject | grep -vE '(/tests/|tests\.rs$)' | sed '/^$/d' | sort || true)
		DROPPED=$(comm -23 <(printf '%s\n' "$RAW" | sed '/^$/d' | sort) <(printf '%s\n' "$KEPT") || true)
		report_dropped "12." "$DROPPED" || return 1
		# 哨兵：**恰好 1 个文件**，且就是扩展监管的 `crates/nous/ext/src/spawn.rs`
		NF=$(printf '%s\n' "$KEPT" | grep -c . || true)
		if [ "$NF" -ne 1 ] || [ "$(printf '%s\n' "$KEPT")" != "crates/nous/ext/src/spawn.rs" ]; then
			echo "进程创建点不是恰好 1 个文件 == crates/nous/ext/src/spawn.rs（实得 $NF 个）："
			printf '%s\n' "$KEPT" | sed 's/^/       /'
			return 1
		fi
		echo "       哨兵：进程创建点文件 == $(printf '%s\n' "$KEPT")"
	}

	# 13. 无文件锁（Q26：单进程不需要文件锁）—— spec 字面：flock / fs2 / File::lock / `.lock()` 为空
	c13() {
		local RAW KEPT DROPPED L
		RAW=$(git grep -nE 'flock|fs2|File::lock|\.lock\(\)' -- $A || true)
		# 文件锁 = flock / fs2 / File::lock（Q26：单进程不需要文件锁）。
		# `.lock()` 单独统计：spec §6 要求进程内同步原语，`Mutex::lock()` / `RwLock` 是那一类
		# （字段方法名相同），不是文件锁；一起判红会让 §6 与判据 13 互斥。
		KEPT=$(printf '%s\n' "$RAW" | grep -v '/tests/' \
			| grep -E ':[0-9]+:.*(flock|fs2|File::lock)' | grep -v '\.lock\(\)' || true)
		DROPPED=$(comm -23 <(printf '%s\n' "$RAW" | sed '/^$/d' | sort) <(printf '%s\n' "$KEPT" | sed '/^$/d' | sort) || true)
		report_dropped "13." "$DROPPED" || return 1
		L=$(printf '%s\n' "$DROPPED" | grep -c '\.lock\(\)' || true)
		echo "       （其中 $L 行是 \`.lock()\`：接收者全是进程内 Mutex/RwLock/Child，没有 File / fs2 类型）"
		[ -z "$KEPT" ] || { echo "$KEPT"; return 1; }
	}

	judge "1. 正向依赖（nous-app 的包 --depth 1 含 durable/wire/tools/runtime，无 chord/rusqlite）" c1
	judge "2. 反向依赖（内核层 --depth 1 不依赖应用层）" c2
	judge "3. 词表基线（S8：只减不增）" c3
	judge "4. 路由归属（内核层无 .route(/axum::/tower::）" c4
	judge "5. 数据面唯一入口（内核层之外无 rusqlite/Connection::open/.sqlite/PRAGMA/BEGIN IMMEDIATE）" c5
	judge "6. 进程归应用层（内核层零进程 + spawn 文件集合 == baseline）" c6
	judge "6b. 依赖方向（nous-durable --depth 2 无 providers）" c6b
	judge "7. 端点索引唯一（^### GET /v1/ 恰好 wire.md）" c7
	judge "7b. 索引不抄字段（wire.md 无 \"type\": / camelCase 字段清单段落）" c7b
	judge "8. 代码不引文档（内核 .rs 里 wire.md 只在注释里）" c8
	judge "9. 一个 workspace 根 + 进 systemd 的 bin 恰好 1 个" c9
	judge "10. 旧仓清空（代码与文档 0 处 Nous-Package）" c10
	judge "11. 发现唯一（应用层生产路径无 NOUS_SERVER_ADDR）" c11
	judge "12. 进程只有一处（应用层 spawn 恰好 1 处，在扩展监管）" c12
	judge "13. 无文件锁（应用层无 flock/fs2/File::lock/.lock()）" c13

	TOTAL=$((B_GREEN+B_RED+B_SKIP))
	printf '\n  \033[1mspec 第 1 章边界判据：%d 条 —— 绿 %d / 红 %d / SKIP %d\033[0m\n' \
		"$TOTAL" "$B_GREEN" "$B_RED" "$B_SKIP"
fi

printf '\n'
if [ $FAIL -eq 0 ]; then printf '\033[32m通过\033[0m\n'; exit 0; else printf '\033[31m未通过\033[0m\n'; exit 1; fi
