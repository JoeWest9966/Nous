#!/usr/bin/env python3
"""#93 插件版本考古器：从仓库痕迹机械地推每个插件的版本线。

口径（写进 docs/plugin-versions.md 与 decisions.md）：

  1. 改名链：以 `<dir>/Cargo.toml` / `<dir>/plugin.toml` 的改名（`R0xx`）反推目录的前世，
     一直追到最早入库那次。
  2. 噪声（不算插件经历）：纯 `R100` 搬家、只改 `plugin.toml` 的 `version` 行、
     `chore(release)` / `release:` / `chore(plugins): … 版本追平` / `chore(#14)` 这类
     发行版号同步，以及同标题同日期的重复提交（rebase / cherry-pick 的同一件事）。
  3. 起点：这个插件第一次在仓里以模块名存在那次 = `0.1.0`（不计入累加）。「存在」取
     「本目录入库」与「`ROUTES` 里首次登记本模块」两者中更早的那个 —— 票面硬问题 3 拍：
     端点属于模块，handler 还住在 `server/` 时的提交也算它的经历。
  4. 分档（机械判据，宁可低估 minor）：
       minor = 本模块的**目录改名**（vendor 段或模块段变了，按 git 的 R 记录认）/
               `plugin.toml` 的 `name` / `layer` / `crate` / `vendor` / `provides`
               有删改（整份新增不算），或 `meta.rs` 里 `module: "<本模块>"` 的 ROUTES 行
               有增删改（端点新增 / 删除 / 换路径换方法）。
       patch = 其余一切（实现 / 日志 / 性能 / 文案 / README / 测试）。
     「端点改形状 / 改状态码」不体现在 ROUTES 行上，机械判据抓不到 → 落 patch 档（已知低估）。

判据的语义（**别把它当"版本必须等于考古结论"用**）：考古结论是 `#93` 那一刻的快照，
之后各插件按规则 8 自己涨。所以

  · `--check`         盘上的版本 **不得低于** 考古结论（低于 = 有人手改回去了）；
  · `--doc-check`     `docs/plugin-versions.md` 的表 == 现算的考古结论（表是生成物，别手抄）；
  · `--write-doc`     把表写回 `docs/plugin-versions.md`（`#93` 之后**不要**再跑它 ——
                      表记的是快照，不是当前版本）。

用法：
  python3 scripts/plugin-versions.py             # 打印表（markdown）
  python3 scripts/plugin-versions.py --check      # 版本不得低于考古结论
  python3 scripts/plugin-versions.py --doc-check  # 文档里的表 == 现算的考古结论
  python3 scripts/plugin-versions.py --write-doc  # 重写文档里的表（一次性）
  python3 scripts/plugin-versions.py --json       # 机器可读（含每个提交的分档与理由）
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

# 插件目录的唯一真相是 `docs/plugins.md`（规则 1 / 6）：`scripts/layer-paths.sh <layer>`
# 只读那张表的 module / vendor / crate / layer 四列，本脚本**不写死** `crates/<vendor>/<module>`
# 这种两层结构，也不点名任何模块。
LAYERS = ('kernel', 'platform', 'app', 'dev')


def git(*args):
    return subprocess.run(['git', '-C', ROOT] + list(args),
                          capture_output=True, text=True, check=True).stdout


def plugin_dirs():
    """`docs/plugins.md` 里所有有目录的模块：`目录名 -> crates/<vendor>/<目录名>`。

    目录名由模块全名推出（`docs/plugins.md` 规则 12，与 `scripts/layer-paths.sh` 同一算法）：
    `Element_Models` → `crates/Element/Models`。

    key 用**模块全名**（`plugin.toml` 的 `name`）：`/v1/capabilities`、路由声明、本文档三者
    都按全名说话，目录名只是目录。
    """
    out = {}
    for layer in LAYERS:
        proc = subprocess.run(['bash', os.path.join(ROOT, 'scripts/layer-paths.sh'), layer],
                              capture_output=True, text=True, cwd=ROOT)
        if proc.returncode != 0:
            raise SystemExit(f'layer-paths.sh {layer} 失败（清单是判据的一部分）：'
                             f'{proc.stderr.strip()}')
        for path in proc.stdout.split():
            rel = os.path.relpath(os.path.join(ROOT, path), ROOT).replace(os.sep, '/')
            manifest = os.path.join(ROOT, rel, 'plugin.toml')
            try:
                with open(manifest, 'rb') as handle:
                    name = tomllib.load(handle).get('name')
            except (OSError, tomllib.TOMLDecodeError):
                name = None
            out[name or os.path.basename(rel)] = rel
    return dict(sorted(out.items()))


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


def touches_only_manifest_bookkeeping(sha, files):
    """提交对这几个文件的改动是不是只动了"账本行"（version / endpoints）。

    这两种行不是插件的能力面：`version` 跟发行版号同步、`endpoints` 已按规则 11 整个删掉
    （删它属于清单结构变更，不是插件经历）。身份行（name/layer/crate/vendor/provides）
    照样算 minor —— 那条判据在 `classify` 里，不在这里。
    """
    lines = diff_lines(sha, files)
    if not lines:
        return True
    return all(re.match(r'^[+-]\s*(version|endpoints)\s*=', line) for line in lines)


MANIFEST_KEYS = re.compile(r'^[+-]\s*(name|layer|crate|vendor|provides)\b')


def added_in(sha, path):
    out = git('show', '--format=', '--name-status', sha, '--', path)
    return any(line.split('\t')[:1] == ['A'] for line in out.splitlines() if line)


def route_lines_of(module, sha):
    """这个提交在 `meta.rs` 的 `ROUTES` 里改到的、属于本模块的行。"""
    return [line for line in diff_lines(sha, ['crates/Nous/Server/src/meta.rs'])
            if f'module: "{module}"' in line]


def dir_renamed(sha, chain):
    """这个提交是不是把这个插件自己的目录改名了（vendor 段或模块段变了）。

    规则 8 把「vendor 或模块改名」算 minor，可**纯改名提交里 plugin.toml 一行都没变**
    （`git show` 的 +/- 是空的），靠行判据会落成 patch —— 2026-11-25 把 29 个目录改成
    PascalCase 时真踩到：16 个模块全落 patch，与规则 8 的跨度表不符。
    所以改名走 git 的 R 记录：新旧路径的**父目录不同**就是改名。
    """
    new_root = chain[0]
    for commit in COMMITS:
        if commit['sha'] != sha:
            continue
        for status, old, new in commit['changes']:
            if not status.startswith('R'):
                continue
            if not new.startswith(new_root + '/'):
                continue
            if os.path.dirname(old) != os.path.dirname(new):
                return True
    return False


def classify(module, sha, chain):
    """机械分档：minor 或 patch。见模块文档第 4 条。"""
    if dir_renamed(sha, chain):
        return 'minor', '插件目录改名（vendor 段或模块段变了）'
    manifest = f'{chain[0]}/plugin.toml'
    if not added_in(sha, manifest):
        for line in diff_lines(sha, [manifest]):
            if line.startswith('-') and MANIFEST_KEYS.match(line):
                return 'minor', 'plugin.toml 的 name/layer/crate/provides 删改了'
    routes = route_lines_of(module, sha)
    if routes:
        return 'minor', f'ROUTES 里 {len(routes)} 行端点声明变动'
    return 'patch', ''


def counted_commits(module, chain):
    """这个插件自己的经历（剔除噪声、跨改名链去重、时间正序）。

    一条提交算这个插件的经历，得满足其中一个：
      · 改到了本模块目录（含改名链上的前世）；
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
                and touches_only_manifest_bookkeeping(commit['sha'], manifests):
            continue  # 只改了 plugin.toml 的 version 行：发行版号同步，不是插件经历
        if NOISE_SUBJECT.match(commit['subject']):
            continue
        seen.add(commit['sha'])
        seen_keys.add(key)
        tier, why = classify(module, commit['sha'], chain)
        picked.append({'sha': commit['sha'][:8], 'date': commit['date'],
                       'subject': commit['subject'], 'tier': tier, 'why': why})
    return picked


def semver(text):
    """`X.Y.Z` → (X, Y, Z)；不是合法 semver 就 None。"""
    parts = (text or '').split('.')
    if len(parts) != 3 or not all(p.isdigit() for p in parts):
        return None
    return tuple(int(p) for p in parts)


def manifest_version(rel):
    with open(os.path.join(ROOT, rel, 'plugin.toml'), 'rb') as handle:
        return tomllib.load(handle).get('version')


DOC = os.path.join(ROOT, 'docs/plugin-versions.md')
DOC_ROW = re.compile(
    r'^\| `(?P<module>[A-Za-z0-9_-]+)` \| (?P<total>\d+) \| (?P<minor>\d+) \| '
    r'(?P<patch>\d+) \| (?P<version>[\d.]+) \| (?P<manifest>[\d.]+) \| '
    r'`(?P<sha>[0-9a-f]{8})` ',
    re.M,
)


def archaeology():
    """每个插件的考古结论：`0.1.0` 起点 + 逐条按规则 8 累加。"""
    rows = []
    for module, rel in plugin_dirs().items():
        chain = rename_chain(rel)
        commits = counted_commits(module, chain)
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
            'module': module,
            'dir': rel,
            'chain': chain,
            'base': base,
            'commits': commits,
            'total': len(commits),
            'minor': minor,
            'patch': patch,
            'version': f'0.{major_y}.{major_z}',
            'manifest': manifest_version(rel),
        })
    return rows


def table(rows):
    """表头 + 每行。`现填` 一列是**生成那一刻**盘上的值，用来对照「推出」。"""
    lines = ['| module | 提交数 | minor 档 | patch 档 | 推出 | 现填 | 起点提交 |',
             '|---|---|---|---|---|---|---|']
    for r in rows:
        base = r['base']
        origin = f"`{base['sha']}` {base['date']}" if base else '—'
        lines.append(f"| `{r['module']}` | {r['total']} | {r['minor']} | {r['patch']} | "
                     f"{r['version']} | {r['manifest']} | {origin} |")
    return '\n'.join(lines)


def check_doc(rows):
    """`docs/plugin-versions.md` 的表必须是**生成物**，不是手抄一遍就冻住。

    只校考古结论（提交数 / minor 档 / patch 档 / 推出 / 起点 sha）—— `现填` 一列不校：
    `#93` 之后各插件按规则 8 自己涨，那一列会变得比盘上低，那是正常的。
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
    known = {row['module'] for row in rows}
    for module in seen:
        if module not in known:
            bad.append(f'{module}: 文档里的行已无对应 plugin.toml')
    return bad


def write_doc(rows):
    text = open(DOC, encoding='utf-8').read()
    start = text.index('| module | 提交数 |')
    end = text.index('\n\n', start)
    updated = text[:start] + table(rows) + text[end:]
    open(DOC, 'w', encoding='utf-8').write(updated)


def main():
    global COMMITS
    COMMITS = parse_log()
    rows = archaeology()

    if '--json' in sys.argv:
        print(json.dumps(rows, ensure_ascii=False, indent=1))
        return 0

    if '--write-doc' in sys.argv:
        write_doc(rows)
        print(f'wrote {os.path.relpath(DOC, ROOT)}（{len(rows)} 行）')
        return 0

    if '--check' in sys.argv or '--doc-check' in sys.argv:
        bad = []
        if '--check' in sys.argv:
            # 语义：**不得低于**考古结论。`#93` 之后自己涨（高于）是正常的；
            # 低于 = 有人把版本手改回去了，或者插件目录换了名字导致考古面变了。
            for row in rows:
                got = semver(row['manifest'])
                want = semver(row['version'])
                if got is None:
                    bad.append(f"{row['module']}: plugin.toml 的 version={row['manifest']!r} 不是 X.Y.Z")
                elif want is not None and got < want:
                    bad.append(f"{row['module']}: plugin.toml={row['manifest']} 比考古结论 "
                               f"{row['version']} 低（插件版本只增不减）")
        if '--doc-check' in sys.argv:
            bad += check_doc(rows)
        for item in bad:
            print(f'✗ {item}')
        print(f'checked={len(rows)} mismatched={len(bad)}')
        return 1 if bad else 0

    print(table(rows))
    return 0


if __name__ == '__main__':
    sys.exit(main())
