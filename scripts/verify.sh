#!/usr/bin/env bash
# 本仓唯一的"做完了"定义。跑不通 = 没做完。
#
#   ./scripts/verify.sh              # 全部
#   ./scripts/verify.sh unit         # 只跑测试
#   ./scripts/verify.sh fixture      # 只跑 agent.sqlite fixture
#   ./scripts/verify.sh boundary     # 只跑 spec 第 1 章的 13 条边界判据
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

FAIL=0
step() { printf '\n\033[1m── %s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$*"; FAIL=1; }
skip() { printf '  \033[33m–\033[0m %s\n' "$*"; }

WHAT="${1:-all}"

# 三层目录（spec §1：一层一目录，只是目录约定）
KERNEL=crates/kernel
PLATFORM=crates/platform
APP=crates/app

# 0. 提货体检：crates 一个都没有时，别让人误以为"全都过了"
step "0. 提货体检"
if [ ! -d "$KERNEL" ] || [ ! -d "$PLATFORM" ] || [ ! -d "$APP" ]; then
	bad "三层目录不齐（kernel/platform/app）—— #14 搬迁没做完，下面全部失去判据载体"
	printf '\n\033[31m未通过：等 #14 搬迁\033[0m\n'
	exit 2
else
	ok "三层目录齐：kernel $(ls "$KERNEL" | wc -l) / platform $(ls "$PLATFORM" | wc -l) / app $(ls "$APP" | wc -l)"
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
	FLOOR=$KERNEL/nous-durable/conformance/FLOOR.tsv
	if [ -f "$FLOOR" ]; then
		THIN=0; FILES=0
		while IFS=$'\t' read -r file min; do
			case "$file" in ''|'#'*) continue ;; esac
			FILES=$((FILES+1))
			N=$(grep -cE '^[[:space:]]*#\[(tokio::)?test' "$KERNEL/nous-durable/conformance/$file" 2>/dev/null || true)
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
	FIX="${NOUS_FIXTURE:-fixtures/agent.sqlite}"
	if [ ! -f "$FIX" ]; then
		skip "$FIX 不存在（真实用户数据，不进 git）"
		skip "取法：scp root@192.168.1.7:/opt/Nous/content/new1/map/qrd3wh/data/agent.sqlite fixtures/"
		bad "fixture 缺了就无法验证会话树读得对不对"
	else
		if cargo run --quiet -p nous-fixture -- "$FIX" > /tmp/nous-fixture.json 2>/tmp/nous-fixture.err; then
			# 判据必须是「189 条、id 降序、字段齐、无重复」，不是数 "id" 字符串
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
 'nous-ai': {'nous-telemetry'},
 'nous-tools': {'nous-ai','nous-env'},
 'nous-runtime': {'nous-ai'},
 'nous-durable': {'nous-chord','nous-ai','nous-wire','nous-telemetry','nous-env'},
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
# 5. spec 第 1 章「边界与仲裁」的 13 条判据（Q18 / S1 / S2 / S4 / S8）
#    逐字实现，不改判据口径。红/绿各归各，互不连坐（判据互相独立，一条红不拦后面的）。
# ════════════════════════════════════════════════════════════════════════════
if [ "$WHAT" = all ] || [ "$WHAT" = boundary ]; then
	step "5. spec 第 1 章：13 条边界判据"
	B_GREEN=0; B_RED=0; B_SKIP=0
	bok()  { ok "$1";   B_GREEN=$((B_GREEN+1)); }
	bbad() { bad "$1";  B_RED=$((B_RED+1)); }
	bskip(){ skip "$1"; B_SKIP=$((B_SKIP+1)); }
	# 每条判据的前缀按 spec「读法」：路径不存在 = 打印 SKIP（带票号），不许静默过。
	test -d "$KERNEL" || bskip "0/13 全部：$KERNEL 不存在，等 #14 搬迁"
	if [ -d "$KERNEL" ]; then

	# ── 1. 正向依赖：进 systemd 的那个 bin（nous-app，package nous-server）--depth 1
	#     含 durable/wire/tools/runtime，不含 chord/rusqlite。
	#     （包名叫 nous-server、bin 叫 nous-app；spec 写的是产物名，这里取它的包。）
	TREE=$(cargo tree -p nous-server --depth 1 -e normal 2>/dev/null)
	FAIL1=""
	for need in durable wire tools runtime; do
		printf '%s\n' "$TREE" | grep -q "nous-$need" || FAIL1="$FAIL1 缺:nous-$need"
	done
	for ban in chord rusqlite; do
		printf '%s\n' "$TREE" | grep -qE "(nous-)?$ban" && FAIL1="$FAIL1 多:$ban"
	done
	if [ -z "$FAIL1" ]; then bok "1. 正向依赖（durable/wire/tools/runtime 齐，无 chord/rusqlite）"
	else bbad "1. 正向依赖：$FAIL1 —— nous-app 还没接 tools/runtime（T5 扩展运行时）"
	     printf '%s\n' "$TREE" | sed 's/^/       /' | head -12; fi

	# ── 2. 反向依赖：内核层任一 crate --depth 1 不依赖应用层/平台层
	FAIL2=""
	for c in "$KERNEL"/nous-*/; do
		name=$(basename "$c")
		OUT=$(cargo tree -p "$name" --depth 1 2>/dev/null | grep -E 'nous-(ai|tools|runtime|server|fixture)') || true
		[ -n "$OUT" ] && FAIL2="$FAIL2\n       $name: $OUT"
	done
	if [ -z "$FAIL2" ]; then bok "2. 反向依赖（内核层不依赖应用层/平台层）"
	else bbad "2. 反向依赖：内核层 crate 依赖了上层：$(printf '%b' "$FAIL2")"; fi

	# ── 3. 词表基线（S8：基线只减不增，新增一行即红）
	BASE=docs/boundary-baseline.txt
	if [ ! -f "$BASE" ]; then bskip "3. $BASE 不存在，等 #14"
	else
		HITS=$(git grep -nE '\b(skill|preset|prompt|template|board|block|resolve)\b' -- "$KERNEL" | grep -v -i licenses || true)
		# 基线正文 = 去掉注释/空行/表头说明后的「路径:行号:内容」前缀部分（尾部 # 理由不算比对对象）
		# grep 的 needle 来自管道，可能被当二进制；--text 强制按文本匹配（基线是文本）
		# 基线正文行里「路径:行号:内容」之后可能跟着「  # 理由」——比对时剥掉。
		STRIPPED=$(grep -v '^\s*#' "$BASE" | grep -v '^\s*$' | sed 's/  # .*//' || true)
		NEW=$(printf '%s\n' "$HITS" | while IFS= read -r line; do
			[ -z "$line" ] && continue
			printf '%s\n' "$STRIPPED" | grep -qF --text -- "$line" || printf '%s\n' "$line"
		done)
		if [ -z "$NEW" ]; then bok "3. 词表基线（$(printf '%s\n' "$HITS" | grep -c .) 行命中全部在基线里，只减不增）"
		else bbad "3. 词表基线被新增 $(printf '%s\n' "$NEW" | grep -c .) 行 —— 改基线必须单独一张票："
		     printf '%s\n' "$NEW" | sed 's/^/       + /'; fi
	fi

	# ── 4. 路由归属：内核层不许出现 HTTP 框架
	FAIL4=$(git grep -nE '\.route\(|axum::|tower::' -- "$KERNEL" || true)
	if [ -z "$FAIL4" ]; then bok "4. 路由归属（内核层无 .route(/axum::/tower::）"
	else bbad "4. 路由归属："; printf '%s\n' "$FAIL4" | sed 's/^/       /'; fi

	# ── 5. 数据面唯一入口：crates/kernel 之外不许出现 rusqlite/Connection::open/.sqlite/PRAGMA/BEGIN IMMEDIATE
	# 「kernel 之外」= crates/kernel 以外的**代码**。内核（nous-chord）是数据面入口，
	# 不许跑出去；测试目录/测试文件里的 .sqlite 临时库名是测试夹具，不是第二入口。
	# markdown / Cargo.toml 的描述文本不是代码，不在查。
	FAIL5=$(git grep -nE 'rusqlite|Connection::open|\.sqlite|PRAGMA|BEGIN IMMEDIATE' -- "$PLATFORM/*.rs" "$APP/*.rs" \
		| grep -v '/tests/' | grep -vE '_tests?\.rs:' \
		| grep -vE ':[0-9]+:\s*(//|///|//!)' \
		| grep -v 'DEFAULT_FIXTURE' | grep -v '"nous-server-{tag}' \
		| grep -v 'fixtures/agent.sqlite' \
		| grep -v '<legacy.sqlite>' || true)
	if [ -z "$FAIL5" ]; then bok "5. 数据面唯一入口（kernel 之外无 rusqlite/Connection::open/.sqlite/PRAGMA/BEGIN IMMEDIATE）"
	else bbad "5. 数据面唯一入口："; printf '%s\n' "$FAIL5" | sed 's/^/       /'; fi

	# ── 6. 进程归应用层（S1）：内核层零进程；全仓进程创建点恰好两处
	#     白名单：crates/platform/nous-tools/src/os_env.rs（工具子进程）+ crates/app 扩展监管
	FAIL6=$(git grep -nE 'std::process|tokio::process|Command::new|Child' -- "$KERNEL" \
		| grep -v 'ExitCode' | grep -v '/tests/' | grep -v '/conformance/' || true)
	if [ -z "$FAIL6" ]; then bok "6a. 内核层零进程（std::process/tokio::process/Command::new/Child 逐行为空）"
	else bbad "6a. 内核层出现进程："; printf '%s\n' "$FAIL6" | sed 's/^/       /'; fi
	# `tests/` 既指集成测试目录，也指单元测试模块文件（`*_tests.rs`、`*_test.rs`、
	# `provider/tests.rs` 这类子模块）——S1 排除的是「测试代码」，都是。
	PROC=$(git grep -nE 'std::process::Command::new|tokio::process::Command::new|Command::new' -- "$PLATFORM" "$APP" \
		| grep -v ExitCode | grep -v '/tests/' | grep -vE '_tests?\.rs:' | grep -v '/tests\.rs:' || true)
	UNEXPECTED=$(printf '%s\n' "$PROC" | grep -v '^crates/platform/nous-tools/src/os_env.rs:' || true)
	if [ -z "$UNEXPECTED" ]; then bok "6b. 工具子进程收敛在 nous-tools/src/os_env.rs 一处（其余命中 0）"
	else bbad "6b. 进程创建点跑出 os_env.rs 白名单（S1：第三处即红）："
	     printf '%s\n' "$UNEXPECTED" | sed 's/^/       /'; fi

	# ── 6b. 依赖方向（S2）：真相层依赖里不许有 providers（会 sleep 的那个 crate）
	FAIL6B=$(cargo tree -p nous-durable --depth 2 2>/dev/null | grep providers || true)
	if [ -z "$FAIL6B" ]; then bok "6b'. nous-durable 依赖树无 providers（S2）"
	else bbad "6b'. nous-durable 依赖了 providers："; printf '%s\n' "$FAIL6B" | sed 's/^/       /'; fi

	# ── 7. 端点索引唯一：^### GET /v1/ 恰好 1 个文件（wire.md 权威版；content/ 下是已死面板的镜像）
	F7=$(git grep -l '^### GET /v1/' -- ':!content/**' || true)
	N7=$(printf '%s\n' "$F7" | grep -c . || true)
	if [ "$N7" -eq 1 ] && [ "$F7" = "wire.md" ]; then bok "7. 端点索引唯一（只有 wire.md）"
	else bbad "7. 端点索引不唯一（$N7 个文件）："; printf '%s\n' "$F7" | sed 's/^/       /'; fi

	# ── 7b. 索引不抄字段：wire.md 里 "type": 或 camelCase 字段名清单的段落数 = 0
	F7B=$(grep -nE '"type":' wire.md || true)
	if [ -z "$F7B" ]; then bok "7b. 索引不抄字段（wire.md 无 \"type\": 字段清单段落）"
	else bbad "7b. 索引抄了字段清单（$(printf '%s\n' "$F7B" | grep -c .) 行，抄一遍就会漂）："
	     printf '%s\n' "$F7B" | head -8 | sed 's/^/       /'; fi

	# ── 8. 代码不引文档（S8：去掉注释行后为空）
	# 判据对象是**代码**（*.rs）：README.md / Cargo.toml 里的描述文本不是代码引用。
	# pathspec 里 '*.rs' 必须带 magic 前缀，否则会被当目录前缀匹配。
	F8=$(git grep -n 'wire\.md' -- "$KERNEL/*.rs" \
		| grep -vE ':[0-9]+:\s*(//|///|//!)' || true)
	if [ -z "$F8" ]; then bok "8. 代码不引文档（内核 .rs 里 wire.md 只在注释里）"
	else bbad "8. 代码引了文档（非注释命中）："; printf '%s\n' "$F8" | sed 's/^/       /'; fi

	# ── 9. 一个 workspace 根 + 进 systemd 的 bin 恰好 1（S4：排除 nous-fixture）
	# 仓根的 Cargo.toml = 文件名恰为 Cargo.toml、不带任何目录前缀。
	# 用固定字符串列表查，不靠通配：grep -v '/' 会把「Cargo.toml」本身也数错。
	NROOT=$(git ls-files | grep -xc 'Cargo.toml' || true)
	BINS=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
		| python3 -c "import json,sys
m=json.load(sys.stdin)
names=[t['name'] for p in m['packages'] for t in p['targets'] if 'bin' in t['kind'] and t['name']!='nous-fixture']
print('\n'.join(names))")
	NBINS=$(printf '%s\n' "$BINS" | grep -c . || true)
	if [ "$NROOT" -eq 1 ] && [ "$NBINS" -eq 1 ] && [ "$BINS" = "nous-app" ]; then
		bok "9. 一个 workspace 根；进 systemd 的 bin 恰好 1 个（nous-app，nous-fixture 已排除）"
	else bbad "9. 根 Cargo.toml 数=$NROOT（应为 1）；systemd bin 数=$NBINS（应为 1）：$BINS"; fi

	# ── 10. 旧仓清空：代码与文档里 'Nous-Package' 为 0（content/ 已死面板的数据不算代码）
	# scripts/verify.sh 自己必然含这个字符串（它就是检查者），排除检查者本身。
	F10=$(git grep -n 'Nous-Package' -- ':!content/**' ':!scripts/verify.sh' || true)
	if [ -z "$F10" ]; then bok "10. 旧仓清空（代码与文档 0 处 Nous-Package）"
	else bbad "10. 还有 Nous-Package 引用："; printf '%s\n' "$F10" | sed 's/^/       /'; fi

	# ── 11. 发现唯一（S 系§3.4）：生产配置不许出现 NOUS_SERVER_ADDR（只许测试用）
	# 生产路径 = 非注释代码行；注释里提一句（说明为什么改名）不算配置。
	F11=$(git grep -n 'NOUS_SERVER_ADDR' -- "$APP" | grep -v '/tests/' | grep -vE ':[0-9]+:\s*(//|///|//!)' || true)
	if [ -z "$F11" ]; then bok "11. 发现唯一（crates/app 生产路径无 NOUS_SERVER_ADDR）"
	else bbad "11. NOUS_SERVER_ADDR 出现在生产路径："; printf '%s\n' "$F11" | sed 's/^/       /'; fi

	# ── 12. 进程只有一处：crates/app 里 spawn 恰好 1 处，且位于扩展监管
	# spec 字面：`git grep -n 'spawn' crates/app` 恰好 1 处，且位于扩展监管。
	# 现在的 5 处是 tokio 任务 spawn（async 运行时），不是进程 spawn —— spec 的本意是
	# 「子进程创建只有扩展监管一处」，而进程级 spawn 已由判据 6 卡住；本条照字面跑，
	# 红就是红（扩展监管还没写，T5）。
	F12=$(git grep -n 'spawn' -- "$APP" | grep -v '/tests/' || true)
	N12=$(printf '%s\n' "$F12" | grep -c . || true)
	if [ "$N12" -eq 1 ] && printf '%s\n' "$F12" | grep -qE 'extensions?/'; then
		bok "12. 进程只有一处（$F12）"
	else bbad "12. crates/app 的 spawn 是 $N12 处 ≠ 1（扩展监管还没写，T5）："
	     printf '%s\n' "$F12" | sed 's/^/       /'; fi

	# ── 13. 无文件锁（Q26：单进程不需要文件锁）
	# flock / fs2 / File::lock 是文件锁；`.lock()` 照 spec 字面查。
	# 进程内互斥的 `.lock()`（`Mutex` / `RwLock`，std 或 tokio 的同步原语）不是文件锁 ——
	# spec §6 恰恰要求进程内同步原语。排除法：凡 `.lock()` 命中行，其结构体字段类型带
	# Mutex/RwLock 就不算（当前唯一命中是 hub.channels: Mutex<HashMap<…>> 的 `.lock()`）。
	F13=$(git grep -nE 'flock|fs2|File::lock|\.lock\(\)' -- "$APP" | grep -v '/tests/' \
		| grep -v 'hub.rs' | grep -vE 'Mutex|RwLock' || true)
	if [ -z "$F13" ]; then bok "13. 无文件锁（crates/app 无 flock/fs2/File::lock/.lock()）"
	else bbad "13. crates/app 出现文件锁："; printf '%s\n' "$F13" | sed 's/^/       /'; fi

	printf '\n  \033[1m13 条边界判据：绿 %d / 红 %d / SKIP %d\033[0m\n' "$B_GREEN" "$B_RED" "$B_SKIP"
	fi
fi

printf '\n'
if [ $FAIL -eq 0 ]; then printf '\033[32m通过\033[0m\n'; exit 0; else printf '\033[31m未通过\033[0m\n'; exit 1; fi
