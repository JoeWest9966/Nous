#!/usr/bin/env bash
# 部署 nous-app 到 j1900（issue #52）。
#
#   scripts/deploy.sh                     # 首次安装或升级（幂等）
#   scripts/deploy.sh --uninstall         # 只摘产物/服务，不动数据与 token
#   scripts/deploy.sh --purge --yes-i-mean-it   # 连数据一起删
#   scripts/deploy.sh --rotate-token      # 轮转 /etc/nous/token
#
# 三条硬边界：不碰 4790 / nous-panel / content；不 chown|chgrp /opt/*；脚本里不放凭据。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

HOST=${NOUS_DEPLOY_HOST:-deploy@j1900}
REMOTE_ROOT=/opt/nous-app
DATA_DIR=/var/lib/nous
TOKEN_PATH=/etc/nous/token
TOKEN_MODE=0640
UNIT_SRC=deploy/nous-app.service
UNIT_DST=/etc/systemd/system/nous-app.service
UNIT_NAME=nous-app
LISTEN_ADDR=0.0.0.0:4789
TARGET=x86_64-unknown-linux-musl
OLD_COUNT=2
HEALTH_TRIES=6
HEALTH_WAIT=5
BUILD_DIR=target/x86_64-unknown-linux-musl/release
BIN_LOCAL=$BUILD_DIR/nous-app
TS=$(date -u +%Y%m%dT%H%M%SZ)
MODE=deploy
PURGE_CONFIRM=0

usage() {
	cat <<'EOF'
用法：
  scripts/deploy.sh                          首次安装或升级（同 sha 幂等）
  scripts/deploy.sh --uninstall              停服务 + 摘 unit + 删 /opt/nous-app
  scripts/deploy.sh --purge --yes-i-mean-it  卸载后删 /var/lib/nous + /etc/nous/token
  scripts/deploy.sh --rotate-token           重建 /etc/nous/token（root:nous 0640）
  scripts/deploy.sh --selftest-prune         prune 选择逻辑自测（本地临时目录，不碰远端）
  scripts/deploy.sh --selftest-gate          闸门用哪个产物自测（同上）
EOF
}

die() { printf '✗ %s\n' "$*" >&2; exit 1; }
info() { printf '→ %s\n' "$*"; }

# 首次 SSH/sudo 失败时打印可复制的三行手工设置（D10）。
manual_setup() {
	cat >&2 <<'EOF'
手工设置（在 asus 上跑完再重跑 deploy.sh）：
ssh-keygen -t ed25519 -N ''
ssh-copy-id deploy@j1900
ssh j1900 "sudo useradd --system --home /srv/nous/deploy --shell /bin/bash deploy 2>/dev/null; sudo install -d -m 0755 /etc/sudoers.d; echo 'deploy ALL=(root) NOPASSWD: ALL' | sudo tee /etc/sudoers.d/nous-deploy >/dev/null; sudo chmod 0440 /etc/sudoers.d/nous-deploy"   # 注意 shell 必须是真 shell：nologin 会直接拒掉非交互 ssh
EOF
}

# ssh 把**多个参数**用空格拼成一条命令串发过去，内层引号全丢：
#   ssh host sudo -n -- sh -c 'set -e; echo hi'   →  远端跑的是 `sh -c set`（只 dump 环境）
# 所以这里自己拼一条命令：每个参数单引号包住，内部单引号按 POSIX 的 '\'' 转义。远端是 sh。
q_() { printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"; }
remote() {
	local cmd="" a
	for a in "$@"; do cmd="$cmd$(q_ "$a") "; done
	ssh -o BatchMode=yes "$HOST" "sudo -n -- $cmd"
}
rs() { remote "$@"; }
# 远端 stdout 收集（rsync 是本地进程，单独走 ssh）。
rs_out() { remote "$@"; }

require_local() {
	local c
	for c in ssh cargo sha256sum cut date mktemp jq python3; do
		command -v "$c" >/dev/null 2>&1 || die "本机缺 $c"
	done
	[ -f "$UNIT_SRC" ] || die "缺 $UNIT_SRC"
}

preflight_remote() {
	local out rc=0
	out=$(rs true 2>&1) || rc=$?
	if [ $rc -ne 0 ]; then
		printf '%s\n' "$out" >&2
		manual_setup
		die "SSH/sudo 不通（$HOST）；上面三行是可直接复制的手工设置"
	fi
	# 前置检查：命令、磁盘余量；不碰 nous-panel / 4790 / content。
	out=$(rs sh -c 'set -e; for c in systemctl rsync install ln mv cp rm curl grep sed awk stat chown chmod od jq; do command -v "$c" >/dev/null 2>&1 || { echo "missing:$c"; exit 1; }; done; df -P /opt /var/lib 2>/dev/null | awk "NR>1 && \$5+0 > 90 {print \"disk-full:\" \$6; exit 1}"' 2>&1) || rc=$?
	[ $rc -eq 0 ] && [ -z "$out" ] || die "j1900 前置检查失败：$out"
}

unit_sha() { sha256sum "$UNIT_SRC" | awk '{print $1}'; }

build_binary() {
	info "编译 musl 静态产物（$TARGET）"
	# 要装的是 $TARGET 的 std，不是 `rustc -vV` 里那串 commit 哈希
	# （原来从 `rustc -vV` split 出 `want`，拿到的是 "58cf8f9c3 2026-10-13)\nbinary: rustc…"，
	#  于是每次部署都被自己的守卫挡死，一行都没跑出去）。
	if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
		die "本机缺 rust target $TARGET：rustup target add $TARGET"
	fi
	command -v musl-gcc >/dev/null 2>&1 || die "本机缺 musl-tools（musl-gcc）"
	cargo build --release --target "$TARGET" -p nous-server --bin nous-app || die "编译失败"
	[ -x "$BIN_LOCAL" ] || die "产物不存在：$BIN_LOCAL"
	BIN_SHA=$(sha256sum "$BIN_LOCAL" | cut -c1-8)
	BIN_NAME="nous-app-$BIN_SHA-$TARGET"
	info "产物 $BIN_NAME"
}

# 远端库状态。票面 D9：首次/升级只按 nous.db 分叉，不按产物分叉。
remote_state() {
	local out rc=0
	out=$(rs sh -c 'test -e /var/lib/nous/nous.db && echo db=yes || echo db=no; test -L /opt/nous-app/current && echo current=yes || echo current=no' 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "读远端状态失败：$out"
	DB_PRESENT=$(printf '%s\n' "$out" | sed -n 's/^db=//p')
	CURRENT_PRESENT=$(printf '%s\n' "$out" | sed -n 's/^current=//p')
	if [ "$DB_PRESENT" = yes ]; then
		FIRST_RUN=0
	elif [ "$CURRENT_PRESENT" = yes ]; then
		die "nous.db 不在但 current 在：不猜首次，人工对账后再部署"
	else
		FIRST_RUN=1
	fi
}

local_checks() {
	bash -n scripts/deploy.sh || die "deploy.sh 语法错"
	bash -n scripts/deploy-check.sh || die "deploy-check.sh 语法错"
	# 判据 9b 的新口径（#50 落地后）：绑非回环是**允许**的，但必须成对出现
	# ——`NOUS_LISTEN` 非回环 + `NOUS_TOKEN_FILE` 指向 token；缺一个就拒绝部署，
	# 别等到进程起来 `refuse: no-token-on-non-loopback` 才发现。
	grep -qxF "Environment=NOUS_LISTEN=$LISTEN_ADDR" "$UNIT_SRC" || die "unit 的 NOUS_LISTEN 不是 $LISTEN_ADDR（判据 9b）"
	case "$LISTEN_ADDR" in
	127.0.0.1:*|localhost:*) : ;;
	*) grep -qxF 'Environment=NOUS_TOKEN_FILE=/etc/nous/token' "$UNIT_SRC" \
		|| die "unit 绑了非回环却没有 NOUS_TOKEN_FILE（判据 9b：裸奔不许部署）" ;;
	esac
}

# 票面契约：--check 固定两行 quick_check=ok / schema=<n>。
parse_check() {
	local text=$1
	CHECK_OK=0; CHECK_SCHEMA=""
	CHECK_OK=$(printf '%s\n' "$text" | sed -n 's/^quick_check=//p' | head -1)
	CHECK_SCHEMA=$(printf '%s\n' "$text" | sed -n 's/^schema=//p' | head -1)
	[ "$CHECK_OK" = ok ] || return 1
	printf '%s' "$CHECK_SCHEMA" | grep -qE '^[0-9]+$' || return 1
}

ensure_dirs_and_token() {
	local out rc=0
	# 首次：只建 /srv/nous/work、/var/lib/nous、/etc/nous 和 token；
	# 不建 backups/、不提前建 nous.db（应用首次启动自己建）、不 chown/chgrp /opt/*。
	out=$(rs sh -c 'set -e
# 服务用户由部署脚本负责（unit 写死 User=nous）。缺了就在这里建，幂等。
getent group nous >/dev/null || groupadd --system nous
getent passwd nous >/dev/null || useradd --system --gid nous --home /srv/nous/work --shell /usr/sbin/nologin nous
install -d -m 0750 -o nous -g nous /srv/nous/work
install -d -m 0750 -o nous -g nous /var/lib/nous
install -d -m 0755 /etc/nous
if [ ! -e /etc/nous/token ]; then
  umask 0027
  od -An -N32 -tx1 /dev/urandom | tr -d " \n" | cut -c1-43 > /etc/nous/token
  chown root:nous /etc/nous/token
  chmod 0640 /etc/nous/token
fi
install -d -m 0755 /opt/nous-app' 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "建目录/token 失败：$out"
}

# 单库备份：backups/nous-<ts>.db + nous-<ts>.json（票面 D8）。
backup_and_manifest() {
	local ts=$1 bin=$2 base backup manifest tmp_check out rc=0
	base=/var/lib/nous/backups
	backup=$base/nous-$ts.db
	manifest=$base/nous-$ts.json
	out=$(rs sh -c "set -e; install -d -m 0750 -o nous -g nous '$base'" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "建备份目录失败：$out"

	out=$(rs sh -c "NOUS_DB='$DATA_DIR/nous.db' '$bin' --backup '$backup' >/dev/null" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "备份失败：$out"

	CHECK_TEXT=$(rs_out sh -c "NOUS_DB='$DATA_DIR/nous.db' '$bin' --check '$backup'" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "--check 失败：$CHECK_TEXT"
	parse_check "$CHECK_TEXT" || die "--check 输出不可解析（必须 quick_check=ok + schema=<n>）：$CHECK_TEXT"

	local usha bytes
	usha=$(unit_sha)
	bytes=$(rs_out stat -c %s "$backup" 2>/dev/null || echo 0)
	PREV_MANIFEST=$(rs_out sh -c 'ls -1t /var/lib/nous/backups/nous-*.json 2>/dev/null | head -1' 2>/dev/null || true)
	local predecessor
	predecessor=${PREV_MANIFEST##*/}
	[ -n "$predecessor" ] || predecessor=null
	tmp_check=$(mktemp)
	jq -n \
		--arg binary_sha8 "$BIN_SHA" \
		--arg target "$TARGET" \
		--argjson schema_version "$CHECK_SCHEMA" \
		--arg unit_sha256 "$usha" \
		--arg deployed_at "$(date -u +%FT%TZ)" \
		--arg predecessor "$predecessor" \
		--argjson db_bytes "$bytes" \
		'{binary_sha8:$binary_sha8,target:$target,schema_version:$schema_version,unit_sha256:$unit_sha256,deployed_at:$deployed_at,predecessor:$predecessor,db_bytes:$db_bytes}' >"$tmp_check"
	out=$(rs sh -c "cat > '$manifest.tmp' && chmod 0640 '$manifest.tmp' && mv -f '$manifest.tmp' '$manifest'" <"$tmp_check" 2>&1) || rc=$?
	rm -f "$tmp_check"
	[ $rc -eq 0 ] || die "写 manifest 失败：$out"
	MANIFEST_PATH=$manifest
	BACKUP_DIR=$base
}

# 闸门用哪个二进制：新产物优先，其次 current（卸载过/上传失败时）。两者都没有 → 空（跳过闸门）。
gate_probe() {
	local new=$1 old=$2
	if [ -n "$new" ] && [ -e "$new" ]; then printf '%s' "$new"
	elif [ -n "$old" ] && [ -e "$old" ]; then printf '%s' "$old"
	fi
}

# #91 判据 1b：新产物在 → 必须用新的（不是 current）。
selftest_gate() {
	local tmp rc=0 probe
	tmp=$(mktemp -d) || return 1
	mkdir -p "$tmp/new" "$tmp/old"
	touch "$tmp/new/nous-app" "$tmp/old/nous-app"

	probe=$(gate_probe "$tmp/new/nous-app" "$tmp/old/nous-app")
	[ "$probe" = "$tmp/new/nous-app" ] || { printf '\n✗ 新产物在时闸门却用了 %s\n' "$probe" >&2; rc=1; }

	rm -f "$tmp/new/nous-app"
	probe=$(gate_probe "$tmp/new/nous-app" "$tmp/old/nous-app")
	[ "$probe" = "$tmp/old/nous-app" ] || { printf '\n✗ 新产物缺时闸门应退回 current，实得 %s\n' "$probe" >&2; rc=1; }

	rm -f "$tmp/old/nous-app"
	probe=$(gate_probe "$tmp/new/nous-app" "$tmp/old/nous-app")
	[ -z "$probe" ] || { printf '\n✗ 两个都不在时应跳过闸门，实得 %s\n' "$probe" >&2; rc=1; }

	rm -rf "$tmp"
	[ $rc -eq 0 ] && printf 'probe: 新产物优先 / 缺则 current / 都没有则跳过\n'
	return $rc
}

# 保留**最新** $3 个（按 mtime），删其余。选择逻辑只有这一份：远端 prune 与本地
# `--selftest-prune` 跑的是同一个字符串（#91：上一版按目录名 `sort -r`，刚装好的
# 新目录字典序最大时会被自己删掉，上一版还跟着陪葬）。
prune_command() {
	local dir=$1 pattern=$2 keep=$3
	# `ls -1dt` = 按 mtime 新→旧；`tail -n +N` 从第 N 个开始 = 只留前 keep 个。
	printf 'set -e; ls -1dt %s/%s 2>/dev/null | tail -n +%s | while IFS= read -r p; do rm -rf -- "$p"; done' \
		"$dir" "$pattern" "$((keep + 1))"
}

prune_keep_two() {
	local dir=$1 pattern=$2 out rc=0
	out=$(rs sh -c "$(prune_command "$dir" "$pattern" 2)" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "prune $dir/$pattern 失败：$out"
}

# #91 判据 1：prune 的选择逻辑对「名字顺序 ≠ 时间顺序」必须保最新。
# 四个版本目录 + 一个非匹配文件 + 一个非匹配目录；保 2 个。
# 期望：留下 mtime 最新的两个（04e43eb8 / acb57535），删掉 7a461960 / b75daf6a；
# 按名字排（旧实现）会留下 acb57535 / b75daf6a、删掉 04e43eb8 —— 正好红。
selftest_prune() {
	local tmp rc=0
	tmp=$(mktemp -d) || return 1
	mkdir -p "$tmp/nous-app-7a461960" "$tmp/nous-app-b75daf6a" \
		"$tmp/nous-app-acb57535" "$tmp/nous-app-04e43eb8" "$tmp/other-dir"
	touch "$tmp/nous-app.service.20260101T000000Z"
	# mtime：7a461960 最旧 → 04e43eb8 最新（与名字顺序故意相反）
	touch -t 202601010101 "$tmp/nous-app-7a461960"
	touch -t 202601020202 "$tmp/nous-app-b75daf6a"
	touch -t 202601030303 "$tmp/nous-app-acb57535"
	touch -t 202601040404 "$tmp/nous-app-04e43eb8"

	sh -c "$(prune_command "$tmp" 'nous-app-*' 2)" || rc=1
	local kept removed
	kept=$(ls -1d "$tmp"/nous-app-* 2>/dev/null | xargs -r -n1 basename | sort | tr '\n' ' ')
	removed=$(ls -1d "$tmp"/nous-app-* 2>/dev/null | wc -l)

	if [ "$kept" != "nous-app-04e43eb8 nous-app-acb57535 " ] || [ "$removed" -ne 2 ]; then
		printf '\n✗ prune 选择错了：期望留下 nous-app-04e43eb8 / nous-app-acb57535，实得「%s」（%s 个）\n' "$kept" "$removed" >&2
		rc=1
	fi
	if [ ! -d "$tmp/other-dir" ] || [ ! -f "$tmp/nous-app.service.20260101T000000Z" ]; then
		printf '\n✗ prune 删到了非匹配项（other-dir 或 service 快照不见了）\n' >&2
		rc=1
	fi
	rm -rf "$tmp"
	if [ $rc -eq 0 ]; then
		printf 'kept: nous-app-04e43eb8 nous-app-acb57535\nremoved: nous-app-7a461960 nous-app-b75daf6a\n'
	fi
	return $rc
}

install_unit_and_link() {
	local ts=$1 out rc=0
	out=$(rs sh -c "set -e
chmod 0644 /tmp/nous-app.service.new
mv -f /tmp/nous-app.service.new '$UNIT_DST'
cp '$UNIT_DST' '$REMOTE_ROOT/nous-app.service.$ts'
ln -sfn '$REMOTE_ROOT/$BIN_NAME' '$REMOTE_ROOT/current.tmp'
mv -Tf '$REMOTE_ROOT/current.tmp' '$REMOTE_ROOT/current'
systemctl daemon-reload
systemctl enable nous-app >/dev/null 2>&1 || true
systemctl start nous-app" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "装 unit/切链/启动失败：$out"
}

health_check() {
	local i code build text
	for i in $(seq 1 "$HEALTH_TRIES"); do
		sleep "$HEALTH_WAIT"
		# `#50` 落地后 `/v1/*` 全在认证后面：不带 token 的健康检查是**必然假红**
		# （curl -f 遇 401 直接失败，连 json 都拿不到，看起来像服务没起来）。
		text=$(rs_out sh -c "tok=\$(cat '$TOKEN_PATH' 2>/dev/null || true); if [ -n \"\$tok\" ]; then curl -fsS --max-time 4 -H \"authorization: Bearer \$tok\" http://127.0.0.1:4789/v1/device; else curl -fsS --max-time 4 http://127.0.0.1:4789/v1/device; fi" 2>&1 || true)
		if printf '%s' "$text" | grep -q 'refuse:'; then
			printf '%s\n' "$text" >&2
			return 1
		fi
		build=$(printf '%s' "$text" | jq -r '.build // empty' 2>/dev/null || true)
		[ "$build" = "$BIN_SHA" ] && return 0
		if [ -n "$text" ]; then
			# refuse: 快速失败：journal 里出现就别等满六次。
			local journal
			journal=$(rs_out journalctl -u nous-app -n 50 --no-pager 2>/dev/null || true)
			printf '%s\n' "$journal" | grep -q 'refuse:' && { printf '%s\n' "$journal" >&2; return 1; }
		fi
	done
	local j
	j=$(rs_out journalctl -u nous-app -n 50 --no-pager 2>/dev/null || true)
	printf '%s\n' "$j" >&2
	return 1
}

# 回退：current 指回上一版；有 manifest 就恢复快照与 unit；restart。
rollback() {
	local ts=$1 out rc=0 prev manifest
	# 按**时间**倒序取上一版，不能按名字排序：版本名里是 sha8，字母序和部署顺序毫无关系
	# （j1900 实测：回退把 current 指回了刚刚失败的那个版本，因为它的 sha8 恰好排第二）。
	prev=$(rs_out sh -c "ls -1dt $REMOTE_ROOT/nous-app-*/ 2>/dev/null | sed 's#/\$##' | sed -n '2p'" 2>/dev/null || true)
	if [ -z "$prev" ]; then
		info "没有上一版可切，停在 failed"
		rs systemctl stop nous-app >/dev/null 2>&1 || true
		return 1
	fi
	manifest=$(rs_out sh -c "ls -1t /var/lib/nous/backups/*.json 2>/dev/null | sed -n '2p'" 2>/dev/null || true)
	info "回退到 ${prev##*/}"
	out=$(rs sh -c "set -e
ln -sfn '$prev' '$REMOTE_ROOT/current.tmp'
mv -Tf '$REMOTE_ROOT/current.tmp' '$REMOTE_ROOT/current'
if [ -n '$manifest' ] && [ -f '$manifest' ]; then
  m=\$(cat '$manifest')
  b=\$(printf '%s' \"\$m\" | jq -r .predecessor)
  if [ \"\$b\" != null ] && [ -f '/var/lib/nous/backups/\$b' ]; then
    cp '/var/lib/nous/backups/\$b' /var/lib/nous/nous.db.tmp
    mv -f /var/lib/nous/nous.db.tmp /var/lib/nous/nous.db
  fi
fi
# 单元快照按部署时间戳命名：回退时恢复上一次部署留下的 unit（带 78 语义）。
snapshot=\$(ls -1t '$REMOTE_ROOT'/nous-app.service.* 2>/dev/null | sed -n '2p')
if [ -n "\$snapshot" ] && [ -f "\$snapshot" ]; then
  cp "\$snapshot" '$UNIT_DST'
fi
systemctl daemon-reload
systemctl restart nous-app" 2>&1) || rc=$?
	[ $rc -eq 0 ] || { info "回退启动失败：$out"; return 1; }
	# 回退后做一次轻量健康检查；失败就停在 failed，不回退第二层。
	local j
	j=$(rs_out sh -c "tok=\$(cat '$TOKEN_PATH' 2>/dev/null || true); if [ -n \"\$tok\" ]; then curl -fsS --max-time 4 -H \"authorization: Bearer \$tok\" http://127.0.0.1:4789/v1/device; else curl -fsS --max-time 4 http://127.0.0.1:4789/v1/device; fi" 2>&1 || true)
	if ! printf '%s' "$j" | grep -q 'refuse:'; then
		return 0
	fi
	info "回退后仍 refuse：$(printf '%s' "$j" | head -2)"
	return 1
}

failed_log() {
	local ts=$1 out rc=0
	out=$(rs sh -c "install -d -m 0750 -o nous -g nous /var/lib/nous/backups; journalctl -u nous-app -n 200 --no-pager > /var/lib/nous/backups/failed-$ts.log 2>&1; chmod 0640 /var/lib/nous/backups/failed-$ts.log" 2>&1) || rc=$?
	[ $rc -eq 0 ] || info "写 failed 日志失败：$out"
	info "journal 尾部已存 /var/lib/nous/backups/failed-$ts.log"
}

# 把产物与单元推上去（只落到版本目录，**不**切链、不启动）。
# 之所以单独一步、且在「升级」之前：库在、current 不在（`--uninstall` 之后）
# 时没有远端旧产物可用来做 `--active-tasks` / `--backup`，只能拿新产物跑这两件事。
upload_artifacts() {
	local stage
	stage=$(mktemp -d)
	cp "$BIN_LOCAL" "$stage/$BIN_NAME"
	cp "$UNIT_SRC" "$stage/nous-app.service.new"
	rsync -a -e "ssh -o BatchMode=yes" "$stage/$BIN_NAME" "$HOST:/tmp/$BIN_NAME" || die "rsync 产物失败"
	rsync -a -e "ssh -o BatchMode=yes" "$stage/nous-app.service.new" "$HOST:/tmp/nous-app.service.new" || die "rsync unit 失败"
	rm -rf "$stage"
	# 版本是一**目录**（`nous-app-<sha8>-<target>/nous-app`），`current` 指向目录：
	# unit 的 ExecStart 是 `/opt/nous-app/current/nous-app`，回退逻辑也是按
	# `nous-app-*/nous-app` 找上一版。把产物落成裸文件，`current` 就成了指向文件的链接，
	# 起服务必然 203/EXEC（Not a directory）。
	rs sh -c "set -e; install -d -m 0755 '$REMOTE_ROOT'; install -d -m 0755 '$REMOTE_ROOT/$BIN_NAME'; install -m 0755 '/tmp/$BIN_NAME' '$REMOTE_ROOT/$BIN_NAME/nous-app'; rm -f '/tmp/$BIN_NAME'" || die "落产物失败"
}

do_deploy() {
	local_checks
	preflight_remote
	build_binary
	# 幂等：同 sha 且 current 已指过去 → 零副作用（数据/产物树都不动）。
	local cur unit_remote
	cur=$(rs_out readlink /opt/nous-app/current 2>/dev/null || true)
	# 幂等判据是「产物 + 单元」两样都一样，**不是只看产物**：
	# 只改 unit（例如把监听从回环改成 0.0.0.0、把 RestartPreventExitStatus 加上）时，
	# 二进制 sha 不变，只看二进制就会静默什么都不做 —— 实测踩到：改了 unit 跑 deploy，
	# 它说「同 sha 已部署」然后退出，远端还是老 unit。
	unit_remote=$(rs_out sha256sum "$UNIT_DST" 2>/dev/null | awk '{print $1}' || true)
	if [ "${cur##*/}" = "$BIN_NAME" ] && [ "$unit_remote" = "$(unit_sha)" ]; then
		info "同 sha（$BIN_SHA）且 unit 未变，current 不变（幂等）"
		exit 0
	fi
	remote_state
	ensure_dirs_and_token
	upload_artifacts

	if [ "$FIRST_RUN" -eq 0 ]; then
		info "升级：stop → active-tasks → backup → check"
		# `--uninstall` 之后 unit 不在（数据还在），这条命令必须容错：
		# 否则「卸载 → 再部署」这个正常流程会死在 stop 上。
		rs sh -c "systemctl stop nous-app 2>/dev/null || true" || die "stop 失败"
		local bin_old tasks
		bin_old=$(rs_out readlink -f /opt/nous-app/current/nous-app 2>/dev/null || true)
		# 闸门必须用**即将上场的产物**数任务：用旧二进制数 = 改口径那一版永远上不去
		# （2026-11-22 实测：新版数出 0，旧版数出 7805，闸门信了旧的）。产物不在才退回落 current。
		local probe
		probe=$(gate_probe "$REMOTE_ROOT/$BIN_NAME/nous-app" "$bin_old")
		if [ -n "$probe" ]; then
			# `NOUS_DB` 必须显式给：unit 里那是 `Environment=`，ssh 里没有，默认 `./nous.db` 会打不开。
			tasks=$(rs_out sh -c "NOUS_DB='$DATA_DIR/nous.db' '$probe' --active-tasks" 2>&1) || die "--active-tasks 失败：$tasks"
			if printf '%s' "$tasks" | grep -qE 'task|conversation'; then
				printf '%s\n' "$tasks" >&2
				rs systemctl start nous-app >/dev/null 2>&1 || true
				die "有活跃任务，拒绝升级（task id + conversation id 见上）"
			fi
			# 闸门口径（#91）：只数在飞的任务（pending/running/waiting/completing）；
			# `sleeping` 是触发器排队，不算在跑 —— 按 "非 terminal" 数会让升级永久被拒。
			if printf '%s' "$tasks" | grep -qE '^[1-9][0-9]*$'; then
				rs systemctl start nous-app >/dev/null 2>&1 || true
				die "有 $tasks 个在飞的任务（pending/running/waiting/completing），拒绝升级"
			fi
		fi
		backup_and_manifest "$TS" "${bin_old:-$REMOTE_ROOT/$BIN_NAME/nous-app}"
	fi

	install_unit_and_link "$TS"
	if health_check; then
		prune_keep_two "$REMOTE_ROOT" 'nous-app-*'
		[ "$FIRST_RUN" -eq 0 ] && prune_keep_two /var/lib/nous/backups 'nous-*.json'
		[ "$FIRST_RUN" -eq 0 ] && prune_keep_two /var/lib/nous/backups 'nous-*.db'
		info "部署完成（$BIN_NAME，sha8=$BIN_SHA）"
		exit 0
	fi

	info "健康检查失败 → 自动回退"
	if rollback "$TS"; then
		info "已回退到上一版；本次失败"
		exit 1
	fi
	failed_log "$TS"
	die "回退也起不来：停在 failed，见 /var/lib/nous/backups/failed-$TS.log"
}

do_uninstall() {
	preflight_remote || true
	local out rc=0
	out=$(rs sh -c 'set -e
systemctl stop nous-app 2>/dev/null || true
systemctl disable nous-app 2>/dev/null || true
rm -f /etc/systemd/system/nous-app.service
systemctl daemon-reload
rm -rf /opt/nous-app' 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "uninstall 失败：$out"
	# 数据目录与 token 一个字节不动。
	info "已卸载产物与服务；/var/lib/nous 与 /etc/nous/token 未动"
}

do_purge() {
	[ "$PURGE_CONFIRM" -eq 1 ] || die "--purge 必须显式带 --yes-i-mean-it（不接受 -y）"
	do_uninstall
	local out rc=0
	out=$(rs sh -c "set -e; rm -rf '$DATA_DIR'; rm -f '$TOKEN_PATH'" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "purge 失败：$out"
	info "已删 $DATA_DIR 与 $TOKEN_PATH"
}

do_rotate_token() {
	preflight_remote
	local out rc=0
	out=$(rs sh -c "set -e
install -d -m 0755 /etc/nous
umask 0027
od -An -N32 -tx1 /dev/urandom | tr -d ' \n' | cut -c1-43 > '$TOKEN_PATH.tmp'
chown root:nous '$TOKEN_PATH.tmp'
chmod '$TOKEN_MODE' '$TOKEN_PATH.tmp'
mv -f '$TOKEN_PATH.tmp' '$TOKEN_PATH'
systemctl restart nous-app 2>/dev/null || true" 2>&1) || rc=$?
	[ $rc -eq 0 ] || die "rotate-token 失败：$out"
	info "token 已轮转（只回显路径，不回显内容）：$TOKEN_PATH"
}

while [ $# -gt 0 ]; do
	case "$1" in
	--uninstall) MODE=uninstall ;;
	--purge) MODE=purge ;;
	--yes-i-mean-it) PURGE_CONFIRM=1 ;;
	--rotate-token) MODE=rotate-token ;;
	--selftest-prune) MODE=selftest-prune ;;
	--selftest-gate) MODE=selftest-gate ;;
	-h|--help) usage; exit 0 ;;
	*) die "未知参数：$1（--help 看用法）" ;;
	esac
	shift
done

case "$MODE" in
selftest-prune) selftest_prune; exit $? ;;
selftest-gate) selftest_gate; exit $? ;;
deploy) do_deploy ;;
uninstall) do_uninstall ;;
purge) do_purge ;;
rotate-token) do_rotate_token ;;
esac
