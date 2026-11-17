#!/bin/sh
# judge.sh —— 附录 A 的执行器。
#
# 本文件只做两件事：
#   1. 把 case 名映射到一次真实的场景执行（`nous-ext-probe` 或仓库扫描）；
#   2. 载体不存在时打印 SKIP（**stderr**）并 exit 0 —— SKIP 不是绿。
#
# 期望值写在附录 A（`docs/spec/ext-runtime.md`）里，**本文件不写任何期望值**：
# `nous-ext-probe` 打印观测（`key=value`），附录 A 的命令用 `grep -F` 做断言。
# 判据里不许出现 `echo "commit_seq=..."` 这种桩（EX-81 扫的就是它）。
#
#   ./scripts/judge.sh --list      # [behavior] 78 + [tool] 1
#   ./scripts/judge.sh --ids       # 79 行编号
#   ./scripts/judge.sh --pending   # 15 条待实测哨兵
#   ./scripts/judge.sh EX-13       # 按编号
#   ./scripts/judge.sh section-twice [--dirty]   # 按语义名
#
# 退出码：已知 case 一律 0（SKIP 也是一种结论），未知 case 2。

set -u

ROOT=$(cd "$(dirname "$0")/.." && pwd)
cd "$ROOT" || exit 1
PROBE="$ROOT/target/debug/nous-ext-probe"

TABLE='
EX-01 manifest-paths
EX-02 load-missing-handle
EX-03 load-empty-extension
EX-04 kernel-untouched-v2
EX-05 manifest-forbidden-keys
EX-06 protocol-mismatch
EX-07 one-process-per-extension
EX-08 frame-keys
EX-09 calls-per-turn
EX-10 truncated-frame
EX-11 inflight-across-reload
EX-12 frame-forbidden-keys
EX-13 section-twice
EX-14 pi-system-count
EX-15 shuffle-order
EX-16 wrong-tag
EX-17 tool-stream-seq
EX-18 tool-cancel
EX-19 tool-cancel-late-output
EX-20 result-and-mutations
EX-21 mutation-replay
EX-22 reject-mutation
EX-23 kill-extension
EX-24 restart-extension
EX-25 nameplate-diffset
EX-26 fd-no-handle-value
EX-27 deliver-twice
EX-28 crash-once
EX-29 third-failure-breaker
EX-31 diagnostic-dedup
EX-32 always-crash
EX-34 concurrency-20
EX-35 output-1mb
EX-36 rss-256mb
EX-37 single-reload-entry
EX-38 reload-triggers
EX-39 callid-across-reload
EX-40 idle-cpu
EX-41 reload-fails-keeps-old
EX-42 partial-write
EX-43 write-complete
EX-44 parse-fails
EX-45 out-of-cwd-write
EX-46 environ-whitelist
EX-47 fd-no-db
EX-48 byte-flip
EX-49 no-toolchain-in-tree
EX-50 triple-not-compatible
EX-51 audit-kinds
EX-52 audit-replay-10
EX-53 entries-filter
EX-54 catalog-moved
EX-55 canary-cross-session
EX-56 disable-extension
EX-57 ex-id-set
EX-58 oversized-frame
EX-59 nameplate-conflict
EX-60 handle-path-escape
EX-61 count-after-restart
EX-62 breaker-push-sse
EX-63 breaker-in-snapshot
EX-64 reload-merge-20
EX-65 reload-two-exts
EX-66 boot-load-failure
EX-67 symlink-escape
EX-68 truncation-deterministic
EX-69 uid-allocation
EX-70 cap-setuid
EX-71 fd-exact-set
EX-72 groups-empty
EX-73 eacces
EX-74 evil-carrier
EX-75 provenance
EX-76 disabled-nameplate-referenced
EX-77 atomic-swap-20x
EX-78 canary-cross-extension
EX-79 canary-cross-request
EX-80 clock-invisible
'

TOOL='
EX-81 different-sources'

PENDING=''

usage() {
  echo "用法：$0 --list | --ids | --pending | <EX-nn> | <case-name> [--dirty]" >&2
  exit 2
}

resolve() {
  { printf '%s\n' "$TABLE"; printf '%s\n' "$TOOL"; } | awk -v k="$1" '$1==k || $2==k { print $1; exit }'
}

name_of() {
  { printf '%s\n' "$TABLE"; printf '%s\n' "$TOOL"; } | awk -v k="$1" '$1==k { print $2; exit }'
}

case "${1-}" in
  --list|-l)
    echo "[behavior]"
    printf '%s\n' "$TABLE"   | awk 'NF { print $1 }'
    echo "[tool]"
    printf '%s\n' "$TOOL"    | awk 'NF { print $1 }'
    exit 0
    ;;
  --ids)
    { printf '%s\n' "$TABLE"; printf '%s\n' "$TOOL"; } | awk 'NF { print $1 }'
    exit 0
    ;;
  --pending|-p)
    printf '%s\n' "$PENDING" | tr ' ' '\n' | awk 'NF'
    exit 0
    ;;
esac

ID=$(resolve "$1")
if [ -z "$ID" ]; then
  echo "judge.sh: 没有这个 case：$1" >&2
  usage
fi
CASE=$(name_of "$ID")
shift
DIRTY=""
for arg in "$@"; do
  case "$arg" in
    --dirty) DIRTY="--dirty" ;;
  esac
done

# ---------------------------------------------------------------------------
# 仓库扫描类判据：不需要起进程。
# ---------------------------------------------------------------------------

ex_id_set() {
  ids=$(./scripts/judge.sh --ids)
  has81=0
  has82=0
  printf '%s\n' "$ids" | grep -qx 'EX-81' && has81=1
  printf '%s\n' "$ids" | grep -qx 'EX-82' && has82=1
  echo "has_EX-81=$has81 has_EX-82=$has82"
}

# EX-81：judge.sh 自己不许硬编码观测值。
different_sources() {
  grep -nE 'echo .*(commit_seq|entry_id|hash|version)=' scripts/judge.sh
  # 干净时 grep 无输出、rc=1；SKIP 不是这里的结论，空输出就是结论。
  exit 0
}

# 名单对得上附录 A（EX-57 的自检口径）。
ex_id_set_selfcheck() {
  # 附录 A 的编号集合与 --ids 的集合相等。
  want=$(sed -n '/^| EX-[0-9]/s/^| \(EX-[0-9]*\) .*/\1/p' docs/spec/ext-runtime.md | sort -u)
  have=$(./scripts/judge.sh --ids | sort -u)
  if [ "$want" = "$have" ]; then
    echo "ids_equal=1 count=$(printf '%s\n' "$have" | wc -l)"
  else
    echo "ids_equal=0"
    printf '%s\n' "$want" >/tmp/judge-want.$$$$
    printf '%s\n' "$have" >/tmp/judge-have.$$$$
    diff /tmp/judge-want.$$$$ /tmp/judge-have.$$$$ | sed -n '1,20p'
    rm -f /tmp/judge-want.$$$$ /tmp/judge-have.$$$$
  fi
}

case "$CASE" in
  ex-id-set) ex_id_set; exit 0 ;;
  different-sources) different_sources ;;
esac

# ---------------------------------------------------------------------------
# 场景类判据：交给 nous-ext-probe（真进程 / 真库 / 真 /proc）。
# ---------------------------------------------------------------------------

if [ ! -x "$PROBE" ]; then
  if ! command -v cargo >/dev/null 2>&1; then
    echo "SKIP: cargo 不在（$ID）" >&2
    exit 0
  fi
  cargo build --offline --quiet \
    -p nous-ext-probe -p nous-server -p nous-app \
    -p exts-section -p exts-tool -p exts-nameplate -p exts-handle -p exts-combo -p exts-evil \
    >/dev/null 2>&1
fi

if [ ! -x "$PROBE" ]; then
  echo "SKIP: 判据执行器没编出来（$ID）" >&2
  exit 0
fi

"$PROBE" "$CASE" $DIRTY
exit $?
