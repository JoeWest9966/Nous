#!/usr/bin/env bash
# plugin.toml 体检（issue #55）：判据全在这一个脚本里。
#
#   ./scripts/check-plugins.sh
#
# 数据源：crates/*/*/ 目录、各 Cargo.toml、docs/plugins.md、cargo metadata、
# crates/nous/durable/README.md。任何不一致都指名并打印 `checked=N mismatched=M`，
# 退出码非 0 —— 不许只 exit 0。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

FAIL=0
ok()   { printf '  ✓ %s\n' "$*"; }
bad()  { printf '  ✗ %s\n' "$*"; FAIL=1; }

if ! python3 -c 'import tomllib' 2>/dev/null; then
	printf 'RED: python3 没有 tomllib（需要 3.11+）\n' >&2
	exit 2
fi

REPORT=$(python3 - <<'PY'
import glob
import json
import os
import re
import subprocess
import sys
import tomllib

ROOT = os.getcwd()
PLUGINS_DOC = os.path.join(ROOT, 'docs/plugins.md')
README = os.path.join(ROOT, 'crates/nous/durable/README.md')

printed = []
fail = []


def report(msg, is_bad=False):
    printed.append(('✗' if is_bad else '✓', msg))
    if is_bad:
        fail.append(msg)


def cell_text(value):
    # markdown 表格里可用反引号包住模块 / 包名
    return value.strip().strip('`').strip()


def read_plugins_rows():
    if not os.path.exists(PLUGINS_DOC):
        report(f'{PLUGINS_DOC} 缺失（清单是判据的一部分）', True)
        return {}
    rows = {}
    with open(PLUGINS_DOC, encoding='utf-8') as handle:
        for line in handle:
            if not line.lstrip().startswith('|'):
                continue
            cells = [c.strip() for c in line.strip().strip('|').split('|')]
            if len(cells) < 4:
                continue
            module = cell_text(cells[0])
            # 只认反引号包起来的模块名；表头 `module` 与分隔行 `---` 都不是模块。
            if not (cells[0].strip().startswith('`') and re.fullmatch(r'[a-z][a-z0-9-]*', module)):
                continue
            crate_raw = cells[2]
            if crate_raw.startswith('(待建)') or cell_text(crate_raw) in ('—', '-', ''):
                continue  # 待建 / 外挂行不是 crate 目录
            rows[module] = {
                'vendor': cell_text(cells[1]),
                'crate': cell_text(crate_raw),
                'layer': cell_text(cells[3]),
            }
    if not rows:
        report(f'{PLUGINS_DOC} 不可解析（一行 crate 都没读出来）', True)
    return rows


def read_cargo_version(manifest, meta):
    with open(manifest, 'rb') as handle:
        package = tomllib.load(handle).get('package', {})
    version = package.get('version')
    if isinstance(version, dict) and version.get('workspace'):
        version = None
    if version is None:
        # `version.workspace = true`：读 workspace 根，不复制常量到每个 crate。
        with open(os.path.join(ROOT, 'Cargo.toml'), 'rb') as handle:
            version = tomllib.load(handle).get('workspace', {}).get('package', {}).get('version')
    if meta:
        for pkg in meta['packages']:
            if os.path.normpath(pkg['manifest_path']) == os.path.normpath(manifest):
                if pkg.get('version'):
                    version = pkg['version']  # cargo 已解析的最终值，优先
                break
    return version


def cargo_metadata():
    try:
        proc = subprocess.run(
            ['cargo', 'metadata', '--format-version', '1'],
            capture_output=True, text=True,
        )
    except OSError as exc:
        report(f'cargo metadata 跑不起来：{exc}', True)
        return None
    if proc.returncode != 0:
        report(f'cargo metadata 失败：{proc.stderr.strip()[:400]}', True)
        return None
    return json.loads(proc.stdout)


def internal_deps(meta, manifest_path):
    if not meta:
        return None
    by_path = {os.path.normpath(p['manifest_path']): p for p in meta['packages']}
    pkg = by_path.get(os.path.normpath(manifest_path))
    if pkg is None:
        report(f'cargo metadata 里找不到 {os.path.relpath(manifest_path, ROOT)}', True)
        return None
    deps = set()
    for dep in pkg['dependencies']:
        if dep.get('kind') not in (None, 'normal'):
            continue  # dev-/build- 依赖不算模块的 requires
        if dep.get('source') is not None:
            continue  # 只算 workspace 内部 crate
        deps.add(dep['name'])
    return deps


def check_readme():
    if not os.path.exists(README):
        report('README_MISSING crates/nous/durable/README.md 缺失', True)
        return
    with open(README, encoding='utf-8') as handle:
        text = handle.read()
    lines = text.splitlines()
    expected = [
        '## 1 它是什么 / What it is',
        '## 2 什么时候用 / When to use',
        '## 3 清单 / Manifest',
        '## 4 API',
        '## 5 权限 / Permissions',
        '## 6 失败模式 / Failure modes',
        '## 7 判据 / Judgement',
        '## 8 反悔点 / Reversal',
    ]
    for heading in expected:
        if not any(line.strip() == heading for line in lines):
            report(f'README_MISSING_CHAPTER durable: {heading}', True)
    if sum(1 for line in lines if line.startswith('# ')) != 1:
        report('README_TITLE durable: `^# ` 必须恰好命中一次', True)
    for anchor in ('[中文](#zh)', '[English](#en)', '<a id="zh"></a>', '<a id="en"></a>'):
        if anchor not in text:
            report(f'README_ANCHOR durable: 缺 {anchor}', True)
    # 双语：每章下面都要有中文行与英文行（第 3、4 章只写来源 / 待生成，例外）
    for n in (1, 2, 5, 6, 7, 8):
        start = next((i for i, line in enumerate(lines) if line.strip().startswith(f'## {n} ')), None)
        if start is None:
            continue
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith('## ')), len(lines))
        section = '\n'.join(lines[start + 1:end])
        if not re.search(r'[\u4e00-\u9fff]', section):
            report(f'README_BILINGUAL durable: 第 {n} 章没有中文', True)
        if not re.search(r'[A-Za-z]{4,}', section):
            report(f'README_BILINGUAL durable: 第 {n} 章没有英文', True)

    # 第 4 章：只有一行「待生成」，不许表格
    start = next((i for i, line in enumerate(lines) if line.strip() == '## 4 API'), None)
    if start is not None:
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith('## ')), len(lines))
        body = [line for line in lines[start + 1:end] if line.strip()]
        sentinel = '待生成：见 #48 的 /v1/_routes（禁止手写占位表格）'
        if len(body) != 1 or sentinel not in body[0]:
            report('README_API_CHAPTER durable: 第 4 章必须只有一行「待生成」', True)
        if any(line.lstrip().startswith('|') for line in body):
            report('README_API_TABLE durable: 第 4 章不许有表格', True)


rows = read_plugins_rows()
dirs = sorted(
    (p for p in glob.glob('crates/*/*') if os.path.isdir(p)),
    key=lambda p: (p.split('/')[1], p.split('/')[2]),
)
manifest_paths = sorted(glob.glob('crates/*/*/plugin.toml'))

# 集合双向：目录 ↔ 清单 ↔ plugin.toml。待建 / 外挂行已从 rows 里排除。
dir_set = {os.path.relpath(p).replace(os.sep, '/') for p in dirs}
row_set = {f'crates/{v["vendor"]}/{m}' for m, v in rows.items()}
manifest_dirs = {os.path.dirname(p).replace(os.sep, '/') for p in manifest_paths}
for label, extra in (
    ('DIR_NO_ROW', dir_set - row_set),
    ('ROW_NO_DIR', row_set - dir_set),
    ('DIR_NO_MANIFEST', dir_set - manifest_dirs),
    ('MANIFEST_NO_DIR', manifest_dirs - dir_set),
    ('MANIFEST_DUP', {p for p in manifest_paths if os.path.dirname(p).replace(os.sep, '/') not in dir_set}),
):
    for item in sorted(extra):
        report(f'{label} {item}', True)

meta = cargo_metadata()
for directory in dirs:
    rel = directory.replace(os.sep, '/')
    module = os.path.basename(directory)
    vendor = os.path.basename(os.path.dirname(directory))
    manifest = os.path.join(directory, 'plugin.toml')
    if not os.path.isfile(manifest):
        continue  # 缺清单已由集合检查指名
    try:
        with open(manifest, 'rb') as handle:
            pm = tomllib.load(handle)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        report(f'{module}: plugin.toml 解析失败：{exc}', True)
        continue

    failures = []
    for key, want in (('name', module), ('vendor', vendor)):
        got = pm.get(key)
        if got != want:
            failures.append(f'{key}={got!r}（应为 {want!r}）')
    row = rows.get(module)
    if row is None:
        failures.append('docs/plugins.md 无对应行')
    else:
        if row['vendor'] != vendor:
            failures.append(f'vendor 与清单行 {row["vendor"]!r} 不符')
        got_layer = pm.get('layer')
        if got_layer != row['layer']:
            failures.append(f'layer={got_layer!r}（应为 {row["layer"]!r}）')
        if pm.get('crate') != row['crate']:
            failures.append(f'crate={pm.get("crate")!r}（应为 {row["crate"]!r}）')
        provides = pm.get('provides', {})
        if not isinstance(provides, dict):
            failures.append('provides 不是表')
        else:
            capability = provides.get('capability')
            if capability is not None and capability != module:
                failures.append(f'capability={capability!r}（module 列是 {module!r}）')
            if provides.get('endpoints') != []:
                failures.append('endpoints 必须是 []（本票不生成端点）')

    cargo = os.path.join(directory, 'Cargo.toml')
    try:
        cargo_version = read_cargo_version(cargo, meta)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        failures.append(f'Cargo.toml 解析失败：{exc}')
        cargo_version = None
    if pm.get('version') != cargo_version:
        failures.append(f'version={pm.get("version")!r}（Cargo.toml 是 {cargo_version!r}）')

    want_deps = internal_deps(meta, os.path.abspath(cargo))
    if want_deps is not None:
        got_deps = pm.get('requires', {})
        if not isinstance(got_deps, dict):
            failures.append('requires 不是表')
            got_deps = {}
        missing = sorted(want_deps - set(got_deps))
        extra = sorted(set(got_deps) - want_deps)
        if missing:
            failures.append('requires 少写：' + ', '.join(missing))
        if extra:
            failures.append('requires 多写：' + ', '.join(extra))
        for dep, constraint in sorted(got_deps.items()):
            if constraint not in ('^0.1', '^0.1.0', '0.1.0'):
                failures.append(f'requires.{dep}={constraint!r}（内部 crate 版本是 ^0.1）')

    if failures:
        report(f'{module}: ' + '；'.join(failures), True)

check_readme()

for mark, msg in printed:
    print(f'{mark} {msg}')
print(f'checked={len(dirs)} mismatched={len(fail)}')
sys.exit(1 if fail else 0)
PY
)
PY_RC=$?

printf '%s\n' "$REPORT"

exit "$PY_RC"
