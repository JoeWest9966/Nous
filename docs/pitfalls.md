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

## 2026-11-17 · 分支从"落后的远端 main"起，追加式文档退化成"新建文件"

**症状**：给 `decisions.md` / `pitfalls.md` 追加条目，合并时冲突是 `添加/添加`（不是尾部冲突），
而且**分支侧那份文件只有新增的几十行，正文一个字都没有**——照"两边都留"的字面解（把两份内容
拼起来）会看起来没问题，实际是拿一份残缺文件覆盖一份完整文件。

**原因**：分支是用 `origin/main` 建的，而**本地 main 比远端多了 2 个提交**（合并还没 push），
根目录那时还没有这两个文档。于是 `>>` 不是追加，是创建。

**解**：开工前 `git fetch` 并确认 base 是**远端最新的 main**（`git log --oneline -1 origin/main`
与你以为的 HEAD 一致）；解这类冲突固定用「取 ours 的完整文件 + 追加 theirs 的新增部分」，
并核对 `grep -c '^## '` 的条数不会因为合并而减少。

## 2026-11-17 · 在 CI 里断言一个 `.gitignore` 的文件必须存在 ⇒ 流水线永久红

**症状**：`verify.yml` 加了一步 `[ -f fixtures/agent.sqlite ] || exit 1`，写的时候本地有这个
文件，看着没问题。

**原因**：`fixtures/*.sqlite` 在 `.gitignore` 里（真实用户数据永不入库），CI checkout 永远没有
它，也够不着内网那台机器。**断言拿不到的东西＝每次都红**；红久了，后面真正的红就没人看了。

**解**：CI 里**删掉**这一步（不要改成 `|| true` / `|| echo skip`——那会变成"每步都绿、什么都
没验"，比红更糟），在 workflow 里留注释写清楚"为什么不在 CI 跑 + 本地怎么跑 + 哪张票会让它
能跑"。脱敏小样本进仓（`#43`）之后再加回来。

## 2026-11-18 · 「接线」验收被一行 `let _x = …clone();` 骗过去

**症状**：`#44` 交付说"server 已把手里的 driver 接到请求链路"，判据 1 也真的绿了（15/15）。
实测 `POST /v1/sessions/:id/messages` 之后库里 assistant entry 一条没多。

**原因**：那段代码是

```rust
let _driver = runtime.driver.clone();   // 注释写着"通过 TurnRequest 传给 sessions"
```

没有 `TurnRequest`、没有 `spawn`、没有 provider —— 这一行唯一作用是让 `git grep 'driver'`
有命中。**判据 1 只查依赖方向，看不见"依赖接上了但没人调"**；`git grep` 只看键名，看不见语义。
两样都指向"绿"，而功能是死的。

**解**：任何"接上了"的验收，必须有一条**从 HTTP 进来、到库里多一行出去**的实测输出
（`entries` 计数 +1、SSE 有 `message_start`），不许拿 `cargo tree` / `git grep` / 判据绿代替。
顺手记一条：注释里写"传给 X"而下面没有传，比不写注释更坏 —— 它会让 reviewer 跳过这一段。

## 2026-11-18 · `interval_ms: u64::MAX` 撞上真时钟 → `attempt to add with overflow`

**症状**：轮次的集成测试从虚拟时钟（`ManualClock::new(0)`，`now_ms()` 恒为 0）切到真时钟后，
一跑就 panic：`attempt to add with overflow`，栈落在 `generation.rs` 的
`deadline = Some(clock.now_ms() + interval)`。

**原因**：为了"这一轮只提交终态、不中途 flush"，应用层把 `interval_ms` 设成 `u64::MAX`。
虚拟时钟下 `0 + u64::MAX` 不溢出，所以测试一直是绿的；真时钟下 `now_ms() > 0` 立刻溢出。

**解**：`generation.rs` 改成 `saturating_add`。规矩：**凡是用"极大值"关掉某个定时器的地方，
都要假设它会被加到真实时间上** —— 虚拟时钟不会替你发现这类问题。

## 2026-11-18 · 第二个会话永远建不出来：`DocumentCreate` 撞同一份地址

**症状**：`POST /v1/sessions` 第一次 201，第二次 500 ——
`create refused: Document address already has a current incarnation: ["qrd3wh.sessions","session",0,0,""]`。
单测全绿（每个测试只建一个会话），真机上第二个人建会话就炸。

**原因**：会话名字存在**一份共享**的 `qrd3wh.sessions` 文档里（`names: {会话id: 名字}`，上游就是这个
形状）。建会话代码写成了「每个会话 `DocumentCreate` 一份」，而这份文档的地址没有区分键
（`kind` + `scope=session`，没有 `key`），第二个会话就撞上同一个地址。

**解**：先 `find_document` 看有没有这份文档：没有就 `DocumentCreate`（`{names:{id:名字}}`），
有就 `DocumentChange` 追加一个名字（`version + 1`）。判据：`creating_two_conversations_keeps_both_names`
（负向哨兵验过 —— 退回「每次都 Create」立刻红）。
**通用规矩**：`DocumentCreate` 之前先问「这个地址会不会已经有主」，尤其当地址里没有 `key`。
