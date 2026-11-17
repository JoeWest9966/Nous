#!/bin/sh
# judge.sh v0 —— **只有入口，没有任何判断**。
#
# 本文件的职责只有一个：把附录 A 的 EX 编号变成一个可调用的入口。
# 每个 case 打印一行 SKIP 然后 exit 0。这里**不许长出一条真实断言**：
# 骨架一旦带上判断，spec 线的人就会为了让骨架"看起来更像完成"而改判据，
# 那正是我们一路在防的漂移。填判据的活属于 T5 的小子，不属于本文件。
#
#   ./scripts/judge.sh --list      # 80 行：EX 编号
#   ./scripts/judge.sh EX-13       # 按编号
#   ./scripts/judge.sh section-twice   # 按语义名（附录 A 里用的就是这个名字）
#
# 退出码：已知 case 一律 0（SKIP 也是一种结论），未知 case 2。

set -u

TABLE='
EX-01 -
EX-02 load-missing-handle
EX-03 load-empty-extension
EX-04 -
EX-05 -
EX-06 protocol-mismatch
EX-07 -
EX-08 dump-frame-keys
EX-09 calls-per-turn
EX-10 truncated-frame
EX-11 inflight-across-reload
EX-12 dump-frame-keys
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
EX-26 handle-value
EX-27 deliver-twice
EX-28 crash-once
EX-29 third-failure-breaker
EX-30 breaker-10-turns
EX-31 diagnostic-dedup
EX-32 always-crash
EX-33 breaker-idle-calls
EX-34 concurrency-20
EX-35 output-1mb
EX-36 rss-256mb
EX-37 -
EX-38 reload-triggers
EX-39 callid-across-reload
EX-40 idle-wakeups
EX-41 reload-fails-keeps-old
EX-42 partial-write
EX-43 write-complete
EX-44 parse-fails
EX-45 out-of-cwd-write
EX-46 -
EX-47 -
EX-48 byte-flip
EX-49 -
EX-50 triple-not-compatible
EX-51 audit-kinds
EX-52 audit-replay-10
EX-53 entries-filter
EX-54 -
EX-55 two-sessions-isolated
EX-56 disable-extension
EX-57 --list
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
EX-69 -
EX-70 -
EX-71 -
EX-72 -
EX-73 out-of-cwd-write
EX-74 evil-carrier
EX-75 provenance
EX-76 disabled-nameplate-referenced
EX-77 atomic-swap-20x
EX-78 canary-cross-extension
EX-79 canary-cross-request
EX-80 clock-invisible
'

usage() {
  echo "用法：$0 --list | <EX-nn> | <case-name>" >&2
  exit 2
}

resolve() {
  # 参数可以是编号也可以是语义名；输出编号；找不到输出空
  printf '%s\n' "$TABLE" | awk -v k="$1" '$1==k || $2==k { print $1; exit }'
}

case "${1-}" in
  --list|-l|"")
    printf '%s\n' "$TABLE" | awk 'NF { print $1 }'
    exit 0
    ;;
esac

ID=$(resolve "$1")
if [ -z "$ID" ]; then
  echo "judge.sh: 没有这个 case：$1" >&2
  usage
fi

# v0：每条都只到这里。
echo "SKIP: 等 T5（$ID）"
exit 0
