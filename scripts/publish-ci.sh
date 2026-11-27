#!/usr/bin/env bash
#
# 把**本地/远程机上编好的** nous-app 按提交号挂到滚动预发布 `ci`。
#
# 为什么要有这个脚本：CI 推 main 也会自动出一份（`.github/workflows/package.yml`），
# 但它跑在 4 核 runner 上，15–25 分钟。急着要某一份（部署 j1900、给别人复现）时，
# 在开发机上编（增量 30 秒 / 冷编译 3–5 分钟）再跑这个脚本，2 分钟就有一份同名成品。
# 两条路**落同一个名字**（`nous-app-<sha8>-<target>`），所以谁先到都一样，后到的覆盖。
#
# 用法：
#   ./scripts/publish-ci.sh                      # 用默认 musl 产物
#   ./scripts/publish-ci.sh path/to/nous-app     # 指定产物（编在别处时用）
#   ./scripts/publish-ci.sh --dry-run            # 只打印要做什么，不碰网络
#   ./scripts/publish-ci.sh --dirty              # 工作树不干净也发（名字带 -dirty）
#   ./scripts/publish-ci.sh --allow-dynamic      # 非静态产物也发（默认拒绝）
#
# 环境：
#   GH_TOKEN         有 repo 写权限的 token；不设就用 `gh auth` 里那个
#   NOUS_CI_RELEASE  滚动发布的名字，默认 ci
#   NOUS_CI_KEEP     保留几份，默认 5
#
# 退出码：2 = 用法错；1 = 前置检查没过（产物不是静态 ELF / 工作树脏 / git 不在仓里）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RELEASE="${NOUS_CI_RELEASE:-ci}"
KEEP="${NOUS_CI_KEEP:-5}"
TARGET="x86_64-unknown-linux-musl"
BIN_DEFAULT="target/$TARGET/release/nous-app"

DRY=0
DIRTY_OK=0
DYNAMIC_OK=0
BIN=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY=1 ;;
    --dirty) DIRTY_OK=1 ;;
    --allow-dynamic) DYNAMIC_OK=1 ;;
    -h|--help) sed -n '2,28p' "$0"; exit 0 ;;
    -*) echo "不认识的参数：$arg" >&2; exit 2 ;;
    *) BIN="$arg" ;;
  esac
done
BIN="${BIN:-$BIN_DEFAULT}"

die() { echo "✗ $*" >&2; exit 1; }
say() { echo "· $*"; }

# ── 前置检查：产物在、是静态 ELF ────────────────────────────────────────────
[ -x "$BIN" ] || die "找不到可执行产物：$BIN
  先编一份：cargo build --release --target $TARGET -p nous-server --bin nous-app
  或者把它作为参数传进来：./scripts/publish-ci.sh /path/to/nous-app"

FILE_DESC="$(file -b "$BIN")"
case "$FILE_DESC" in
  ELF*64-bit*) ;;
  *) die "不是 64 位 ELF：$FILE_DESC" ;;
esac
if ldd "$BIN" 2>&1 | grep -q '=>'; then
  [ "$DYNAMIC_OK" = 1 ] || die "不是静态链接（j1900 只跑静态产物）
  $(ldd "$BIN" 2>&1 | head -3)
  确实要发动态的，加 --allow-dynamic。"
  say "警告：发的是动态链接产物（--allow-dynamic）"
fi

# ── 名字里的提交号：产物必须对得上某个提交，否则「按 sha 取货」是假的 ──────
git rev-parse --git-dir >/dev/null 2>&1 || die "不在 git 仓里，取不到提交号"
SHA="$(git rev-parse --short=8 HEAD)"
SUFFIX=""
if [ -n "$(git status --porcelain)" ]; then
  [ "$DIRTY_OK" = 1 ] || die "工作树不干净：产物对应不了任何一个提交
  先提交，或者加 --dirty（名字里会带 -dirty，取货的人一眼看得出）"
  SUFFIX="-dirty"
  say "警告：工作树不干净，名字带 -dirty"
fi

NAME="nous-app-$SHA$SUFFIX-$TARGET"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
install -m755 "$BIN" "$STAGE/$NAME"
( cd "$STAGE" && sha256sum "$NAME" > "$NAME.sha256" )

# 版本自证：发出去的东西是什么版本，写在日志里，别只靠记忆。
VERSION_LINE="$("$STAGE/$NAME" --version 2>&1 | head -1 || echo '(取不到 --version)')"
say "产物  $NAME"
say "大小  $(du -h "$STAGE/$NAME" | cut -f1)"
say "版本  $VERSION_LINE"
say "sha   $(sha256sum "$STAGE/$NAME" | cut -d' ' -f1)"

if [ "$DRY" = 1 ]; then
  say "--dry-run：要执行的是"
  echo "    gh release view $RELEASE || gh release create $RELEASE --prerelease --target main …"
  echo "    gh release upload $RELEASE $NAME $NAME.sha256 --clobber"
  echo "    # 只留最近 $KEEP 份（按上传时间）"
  exit 0
fi

REPO_ARG=()
if [ -n "${GH_REPO:-}" ]; then REPO_ARG=(--repo "$GH_REPO"); fi

# ── 发布 ────────────────────────────────────────────────────────────────────
if ! gh release view "$RELEASE" "${REPO_ARG[@]}" >/dev/null 2>&1; then
  say "建滚动预发布 $RELEASE"
  gh release create "$RELEASE" "${REPO_ARG[@]}" \
    --prerelease \
    --target main \
    --title "CI 构建" \
    --notes "按提交号命名的产物（\`git rev-parse --short=8\`）。CI 推 main 会自动出一份，开发机上跑 \`scripts/publish-ci.sh\` 也能补一份同名覆盖。只留最近 $KEEP 份；正式版本在 \`v*\` 的 Release 里。"
fi

say "上传到 $RELEASE"
gh release upload "$RELEASE" "${REPO_ARG[@]}" \
  "$STAGE/$NAME" "$STAGE/$NAME.sha256" --clobber

# ── 只留最近 KEEP 份（每份 2 个文件）───────────────────────────────────────
say "清理：只留最近 $KEEP 份"
gh release view "$RELEASE" "${REPO_ARG[@]}" --json assets \
  -q '.assets[] | "\(.createdAt)\t\(.name)"' \
  | sort -r | tail -n +$((KEEP * 2 + 1)) | cut -f2 \
  | while read -r old; do
      [ -n "$old" ] && gh release delete-asset "$RELEASE" "$old" "${REPO_ARG[@]}" -y >/dev/null
    done

say "完成：$(gh release view "$RELEASE" "${REPO_ARG[@]}" --json url -q .url)"
echo
echo "取货："
echo "  gh release download $RELEASE -p '$NAME*'"
echo "部署："
echo "  NOUS_APP_BIN=./$NAME scripts/deploy.sh"
