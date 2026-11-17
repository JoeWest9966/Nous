# 本库自己踩的坑

**上游的坑不在这里**（那些在 `docs/pitfalls-from-pi.md`，按 changelog 逐条判定）。这里只记
「我们自己的代码/文档里踩出来的、下次还会再踩」的东西。**只追加**，每条写清症状 —— 症状比
原因重要，因为人是在看到症状时来搜这份文件的。

---

## 2026-11-17 · 文档里写了一个真控制字符 ⇒ 这份文档从搜索里消失

**症状**：`rg '500ms' docs/` 明明该命中 `docs/storage.md`，输出里却整个没有这份文件；`grep`
直接跳过；换 `-a` 才看得见。你会以为「我记错了，没写过」，然后**再写一遍**同一件事。

**原因**：`docs/storage.md` 里有一行

```
| 孤立代理对与 `\x00` 往返无损 |
```

那个 `\x00` 是**真正的 0x00 字节**，不是转义文本。`grep` / `rg` 见 NUL 就判定整个文件是
二进制，默认跳过（`rg` 只给一句 "binary file matches" 甚至什么都不给）。367 行的文档因此对
所有搜索隐形。发现它纯属偶然：搜 `500ms` 时那份文件在结果里缺席。

**怎么修**：写成转义文本 `\u0000`（Markdown 里就是在说「这个字符」），别放真字节。

**怎么防**：改完文档跑一遍扫控制字符（这次一次性扫了全库 `.md`/`.rs`/`.toml`/`.sh`/`.tsv`/
`.json`，除 Tab/换行外 0 个）：

```bash
python3 - <<'PY'
import pathlib
for p in pathlib.Path('.').rglob('*'):
    if 'target' in p.parts or '.git' in p.parts or not p.is_file(): continue
    if p.suffix not in {'.md','.rs','.toml','.sh','.tsv','.json'}: continue
    for i, ch in enumerate(p.read_bytes()):
        if ch < 0x09 or ch in (0x0b, 0x0c) or 0x0e <= ch < 0x20 or ch == 0x7f:
            print(p, i, hex(ch)); break
PY
```

**推广**：任何「我写过但搜不到」的文档，先怀疑它是二进制 —— 一个字节就够。

## 2026-11-17 · 复制 SQLite 库时残留的 `-wal` 会在新库上重放 ⇒ 两个库的行混在一起

**症状**：`cp fixtures/agent.sqlite /var/tmp/x.db` 之后，`x.db` 里多出一条**不属于这份 fixture**
的记录（内容还是上一次测试写进去的），但 `entries` 计数、`next_id` 全都自洽 —— 没有任何报错。
`fixtures/agent.sqlite` 本身是干净的（209 条），所以查源头会查到「代码很干净、数据很脏」。

**根因**：`/var/tmp/x.db-wal` 是上一次测试留下的（那次提交还没 checkpoint 就退出了）。`cp` 只覆盖
主文件，SQLite 打开时把**旧 WAL 重放到新主文件**上 —— 两个数据库的行就这么合并了。

**解**：拷之前先 `rm -f /var/tmp/x.db{,-wal,-shm}`，或对源库 `PRAGMA wal_checkpoint(TRUNCATE)`。
调试时先 `select max(id)` 和自己手上的 fixture 对一眼：**数字对不上就是重放，不是代码 bug**。

## 2026-11-17 · 排除式多写一个 `_`，集合从 2 变 3 却看不出是写错了

**症状**：判据 6 数进程构造点，`crates/nous/runtime/src/provider/tests.rs` 被当成生产代码
算进集合，红得不讲道理；而判据 5 里同样目的的排除是好的。

**原因**：排除式写成了 `_tests?\.rs$`，而文件名是 `tests.rs`（**没有下划线前缀**）——一个
`_` 让那个文件漏网。当时判据只打印"集合 N 个"，不打印**排掉了哪些行**，所以 2→3 看起来
像"数据变了"，不像"排除写错了"。

**解**：排除必须可观测——每条判据打印「排掉了 N 行，来自 <文件×次数>」并断言 `N > 0`。
今天 verify.sh §5 的判据 5/6/9/11/12/13 都已改成这个形状。

## 2026-11-17 · 在 main 工作树里 `ln -s` 别的 worktree 的 target ⇒ 冷启动判据验的是别人的产物

**症状**：合并后没编译，为了让"冷启动 rc=124"这条过，建了 `target -> /opt/wt/i14/target`；
verify 立刻全绿。

**原因**：判据验的是"二进制起得来"，但二进制来自另一个工作树。那个 worktree 一删、一切分支、
产物一过期，绿就变成谎话，而且是**静默的**。

**解**：仓里不许出现 `target` 符号链接（这台机器的 `CARGO_TARGET_DIR` 本来就全局指向
`/opt/Warehousetui/target`）；要么显式给 `CARGO_TARGET_DIR`，要么在 verify 里先编。
