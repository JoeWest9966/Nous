#!/usr/bin/env python3
"""从真数据 `fixtures/agent.sqlite` 生成**脱敏小样本** `fixtures/agent.small.sqlite`。

为什么要一份小的进仓：`fixtures/*.sqlite` 是真实用户数据、永不入库（`.gitignore`），
所以 CI 上永远没有它。而判据「能读出 /v1/sessions 同形 JSON」又必须真跑一条 ——
断言一个拿不到的文件必须存在 = 流水线永久红（见 docs/pitfalls.md）。

做法是**整库照抄再删行**，不是重新造库：durable 的元数据表（schema 版本、record_ids 映射、
document_revisions 链）只有真库才对得上，手搓一个必然读出错误结论。

脱敏三条（够用就行，判据要看的是形状不是内容）：
1. 每条 JSON 记录里的**字符串截断到 200 字**
2. `usage` / `cost` 下所有数字**归零**（token 数、钱）
3. 只留 id 最小的前 N 个会话 —— 父会话 id 比子会话小，这样 `parent` 不会指向被删掉的会话

用法：
    python3 scripts/make-small-fixture.py                     # 默认 120 个会话
    python3 scripts/make-small-fixture.py --keep 130 --out /tmp/x.sqlite

输出会打印：会话数、条目数、体积。改完 `git add fixtures/agent.small.sqlite` 提交。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import sqlite3
import sys

MAX_STR = 200
TRUNC = "…（截断）"
ZERO_KEYS = {"usage", "cost"}

TRUTH = pathlib.Path(__file__).resolve().parent.parent / "fixtures" / "agent.sqlite"


def redact(value, key=None):
    """递归截断长字符串、把 usage/cost 下的数字归零。"""
    if isinstance(value, dict):
        return {k: redact(v, k) for k, v in value.items()}
    if isinstance(value, list):
        return [redact(v, key) for v in value]
    if isinstance(value, str) and len(value) > MAX_STR:
        return value[:MAX_STR] + TRUNC
    if isinstance(value, (int, float)) and not isinstance(value, bool) and key in ZERO_KEYS:
        return 0
    return value


def redact_json(text: str) -> str:
    try:
        return json.dumps(redact(json.loads(text)), ensure_ascii=False, sort_keys=True)
    except json.JSONDecodeError:
        # 不是 JSON 就不该动它 —— durable 的约束是 json_valid(record)，坏数据本身就该炸
        raise SystemExit(f"记录不是合法 JSON，先查真库：{text[:120]!r}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--truth", default=str(TRUTH), help="真数据（只读）")
    ap.add_argument(
        "--out",
        default=str(TRUTH.parent / "agent.small.sqlite"),
        help="输出（进仓的那份）",
    )
    ap.add_argument("--keep", type=int, default=120, help="保留 id 最小的前 N 个会话")
    args = ap.parse_args()

    truth = pathlib.Path(args.truth)
    out = pathlib.Path(args.out)
    if not truth.exists():
        raise SystemExit(f"真数据不在：{truth}（从 j1900 scp，见 verify.sh §2）")

    out.unlink(missing_ok=True)
    for side in ("-wal", "-shm"):
        out.with_name(out.name + side).unlink(missing_ok=True)
    shutil.copy2(truth, out)

    db = sqlite3.connect(out)
    db.execute("pragma journal_mode=delete")
    # 只留 id 最小的前 N 个会话：父会话 id 更小，parent 不会悬空
    keep = [
        row[0]
        for row in db.execute(
            "select id from conversations order by id asc limit ?", (args.keep,)
        )
    ]
    if not keep:
        raise SystemExit("真库里没有会话，别做小样本了")
    ids = ",".join(str(i) for i in keep)

    # 子表按会话剔除；documents 用 owner_id（owner 是会话 id）
    db.execute(f"delete from conversations where id not in ({ids})")
    db.execute(f"delete from entries where conversation_id not in ({ids})")
    db.execute(f"delete from tasks where conversation_id not in ({ids})")
    db.execute(f"delete from submissions where conversation_id not in ({ids})")
    db.execute(f"delete from documents where owner_id not in ({ids})")
    db.execute(
        "delete from document_revisions where document_id not in (select id from documents)"
    )
    # record_ids 是「表名 + 行」的登记簿，删掉悬空的即可
    db.execute(
        "delete from record_ids where id not in ("
        " select id from conversations union select id from entries"
        " union select id from tasks union select id from submissions)"
    )

    for table, cols in (
        ("conversations", ("record",)),
        ("entries", ("record",)),
        ("documents", ("record",)),
        ("tasks", ("record",)),
        ("submissions", ("record",)),
        ("document_revisions", ("content",)),
    ):
        rows = db.execute(f"select rowid, {', '.join(cols)} from {table}").fetchall()
        for row in rows:
            sets = ", ".join(f"{c} = ?" for c in cols)
            db.execute(
                f"update {table} set {sets} where rowid = ?",
                (*[redact_json(v) for v in row[1:]], row[0]),
            )

    db.commit()
    db.execute("vacuum")
    db.commit()

    counts = {
        t: db.execute(f"select count(*) from {t}").fetchone()[0]
        for t in ("conversations", "entries", "documents")
    }
    bad = db.execute("pragma integrity_check").fetchone()[0]
    db.close()

    size = out.stat().st_size
    print(f"会话 {counts['conversations']} · 条目 {counts['entries']} · 文档 {counts['documents']}")
    print(f"体积 {size / 1024:.0f} KB → {out}")
    if bad != "ok":
        print(f"integrity_check 不是 ok：{bad}", file=sys.stderr)
        return 1
    if size > 2 * 1024 * 1024:
        print("超过 2 MB 上限：调小 --keep", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
