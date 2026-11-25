#!/usr/bin/env python3
"""#93 插件版本考古器：从仓库痕迹机械地推每个插件的版本线。

口径（写进 docs/plugin-versions.md 与票面）：
  1. 改名链：以 `<dir>/Cargo.toml` / `<dir>/plugin.toml` 的改名（`R0xx`）反推目录的前世，
     一直追到最早入库那次。
  2. 噪声（不算插件经历）：纯 `R100` 搬家、只改 `plugin.toml` 的 `version` 行、
     `chore(release)` / `release:` / `chore(plugins): … 版本追平` 这类发行版号同步、
     同标题同日期的重复提交（rebase / cherry-pick 的同一件事）。
  3. 起点：本目录（含改名链）第一次入库那次 = `0.1.0`（不计入累加）；
     早于它的「只在 ROUTES 里以本模块名义登记端点」的提交照样计入累加（票面硬问题 3）。
  4. 分档（机械判据，宁可低估 minor）：
       minor = 本模块 `plugin.toml` 的 `name` / `layer` / `crate` / `vendor` / `provides` 有删改
               （整份新增不算），或 `server/src/meta.rs` 里 `module: "<本模块>"` 的 ROUTES 行
               有增删改（端点新增 / 删除 / 换路径换方法）。
       patch = 其余一切（实现 / 日志 / 性能 / 文案 / README / 测试）。
     「端点改形状 / 改状态码」不体现在 ROUTES 行上，机械判据抓不到 → 落 patch 档（已知低估）。

用法：
  python3 scripts/plugin-versions.py            # 打印表（markdown）
  python3 scripts/plugin-versions.py --check     # 与盘上 plugin.toml 比对，不一致退出 1
  python3 scripts/plugin-versions.py --doc-check # 再校 docs/plugin-versions.md 的表没漂
"""
import json
import os
import re
import subprocess
import sys
import tomllib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

NOISE_SUBJECT = re.compile(
    r'^(chore\(release\)|release:|chore\(plugins\):.*版本追平|chore\(#14)'
)


def git(*args):
    return subprocess.run(['git', '-C', ROOT] + list(args),
                          capture_output=True, text=True, check=True).stdout


def parse_log():
    """一次读完：按时间正序的 [{sha, date, subject, changes[(status, old, new)]}]。"""
    raw = git('log', '--no-merges', '--format=@@@%H\x01%ad\x01%s', '--date=short',
              '--name-status', '-M40%')
    commits = []
    for block in raw.split('@@@')[1:]:
        head, _, rest = block.partition('\n')
        sha, date, subject = head.split('\x01')
        changes = []
        for line in rest.strip().splitlines():
            parts = line.split('\t')
            if len(parts) < 2:
                continue
            status = parts[0]
            if status.startswith(('R', 'C')):
                changes.append((status, parts[1], parts[2]))
            else:
                changes.append((status, parts[1], parts[1]))
        commits.append({'sha': sha, 'date': date, 'subject': subject, 'changes': changes})
    commits.reverse()
    return commits


def rename_chain(start):
    """目录的改名链（从今到古）。以 Cargo.toml / plugin.toml 的改名当锚（R100 或 R0xx 都算）。"""
    chain = [start]
    seen = {start}
    path = start
    while True:
        nxt = None
        for commit in COMMITS:
            for status, old, new in commit['changes']:
                if not status.startswith('R'):
                    continue
                if new not in (f'{path}/Cargo.toml', f'{path}/plugin.toml'):
                    continue
                anchor = os.path.basename(new)
                if not old.endswith('/' + anchor):
                    continue
                candidate = old[:-len('/' + anchor)]
                if candidate not in seen:
                    nxt = candidate
                    break
            if nxt:
                break
        if not nxt:
            return chain
        chain.append(nxt)
        seen.add(nxt)
        path = nxt


def diff_lines(sha, paths):
    """一次 show，返回被这个提交改动的 +/- 行（只限 paths 里的路径）。"""
    if not paths:
        return []
    out = git('show', '--format=', '--unified=0', sha, '--', *paths)
    return [line for line in out.splitlines()
            if line[:1] in '+-' and not line.startswith(('+++', '---'))]


def touches_only_manifest_version(sha, files):
    """提交对这几个文件的改动是不是只动了 version 行。"""
    lines = diff_lines(sha, files)
    if not lines:
        return True
    return all(re.match(r'^[+-]\s*version\s*=', line) for line in lines)


MANIFEST_KEYS = re.compile(r'^[+-]\s*(name|layer|crate|vendor|provides)\b')


def added_in(sha, path):
    out = git('show', '--format=', '--name-status', sha, '--', path)
    return any(line.split('\t')[:1] == ['A'] for line in out.splitlines() if line)


def classify(module, sha, chain):
    """机械分档：minor 或 patch。

    minor 只认两种「契约面」痕迹：
      ① `plugin.toml` 里 `name` / `layer` / `crate` / `vendor` / `provides` 有**删改**
         （整份新增不算 —— 那是清单出生，不是 capability 改名）；
      ② `ROUTES` 里 `module: "<本模块>"` 的行有增删改（端点新增 / 删除 / 换路径换方法）。
    其余（含「端点只改回包形状 / 改状态码」这类机械抓不到的）一律 patch —— 宁可低估 minor。
    """
    manifest = f'{chain[0]}/plugin.toml'
    if not added_in(sha, manifest):
        for line in diff_lines(sha, [manifest]):
            if line.startswith('-') and MANIFEST_KEYS.match(line):
                return 'minor', 'plugin.toml 的 name/layer/crate/provides 删改了'
    route_lines = [line for line in diff_lines(sha, ['crates/nous/server/src/meta.rs'])
                   if f'module: "{module}"' in line]
    if route_lines:
        return 'minor', f'ROUTES 里 {len(route_lines)} 行端点声明变动'
    return 'patch', ''


def route_lines_of(module, sha):
    """这个提交在 `meta.rs` 的 `ROUTES` 里改到的、属于本模块的行。"""
    return [line for line in diff_lines(sha, ['crates/nous/server/src/meta.rs'])
            if f'module: "{module}"' in line]


def counted_commits(module, chain):
    """这个插件自己的经历（剔除噪声、跨改名链去重、时间正序）。

    一条提交算这个插件的经历，得满足其中一个：
      · 改到了本模块目录（`crates/<vendor>/<module>/`，含改名链上的前世）；
      · 改到了 `ROUTES` 里 `module: "<本模块>"` 的行 —— 端点契约是这个插件的能力面，
        哪怕 handler 当时还住在 `server/` 里（票面硬问题 3：算）。
    """
    picked = []
    seen = set()
    seen_keys = set()
    for commit in COMMITS:
        if commit['sha'] in seen:
            continue
        # 同一件事被 rebase / cherry-pick 成两个提交（同标题同日期）：只算一次。
        key = (commit['subject'], commit['date'])
        if key in seen_keys:
            continue
        paths = {new for status, old, new in commit['changes']}
        mine = sorted(p for p in paths if any(p.startswith(f'{d}/') for d in chain))
        routes = route_lines_of(module, commit['sha'])
        if not mine and not routes:
            continue
        statuses = [s for s, old, new in commit['changes']
                    if any(old.startswith(f'{d}/') or new.startswith(f'{d}/') for d in chain)]
        if statuses and all(s == 'R100' for s in statuses):
            continue  # 纯搬家：不算这个插件的经历
        manifests = [p for p in mine if p.endswith('plugin.toml')]
        others = [p for p in mine if not p.endswith('plugin.toml')]
        if manifests and not others and not routes \
                and touches_only_manifest_version(commit['sha'], manifests):
            continue  # 只改了 plugin.toml 的 version 行：发行版号同步，不是插件经历
        if NOISE_SUBJECT.match(commit['subject']):
            continue
        seen.add(commit['sha'])
        seen_keys.add(key)
        tier, why = classify(module, commit['sha'], chain)
        picked.append({'sha': commit['sha'][:8], 'date': commit['date'],
                       'subject': commit['subject'], 'tier': tier, 'why': why,
                       'own_dir': bool(mine)})
    return picked


def manifest_version(path):
    with open(os.path.join(ROOT, path), 'rb') as handle:
        return tomllib.load(handle).get('version')


DOC = os.path.join(ROOT, 'docs/plugin-versions.md')
DOC_ROW = re.compile(
    r'^\| `(?P<module>[a-z0-9-]+)` \| (?P<total>\d+) \| (?P<minor>\d+) \| '
    r'(?P<patch>\d+) \| (?P<version>[\d.]+) \| `(?P<sha>[0-9a-f]{8})` ',
    re.M,
)


def check_doc(rows):
    """`docs/plugin-versions.md` 的表必须是**现算的**，不是手抄一遍就冻住。

    只校考古结论那五列（提交数 / minor 档 / patch 档 / 推出 / 起点 sha）。
    模块之后按规则 8 自己涨版本时，这份文档的「推出」列会变得比盘上低 —— 那是正常的，
    所以这里**不**比 `现填`（那个比 `--check` 管），只比「表与 git 痕迹是否还一致」。
    """
    bad = []
    if not os.path.isfile(DOC):
        return [f'{os.path.relpath(DOC, ROOT)} 缺失（#93 的表是交付物）']
    text = open(DOC, encoding='utf-8').read()
    seen = {}
    for match in DOC_ROW.finditer(text):
        seen[match['module']] = match
    for row in rows:
        match = seen.get(row['module'])
        if match is None:
            bad.append(f"{row['module']}: docs/plugin-versions.md 没有行")
            continue
        want = (str(row['total']), str(row['minor']), str(row['patch']),
                row['version'], row['base']['sha'] if row['base'] else '')
        got = (match['total'], match['minor'], match['patch'],
               match['version'], match['sha'])
        if want != got:
            bad.append(f"{row['module']}: 文档 {got} ≠ 推得 {want}")
    for module in seen:
        if module not in {row['module'] for row in rows}:
            bad.append(f'{module}: 文档里的行已无对应 plugin.toml')
    for item in bad:
        print(f'✗ {item}')
    return bad


def main():
    global COMMITS
    COMMITS = parse_log()
    dirs = sorted(
        os.path.relpath(os.path.join(ROOT, 'crates', d), ROOT).replace(os.sep, '/')
        for d in os.listdir(os.path.join(ROOT, 'crates'))
        if os.path.isdir(os.path.join(ROOT, 'crates', d))
    )
    modules = []
    for vendor in dirs:
        base = os.path.join(ROOT, vendor)
        for name in sorted(os.listdir(base)):
            full = os.path.join(base, name)
            if os.path.isdir(full) and os.path.isfile(os.path.join(full, 'plugin.toml')):
                modules.append(f'{vendor}/{name}')

    def base_of(module):
        """起点 = 改名链上第一次入库那次（`0.1.0`）；没经历就是 `None`。"""
        picked = counted_commits(os.path.basename(module), rename_chain(module))
        return picked[0]['sha'] if picked else None

    rows = []
    for module in modules:
        name = os.path.basename(module)
        chain = rename_chain(module)
        commits = counted_commits(name, chain)
        # 起点 = 这个插件**第一次在仓里以模块名存在**那次 = `0.1.0`，不计入累加。
        # “存在” 取两种痕迹中更早的那个：目录入库，或 `ROUTES` 里第一次以它名义登记
        # 端点（handler 那时还住在 `server/`）—— 票面硬问题 3 拍：端点属于模块，算它的经历。
        # `commits` 已按时间正序，首条就是那个更早的。
        base = commits[0] if commits else None
        rest = commits[1:]
        minor = sum(1 for c in rest if c['tier'] == 'minor')
        patch = sum(1 for c in rest if c['tier'] == 'patch')
        # 按规则 8 逐条累加：minor → 0.Y+1.0（patch 位归零），patch → 0.Y.Z+1。
        major_y, major_z = 1, 0
        for c in rest:
            if c['tier'] == 'minor':
                major_y, major_z = major_y + 1, 0
            else:
                major_z += 1
        rows.append({
            'module': os.path.basename(module),
            'dir': module,
            'chain': chain,
            'base': base,
            'commits': commits,
            'total': len(commits),
            'minor': minor,
            'patch': patch,
            'version': f'0.{major_y}.{major_z}',
            'manifest': manifest_version(f'{module}/plugin.toml'),
        })

    if '--json' in sys.argv:
        print(json.dumps(rows, ensure_ascii=False, indent=1))
        return 0

    if '--check' in sys.argv:
        bad = [r for r in rows if r['manifest'] != r['version']]
        for r in bad:
            print(f"✗ {r['module']}: plugin.toml={r['manifest']} 推得={r['version']}")
        if '--doc-check' in sys.argv:
            bad += check_doc(rows)
        print(f'checked={len(rows)} mismatched={len(bad)}')
        return 1 if bad else 0

    if '--doc-check' in sys.argv:
        bad = check_doc(rows)
        print(f'checked={len(rows)} mismatched={len(bad)}')
        return 1 if bad else 0

    print('| module | 提交数 | minor 档 | patch 档 | 推出 | 现填 | 起点提交 |')
    print('|---|---|---|---|---|---|---|')
    for r in rows:
        print(f"| `{r['module']}` | {r['total']} | {r['minor']} | {r['patch']} | "
              f"{r['version']} | {r['manifest']} | `{r['base']['sha']}` {r['base']['date']} |")
    return 0


if __name__ == '__main__':
    sys.exit(main())
