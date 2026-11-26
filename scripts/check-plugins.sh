#!/usr/bin/env bash
# plugin.toml 体检（issue #55）：判据全在这一个脚本里。
#
#   ./scripts/check-plugins.sh
#
# 数据源：crates/*/*/ 目录、各 Cargo.toml、docs/plugins.md、cargo metadata、
# crates/Nous/Durable/README.md。任何不一致都指名并打印 `checked=N mismatched=M`，
# 退出码非 0 —— 不许只 exit 0。
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1

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

printed = []
fail = []


def report(msg, is_bad=False):
    printed.append(('✗' if is_bad else '✓', msg))
    if is_bad:
        fail.append(msg)


def cell_text(value):
    # markdown 表格里可用反引号包住模块 / 包名
    return value.strip().strip('`').strip()


NAME_OK = re.compile(r'^(?:[A-Za-z][A-Za-z0-9]*(?:_[A-Za-z][A-Za-z0-9]*)?|[a-z][a-z0-9-]*)$')


def split_name(name):
    """模块全名 → (vendor 段的小写, 目录名 PascalCase)。规则 12。

    `Nous_Device` → ('nous', 'Device')；`Nous_PresetsAdmin` → ('nous', 'PresetsAdmin')；
    老写法 `data-manager` → (None, 'DataManager')。

    目录名 = 尾段按大写字母断词、再**首字母大写连起来**（Magento 的 `app/code/Amasty/Shiprestriction`）：
    `PresetsAdmin` → `PresetsAdmin`、`DataManager` → `DataManager`。只 `.lower()` 会把
    `presets-admin` 推成 `presetsadmin`，跟仓里目录对不上。
    """
    if '_' in name:
        head, tail = name.split('_', 1)
        return head.lower(), _pascal(tail)
    return None, _pascal(name)


def _words(text):
    """CamelCase / kebab / snake 都拆成词：`PresetsAdmin` / `presets-admin` → ['presets','admin']。"""
    spaced = re.sub(r'(?<=[A-Za-z0-9])(?=[A-Z])', '-', text)
    return [w for w in re.split(r'[-_]+', spaced) if w]


def _pascal(text):
    """→ `PresetsAdmin`（目录名）。"""
    return ''.join(w[:1].upper() + w[1:].lower() for w in _words(text))


def _kebab(text):
    """→ `presets-admin`（包名 / capability 用的小写机器名）。"""
    return '-'.join(w.lower() for w in _words(text))


def read_plugins_rows():
    if not os.path.exists(PLUGINS_DOC):
        report(f'{PLUGINS_DOC} 缺失（清单是判据的一部分）', True)
        return {}
    rows = {}
    duplicates = []
    all_modules = set()
    with open(PLUGINS_DOC, encoding='utf-8') as handle:
        for line in handle:
            if not line.lstrip().startswith('|'):
                continue
            cells = [c.strip() for c in line.strip().strip('|').split('|')]
            if len(cells) < 4:
                continue
            module = cell_text(cells[0])
            # 只认反引号包起来的模块名；表头 `module` 与分隔行 `---` 都不是模块。
            if not (cells[0].strip().startswith('`') and NAME_OK.fullmatch(module)):
                continue
            if module in all_modules:
                duplicates.append(module)
            all_modules.add(module)
            crate_raw = cells[2]
            if crate_raw.startswith('(待建)') or cell_text(crate_raw) in ('—', '-', ''):
                continue  # 待建 / 外挂行不是 crate 目录
            rows[module] = {
                'dir': split_name(module)[1],
                'vendor': cell_text(cells[1]),
                'crate': cell_text(crate_raw),
                'layer': cell_text(cells[3]),
            }
    if duplicates:
        report(f'{PLUGINS_DOC} 模块名重复（后者不再静默覆盖前者）：{sorted(set(duplicates))}', True)
    if not rows:
        report(f'{PLUGINS_DOC} 不可解析（一行 crate 都没读出来）', True)
    return rows


SEMVER = re.compile(r'^(\d+)\.(\d+)\.(\d+)$')


def version_key(text):
    match = SEMVER.match(text or '')
    return tuple(int(part) for part in match.groups()) if match else None


def head_version(rel):
    """git HEAD 里这一行的 version；文件还没入库 / 不在 git 里 → None（不判倒退）。"""
    try:
        proc = subprocess.run(['git', 'show', f'HEAD:{rel}'], capture_output=True, text=True)
    except OSError:
        return None
    if proc.returncode != 0:
        return None
    try:
        return tomllib.loads(proc.stdout).get('version')
    except tomllib.TOMLDecodeError:
        return None


def read_cargo_name(manifest):
    with open(manifest, 'rb') as handle:
        return tomllib.load(handle).get('package', {}).get('name')


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


HAN = re.compile(r'[\u4e00-\u9fff]')
README_LIMIT = 300


def check_readme(dirs):
    """每个插件一份人话 README：存在、恰好一个标题、中文 ≤300 字（docs/plugins.md 规则 7）。

    旧的 8 章双语模板（issue #55）在此作废：它要求的篇幅与「300 字说清是什么」互斥，
    桌面端插件页只读这一段正文。
    """
    for directory in dirs:
        rel = directory.replace(os.sep, '/')
        path = os.path.join(directory, 'README.md')
        if not os.path.isfile(path):
            report(f'README_MISSING {rel}/README.md', True)
            continue
        try:
            with open(path, encoding='utf-8') as handle:
                text = handle.read()
        except OSError as exc:
            report(f'README_UNREADABLE {rel}/README.md：{exc}', True)
            continue
        if not text.strip():
            report(f'README_EMPTY {rel}/README.md', True)
            continue
        if sum(1 for line in text.splitlines() if line.startswith('# ')) != 1:
            report(f'README_TITLE {rel}/README.md：`^# ` 必须恰好命中一次', True)
        count = len(HAN.findall(text))
        if count > README_LIMIT:
            report(f'README_TOO_LONG {rel}/README.md：汉字={count}（上限 {README_LIMIT}）', True)


rows = read_plugins_rows()
dirs = sorted(
    (p for p in glob.glob('crates/*/*') if os.path.isdir(p)),
    key=lambda p: (p.split('/')[1], p.split('/')[2]),
)
manifest_paths = sorted(glob.glob('crates/*/*/plugin.toml'))

# 集合双向：目录 ↔ 清单 ↔ plugin.toml。待建 / 外挂行已从 rows 里排除。
dir_set = {os.path.relpath(p).replace(os.sep, '/') for p in dirs}
row_set = {f'crates/{v["vendor"]}/{v["dir"]}' for v in rows.values()}
rows_by_dir = {f'crates/{v["vendor"]}/{v["dir"]}': (full, v) for full, v in rows.items()}
# key 用**完整目录**而不是目录名：规则 12 允许不同 vendor 下有同名 module
# （`crates/Element/Core` 与 `crates/Trigger/Core`），目录名不唯一。
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
    full_name = pm.get('name')
    if not isinstance(full_name, str) or not NAME_OK.fullmatch(full_name):
        failures.append(f'name={full_name!r}（规则 12 要 `Vendor_Module` 或老的小写连字符名）')
        full_name = module
    name_vendor, name_dir = split_name(full_name)
    if name_dir != module:
        failures.append(f'目录名={module!r}（由全名 {full_name!r} 推出应为 {name_dir!r}）')
    # vendor 是 PascalCase 出品方（`Nous` / `Element` / `Data`），且必须就是目录名。
    # 全名第一段是小写形，所以只跟 `vendor.lower()` 比 —— 这条是规则 12 的「同一件事两套写法」。
    if pm.get('vendor') != vendor:
        failures.append(f'vendor={pm.get("vendor")!r}（应为目录的 {vendor!r}）')
    if name_vendor is not None and name_vendor != vendor.lower():
        failures.append(f'全名第一段是 {name_vendor!r}，与 vendor={vendor!r} 不符（规则 12）')
    row_pair = rows_by_dir.get(rel)
    if row_pair is None:
        failures.append('docs/plugins.md 无对应行')
        row = None
    else:
        row_full, row = row_pair
        if row_full != full_name:
            failures.append(f'清单行是 `{row_full}`，与 plugin.toml 的 name={full_name!r} 不符')
    if row is not None:
        if row['vendor'] != vendor:  # 清单行 vendor 也是 PascalCase（`Nous`）
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
            if capability is not None and capability != _kebab(module):
                failures.append(
                    f'capability={capability!r}（目录名 {module!r} 的小写机器名是 {_kebab(module)!r}）'
                )
            # 规则 11：端点只有一个真相 —— 插件的 `routes()`；注册表（capabilities /
            # `_routes` / openapi）从它生成。清单里不许再抄一份（曾经是永远空的 `[]`，
            # 让人以为"这个插件没端点"）。
            if 'endpoints' in provides:
                failures.append('provides.endpoints 已作废（规则 11）：端点由 routes() 声明，清单不复制')

    # 插件版本**独立演进**（Magento 2 的模块版本语义，`docs/plugins.md` 规则 8）：
    # 不再 == workspace / Cargo.toml 的版本。只守两条：合法 semver、比 HEAD 里的值不倒退。
    cargo = os.path.join(directory, 'Cargo.toml')
    version = pm.get('version')
    key = version_key(version) if isinstance(version, str) else None
    if key is None:
        failures.append(f'version={version!r}（要 X.Y.Z 三个整数）')
    else:
        prev = head_version(os.path.relpath(manifest, ROOT).replace(os.sep, '/'))
        prev_key = version_key(prev) if isinstance(prev, str) else None
        if prev_key is not None and key < prev_key:
            failures.append(f'version={version} 比 HEAD 的 {prev} 低（插件版本只增不减）')

    # 规则 10：包名 = `<vendor>-<module>`（Magento 的 composer 包名同构，扁平写法不写 module-）。
    try:
        package_name = read_cargo_name(cargo)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        failures.append(f'Cargo.toml 解析失败：{exc}')
        package_name = None
    # 包名是 cargo 的世界：全小写 kebab（`nous-device`），与目录的 PascalCase 不同。
    want_package = f'{vendor.lower()}-{_kebab(module)}'
    if package_name is not None and package_name != want_package:
        failures.append(f'包名={package_name!r}（规则 10 要求 {want_package!r}）')

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
        report(f'{full_name}: ' + '；'.join(failures), True)

check_readme(dirs)

for mark, msg in printed:
    print(f'{mark} {msg}')
print(f'checked={len(dirs)} mismatched={len(fail)}')
sys.exit(1 if fail else 0)
PY
)
PY_RC=$?

printf '%s\n' "$REPORT"

exit "$PY_RC"
