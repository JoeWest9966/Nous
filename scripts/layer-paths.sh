#!/usr/bin/env bash
# 路径选择子：从 `docs/plugins.md` 的 `layer` 列输出模块**路径**（空格分隔）。
#
#   K=$(scripts/layer-paths.sh kernel)   # 例如 crates/nous/wire crates/nous/chord …
#   A=$(scripts/layer-paths.sh app)
#
# 规矩（spec §3.0）：
#   - 成员**只从 `docs/plugins.md` 读**，本脚本里不写死任何模块名，也不写死 vendor 名；
#   - 跳过 `crate` 列为 `(待建) …` 或 `—` 的行（目录不存在，不是判据载体）；
#   - 清单缺失 / 不可解析 → 退出码 1（= 判据 RED，不是 SKIP）；参数缺失 → 退出码 2（用法错误）。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

MANIFEST="${NOUS_PLUGINS:-docs/plugins.md}"
LAYER="${1:-}"

if [ -z "$LAYER" ]; then
	printf '用法: %s <layer>   # kernel | platform | app\n' "$0" >&2
	exit 2
fi

if [ ! -f "$MANIFEST" ]; then
	printf 'RED: %s 缺失（清单是判据的一部分）\n' "$MANIFEST" >&2
	exit 1
fi

python3 - "$MANIFEST" "$LAYER" <<'PY'
import sys

path, want = sys.argv[1], sys.argv[2]
rows = []
try:
    lines = open(path, encoding='utf-8').read().splitlines()
except OSError as exc:
    print(f'RED: 读不出 {path}：{exc}', file=sys.stderr)
    raise SystemExit(1)

for line in lines:
    # 只认表格行：| module | vendor | crate | layer | …
    if not line.lstrip().startswith('|'):
        continue
    cells = [c.strip() for c in line.strip().strip('|').split('|')]
    if len(cells) < 4:
        continue
    module, vendor, crate, layer = cells[0], cells[1], cells[2], cells[3]
    # 跳表头与分隔行：模块名必须是 `反引号包起来` 的名字
    if not (module.startswith('`') and module.endswith('`')):
        continue
    module = module.strip('`')
    vendor = vendor.strip('`')
    crate = crate.strip('`')
    layer = layer.strip('`')
    if not module or not vendor:
        continue
    rows.append((module, vendor, crate, layer))

if not rows:
    print(f'RED: {path} 不可解析（一行模块都没读出来）', file=sys.stderr)
    raise SystemExit(1)

out = []
for module, vendor, crate, layer in rows:
    if layer != want:
        continue
    # `(待建) nous-sessions` = 目录还不存在；`—` = 不是 crate。两种都不是判据载体。
    if crate.startswith('(待建)') or crate in ('—', '-', ''):
        continue
    out.append(f'crates/{vendor}/{module}')

print(' '.join(out))
PY
