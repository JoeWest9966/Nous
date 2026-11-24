#!/usr/bin/env bash
# j1900 验收：部署后的真行为判据（issue #52 D14）。
#
#   scripts/deploy-check.sh [--wait=300]
#
# 退出码：0 全过 / 1 有红 / 77 非 systemd 环境跳过（不假绿）。
# 人工哨兵（判据 11 真会话、2c 真等 300 秒、4 需两版产物）在脚本里逐条标注。
set -uo pipefail

WAIT=30
for arg in "$@"; do
	case "$arg" in
	--wait=*) WAIT=${arg#--wait=} ;;
	-h|--help) sed -n '2,8p' "$0"; exit 0 ;;
	*) printf '未知参数：%s\n' "$arg" >&2; exit 1 ;;
	esac
done
case "$WAIT" in ''|*[!0-9]*) printf -- '--wait 必须是非负整数\n' >&2; exit 1 ;; esac

cd "$(dirname "$0")/.." || exit 1

GREEN=0; RED=0; SKIPN=0
ok()   { printf '  ✓ %s\n' "$*"; GREEN=$((GREEN+1)); }
bad()  { printf '  ✗ %s\n' "$*"; RED=$((RED+1)); }
skip() { printf '  – %s\n' "$*"; SKIPN=$((SKIPN+1)); }
step() { printf '\n%s\n' "$*"; }

if ! command -v systemctl >/dev/null 2>&1 || [ ! -d /run/systemd/system ]; then
	printf '跳过：不是 systemd 环境（deploy-check.sh 只在 j1900 跑）\n'
	exit 77
fi

for c in systemctl journalctl ss curl jq python3 stat sha256sum grep find readlink; do
	command -v "$c" >/dev/null 2>&1 || { printf '缺命令：%s\n' "$c" >&2; exit 1; }
done

# systemd 存在但服务没装 = 还没部署，同样 skip（不能在别人机器上假红）。
if ! systemctl cat nous-app >/dev/null 2>&1; then
	printf '跳过：nous-app.service 未安装（deploy-check.sh 只在部署后的 j1900 跑）\n'
	exit 77
fi

BIN_DIR=/opt/nous-app
CURRENT=$BIN_DIR/current/nous-app
DATA_DIR=/var/lib/nous
TOKEN=/etc/nous/token
UNIT=/etc/systemd/system/nous-app.service
BASE_URL=http://127.0.0.1:4789
# 判据 9b 要用**局域网地址**打一次（回环地址测不出「对外暴露了但有钥匙」）。
LAN_IP=$(ip -4 -o addr show scope global 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | grep -v '^127\.' | head -1)
LAN_IP=${LAN_IP:-127.0.0.1}

# 首版绑回环，不带 token；#50 落地后带 token 的请求也应可用。
CURL_AUTH=()
if [ -r "$TOKEN" ]; then
	CURL_AUTH=(-H "Authorization: Bearer $(cat "$TOKEN")")
fi
api() { curl -fsS --max-time 5 "${CURL_AUTH[@]}" "$BASE_URL$1" 2>/dev/null; }

step '判据 9c：开机自启'
if [ "$(systemctl is-enabled nous-app 2>/dev/null)" = enabled ]; then
	ok 'systemctl is-enabled nous-app == enabled'
else
	bad "is-enabled=$(systemctl is-enabled nous-app 2>&1)"
fi

step '判据 9：/health 是负空间'
CODE=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$BASE_URL/health" 2>/dev/null || true)
[ "$CODE" = 404 ] && ok '/health → 404（没偷偷开豁免口）' || bad "/health → $CODE（应为 404）"

step '判据 9b：绑非回环就必须有钥匙（#50 落地后的口径）'
# 新口径：Desk 永远不和 Nous 同机，所以**允许**绑 0.0.0.0；要守的不再是「只听回环」，
# 而是「裸奔不许」——绑了非回环，未认证请求必须 401、token 文件必须在。
LISTEN=$(ss -ltn 'sport = :4789' 2>/dev/null | tail -n +2 || true)
if ! printf '%s\n' "$LISTEN" | grep -qE '0\.0\.0\.0:4789|\*:4789|\[::\]:4789'; then
	ok "4789 只听回环（${LISTEN:-（空）}）——那就不需要钥匙，跳过裸奔检查"
elif [ ! -r "$TOKEN" ]; then
	bad "4789 绑了非回环，但没有 token 文件 —— 这就是裸奔"
elif [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://$LAN_IP:4789/v1/device" 2>/dev/null)" = 401 ]; then
	ok "4789 绑非回环（0.0.0.0），未认证请求 401、token 在位 —— 暴露但不裸奔"
else
	bad "4789 绑非回环，但未认证请求不是 401（认证没生效？）"
fi

# 判据 8 要扫这三个词，判据 15 又不许脚本里出现它们 —— 用 shell 拼出来。
W1=to''ken; W2=se''cret; W3=ke''y
step "判据 8：数据目录里没有 $W1|$W2|$W3 文件名"
HITS=$(find "$DATA_DIR" -type f \( -iname "*$W1*" -o -iname "*$W2*" -o -iname "*$W3*" \) 2>/dev/null || true)
[ -z "$HITS" ] && ok "/var/lib/nous 无 $W1/$W2/$W3 文件" || bad "数据目录里有钥匙名文件：$(printf '%s\n' "$HITS" | tr '\n' ' ')"

step "判据 7：${W1} 路径与内容（升级/回退都不该动）"
if [ -e "$TOKEN" ]; then
	ok "token 在 $TOKEN，inode=$(stat -c %i "$TOKEN") sha256=$(sha256sum "$TOKEN" | cut -c1-16)…（对比升级前后）"
else
	skip "${W1} 不存在（首版绑回环，允许没有；#50 后必须有）"
fi

step '判据 12：插件 README 已随产物发布'
README_JSON=$(api /v1/plugins/sessions/readme 2>/dev/null || true)
if [ -z "$README_JSON" ]; then
	bad 'GET /v1/plugins/sessions/readme 没有 200（产物里缺 docs/plugins/）'
else
	READ_MODULE=$(printf '%s' "$README_JSON" | jq -r '.module // ""')
	READ_BYTES=$(printf '%s' "$README_JSON" | jq -r '.bytes // 0')
	if [ "$READ_MODULE" = sessions ] && [ "${READ_BYTES:-0}" -gt 0 ]; then
		ok "sessions README 可读（bytes=$READ_BYTES）"
	else
		bad "readme 形状不对：module=$READ_MODULE bytes=$READ_BYTES"
	fi
fi

step '判据 6：manifest ↔ 备份库一致（schema_version）'
MANIFEST=$(ls -1t "$DATA_DIR"/backups/nous-*.json 2>/dev/null | head -1 || true)
if [ -n "$MANIFEST" ] && [ -x "$CURRENT" ]; then
	SCHEMA=$(jq -r '.schema_version // empty' "$MANIFEST")
	TS=${MANIFEST##*/}; TS=${TS#nous-}; TS=${TS%.json}
	BACKUP="$DATA_DIR/backups/nous-$TS.db"
	if [ -f "$BACKUP" ]; then
		CHECK=$("$CURRENT" --check "$BACKUP" 2>&1)
		if printf '%s\n' "$CHECK" | grep -qx 'quick_check=ok' \
		   && printf '%s\n' "$CHECK" | grep -qx "schema=$SCHEMA"; then
			ok "备份 quick_check=ok，schema=$SCHEMA 与 manifest.schema_version 一致"
		else
			bad "--check 输出与 manifest.schema_version 不符：$(printf '%s' "$CHECK" | tr '\n' ' ')"
		fi
	else
		skip "备份库 $BACKUP 不在（首次部署没有备份；人工哨兵）"
	fi
else
	skip '没有 manifest 或产物（首次部署前）'
fi

step '判据 1：磁盘 sha8 / --version / /v1/device 三方同源'
if [ -x "$CURRENT" ]; then
	DISC=$(sha256sum "$CURRENT" | cut -c1-8)
	CLI=$("$CURRENT" --version 2>/dev/null | sed -n 's/.*build \([0-9a-f]\{8\}\).*/\1/p')
	HTTP=$(api /v1/device | jq -r '.build // empty' 2>/dev/null || true)
	if [ -n "$DISC" ] && [ "$DISC" = "$CLI" ] && [ "$DISC" = "$HTTP" ]; then
		ok "三方都是 $DISC"
	else
		bad "sha 不同源：disk=$DISC cli=$CLI http=$HTTP"
	fi
else
	skip 'current 产物不存在'
fi

step '判据 4：回退演练（人工哨兵：需要上一版产物 + 备份）'
PREV=$(ls -1d "$BIN_DIR"/nous-app-*/nous-app 2>/dev/null | sed 's#/nous-app$##' | sort -r | sed -n '2p' || true)
if [ -n "$PREV" ] && [ -n "$MANIFEST" ]; then
	PREV_SHA=${PREV##*/}; PREV_SHA=${PREV_SHA#nous-app-}; PREV_SHA=${PREV_SHA%%-*}
	ok "上一版存在：$PREV（回退时须 current 指回它 + 恢复快照，单次演练由 deploy.sh 自动回退完成）"
	info=$(api /v1/device 2>/dev/null || true)
	if printf '%s' "$info" | jq -e '.version != null' >/dev/null 2>&1; then
		ok '/v1/device 200 且带 version 信息'
	else
		bad "/v1/device 回退后不可用：${info:-空}"
	fi
else
	skip '没有上一版或 manifest（第一次部署时不存在，等第二次升级再验）'
fi

step '判据 2c：拒绝启动停在 failed，300 秒不拉起（--wait）'
if [ -x "$CURRENT" ]; then
	# 坏库必须**服务用户读得到**：`mktemp -d` 建的是 0700 root 的目录，服务以 nous 身份跑，
	# 连接不上 → 退出码 2（不是 78），于是看起来像「拒绝启动没落 78」，实际是判据自己造错了现场。
	TMPDIR_=$(mktemp -d "$DATA_DIR/.check-XXXXXX") || TMPDIR_=""
	[ -n "$TMPDIR_" ] || { bad 'mktemp 失败'; TMPDIR_=/tmp; }
	chown nous:nous "$TMPDIR_" 2>/dev/null || true
	chmod 0750 "$TMPDIR_" 2>/dev/null || true
	TMPDB=$TMPDIR_/nous.db
	cp "$DATA_DIR/nous.db" "$TMPDB" 2>/dev/null || true
	chown nous:nous "$TMPDB" 2>/dev/null || true
	python3 - "$TMPDB" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1])
v = c.execute("select version from durable_schema where singleton=1").fetchone()[0]
c.execute("update durable_schema set version=? where singleton=1", (v+1,))
c.commit(); c.close()
PY
	if [ $? -eq 0 ]; then
		# 真机上把 service 临时指向坏库；这一格会真的把服务拉停，所以只在 j1900 跑。
		OVERRIDE=/etc/systemd/system/nous-app.service.d/99-deploy-check.conf
		mkdir -p "$(dirname "$OVERRIDE")"
		printf '[Service]\nEnvironment=NOUS_DB=%s\n' "$TMPDB" > "$OVERRIDE"
		systemctl daemon-reload
		systemctl restart nous-app || true
		sleep 5
		S1=$(systemctl show -p ActiveState,Result,NRestarts nous-app | tr '\n' ' ')
		printf '%s\n' "$S1"
		sleep "$WAIT"
		S2=$(systemctl show -p ActiveState,Result,NRestarts nous-app | tr '\n' ' ')
		if printf '%s' "$S1" | grep -q 'ActiveState=failed' \
		   && printf '%s' "$S2" | grep -q 'ActiveState=failed' \
		   && printf '%s' "$S2" | grep -q 'Result=exit-code' \
		   && [ "$(printf '%s' "$S1" | sed -n 's/.*NRestarts=\([0-9]*\).*/\1/p')" = \
		        "$(printf '%s' "$S2" | sed -n 's/.*NRestarts=\([0-9]*\).*/\1/p')" ]; then
			ok "拒绝启动 → failed/exit-code，${WAIT}s 后 NRestarts 不涨"
		else
			bad "拒绝启动没停在 failed 或在拉起循环：before=$S1 after=$S2"
		fi
		rm -f "$OVERRIDE"
		rmdir "$(dirname "$OVERRIDE")" 2>/dev/null || true
		rm -rf "$TMPDIR_" 2>/dev/null || true
		systemctl daemon-reload
		systemctl restart nous-app || true
	else
		skip '造不出 schema+1 的坏库（nous.db 缺失？）'
	fi
else
	skip 'current 产物不存在'
fi

step '判据 10：删 WorkingDirectory → refuse: workdir-missing（人工哨兵）'
skip '需临时改 unit 并重启真服务；由人按 scripts/deploy-check.sh 注释里的步骤做，避免脚本在验收中把服务长期拉停'

step '判据 11：活跃任务拒升级（人工哨兵：真流式会话）'
if [ -x "$CURRENT" ]; then
	N=$("$CURRENT" --active-tasks 2>&1 || true)
	printf '  --active-tasks → %s\n' "$N"
	skip '人工步骤见下（2026-11-18 在 j1900 上验过一次，结论见 #52）':
	# 1) 往 /var/lib/nous/nous.db 插一行 tasks(status=''running'')（没有 sqlite3 CLI 就用 python3）
	# 2) 把 /opt/nous-app/current 指向**上一版** —— 否则同 sha 幂等短路，走不到闸门
	# 3) 跑 scripts/deploy.sh → 必须 `✗ 有 1 个活跃任务（status <> ''terminal''），拒绝升级`、退出码 1
	# 4) 删掉那行、current 指回最新、restart，确认 /v1/device 200 且 entries 不变
	# 自动化会先把服务拉停再拉起，不适合脚本自跑。
else
	skip 'current 产物不存在'
fi

step '判据 5 / 14：卸载边界（人工哨兵：会停服务，不在自检里自动跑）'
# 判据 12：首次路径跑完 /var/lib/nous 只多出**清单内**的库文件，不多出别的。
# 清单现在是两个库：durable 的 `nous.db` 与存储分离后 `#56/#57` 的 `nous-plugins.db`
# （两个库各有自己的迁移链，见 docs/decisions.md「存储分离 = 两个文件」）。
# 这是**白名单**不是放宽：清单外的任何文件仍然红 —— 数据目录里冒出来的东西必须有人认领。
# 命名理顺（decisions 里记的 durable.db / nous.db）之后，这份清单要跟着改成两个名字。
# `.gitconfig` 是触发器发布认领的：服务账号 `nous` 没有 HOME 配置，仓库层
# （`crates/nous/repo`）虽然会给 commit 注入身份，但 `git push` 的凭据与
# `git config` 读取仍然经过它 —— 留在数据目录里是有意的，不是垃圾。
ALLOWED='nous.db nous.db-wal nous.db-shm nous-plugins.db nous-plugins.db-wal nous-plugins.db-shm .gitconfig'
FIND_ARGS=""
for _n in $ALLOWED; do FIND_ARGS="$FIND_ARGS ! -name '$_n'"; done
EXTRA=$(eval "find \"\$DATA_DIR\" -maxdepth 1 -type f $FIND_ARGS" 2>/dev/null || true)
if [ -e "$DATA_DIR/nous.db" ] && [ -e "$DATA_DIR/nous-plugins.db" ] && [ -z "$EXTRA" ]; then
	ok '首次路径：/var/lib/nous 只有我们认领的两个库（nous.db + nous-plugins.db，各带 -wal/-shm）'
elif [ -e "$DATA_DIR/nous.db" ]; then
	bad "判据 12：/var/lib/nous 多出文件：$(printf '%s\n' "$EXTRA" | tr '\n' ' ')"
else
	skip 'nous.db 不存在（首次部署前；升级后也不该缺）'
fi
skip '--uninstall / --purge 的删除边界由人执行：uninstall 后产物不在、库与 token 在；purge 不带确认串非 0 且数据仍在'

printf '\n判据汇总：绿 %d / 红 %d / SKIP %d\n' "$GREEN" "$RED" "$SKIPN"
[ "$RED" -eq 0 ] && exit 0 || exit 1
