# 本库自己踩的坑

**上游的坑不在这里**（那些在 `docs/pitfalls-from-pi.md`，按 changelog 逐条判定）。这里只记
「我们自己的代码/文档里踩出来的、下次还会再踩」的东西。**只追加**，每条写清症状 —— 症状比
原因重要，因为人是在看到症状时来搜这份文件的。

---

## 2026-11-26 · 新插一个 `impl` 块，把上一块的 `#[async_trait]` 抢走了

**症状**：`error[E0195]: lifetime parameters or bounds on method `remote_subdirs` do not match the
trait declaration` —— 报的是**另一个插件**（`env`）的方法，而你这次只动了 `repo`。

**原因**：`plugins.rs` 里每个 `impl … Host for AppState` 挤在一起，属性宏写在 impl **正上方**。
用「在某行前插入一段」的方式加新 impl 时，那段插入点选在了 `#[async_trait::async_trait]` 和
原来的 `impl` **之间** —— 于是宏挂到了新 impl 上（新 impl 全是同步方法，加了也无害），
原 impl 丢了宏，它的 `async fn` 就对不上 trait 声明了。

**下次怎么做**：插入新 `impl` 时，锚点选**属性宏之后**（`impl xxx::Host for AppState {` 这一行），
或者插完立刻 `grep -n 'async_trait::async_trait' -A2` 看一眼它下面跟的是哪个 impl。
这个错编译器的报错位置会指向受害的那个 impl（`plugins.rs:236`），而不是作业现场，很容易查错方向。

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

**症状**：判据 6 数进程构造点，`crates/Nous/Runtime/src/provider/tests.rs` 被当成生产代码
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

---

## 5 · `git push` 报 `GnuTLS, handshake failed`，但 `curl` / `gh api` 全都正常

**症状**：`git push origin main` → `致命错误：无法访问 'https://github.com/…'：GnuTLS, handshake failed`；
同一台机器 `curl https://github.com` 返回 200、`gh api repos/…` 正常。
**根因**：git 的 GnuTLS 后端在某次握手上的兼容问题（同一网络下 http 栈能通），不是凭据、不是代理、不是仓库权限。
**解**：`git -c http.version=HTTP/1.1 push origin main`。连续重试无效时别再等，直接换协议版本。

---

## 6 · j1900 上曾经有一个**同名**的 `nous.service`（8090），2026-11-18 已清除

**症状**：`systemctl cat nous` 能查到 `ExecStart=/usr/local/bin/nous serve --addr 0.0.0.0:8090`，
很容易误以为是本项目的服务（本项目将来叫 `nous-app.service`，见 `#52`）。
**根因**：那是一个**历史试验项目**（`nous 1.36.1`，静态二进制，中央-卫星二进制分发 + 外部组件登记，
`/var/nous-dist/{install-central,install-satellite}.sh`），跟本仓库**没有任何代码关系**，只是撞了名字。
**解**：已 `disable --now` + 摘单元 + 删 `/var/nous-dist`、`/root/.nous`、`/usr/local/bin/nous*`（含 12 个 `.bak`），
复验 `status` 为 not-found、`ss` 里 8090 已无人监听。**结论：这台机器上凡是 `nous` 开头的单元，
只有 `nous-panel`（旧 TS 面板，待下线）与将来的 `nous-app` 是本项目的。**

## 7 · `verify.sh` 冷启动段把产物路径写死成 `./target/release`，设了 `CARGO_TARGET_DIR` 就假红

**症状**：`CARGO_TARGET_DIR=/xxx ./scripts/verify.sh` 前 50 多条全绿，最后一条红：
`没打出 listening 行 —— env: './target/release/nous-app': 没有那个文件或目录`。
**根因**：冷启动那段写死 `./target/release/$BIN`，而同一个脚本另两处（`AUTH_BIN` / `FAUX_BIN`）用的是
`${CARGO_TARGET_DIR:-...}`。**同一个路径三处写法**，两处对一处错。
**解**：三处统一 `${CARGO_TARGET_DIR:-./target}/release/$BIN`。找不到产物时的报错要能读出路径，
别只说「没打出 listening 行」——那句会把「产物路径错」伪装成「服务起不来」。

## 8 · 在 worktree 里跑 verify.sh 却共用主树的 `CARGO_TARGET_DIR` → 主树测试假红

**症状**：worktree 里 `verify.sh` 56 ✓，回到主树同一份代码却红一两条，而且红得莫名其妙：
`小样本 fixture 应在仓里：/opt/nous-docs/crates/Nous/Durable/../../../fixtures/agent.small.sqlite`
—— 路径指向一个**已经被删掉的 worktree**。
**根因**：`CARGO_MANIFEST_DIR` 是编译期写进二进制的。在 worktree 里编、但 `CARGO_TARGET_DIR` 指向主树，
产物就带着 worktree 的路径留在主树的 target 里；worktree 一删，那些测试找什么都 NotFound。
`cargo clean -p <那个 crate>` 不够 —— 只要有一条还指着，红的就换一条（本例先红 durable，clean 后红 store）。
**解**：worktree 里跑判据一律用自己的 target（`CARGO_TARGET_DIR=./target` 或干脆不设）；
串了就这么收拾：`cargo clean` 全量重编（`clean -p` 会留下别的）。判据红的时候先看**报错里的路径**是谁的。

## 2026-11-20 · `/v1/models` 空表注释说「宿主会填」，但仓里没有宿主

**症状**：真机上 `/v1/models` 永远 `{"default":null,"items":[]}`，客户端模型栏空白；`models.rs`
开头写着「编目由宿主进程填进来」，顺着调用链找不到任何填的地方。
**根因**：一份「谁都会填」的契约没有指名填的人；空表既可能是「没配编目」也可能是「加载失败」，
客户端和运维都分不出来。
**解**：#65 给空表加 `catalog: {state, source, path}` 诊断块，编目来源落成两个明确的入口
（`NOUS_MODELS` 内联 / `NOUS_MODELS_FILE` 文件），默认模型写回同一份文件（临时文件 + rename）。
**推广**：凡是注释写「由上层填进来」的地方，都要能回答「哪一层的哪个函数填」，否则它就是没实现。

## 2026-11-20 · `root: null` 把「私库没装」说成「不知道在哪」

**症状**：三个资源端点全 200，Desk 却显示 0 条；从响应里读不出是机子上没副本，
还是服务端压根没配数据根。
**根因**：移植时把 TS 版的四条默认路径丢成 `null`，变量名还从 `NOUS_SKILLS`
变量名也漂了（多了 `_ROOT` 后缀），unit 一个都没设。
**解**：默认值回到 TS 版原值，`root` 恒为解析后的字符串；另给 `present` 与
`missing` 说"在不在"。只读，不因为缺目录就建一个空目录掩盖事实（#63）。

## 2026-11-20 · 给公开结构体加字段，把别人的测试字面量全打红了

**症状**：给 `CatalogEnvironment` 加三个远端字段后，`cargo test --workspace` 在
`nous-device/tests/catalog.rs` 和 `server/tests/device_endpoints.rs` 报五处
`E0063 missing fields ... in initializer`，跟业务行为无关。
**根因**：多个测试用结构体字面量构造环境快照（这是好事：不依赖全局 env），
但 Rust 没有「字面量自动补默认值」这回事。
**解**：每个字面量显式写 `remote_base: None, remote_token: None, cache_path: None`,
让「这个测试没有远端」一目了然；`from_env()` 里只读一次 `NOUS_*`，不藏隐式全局读取。
**推广**：改公开配置结构体时，先 `rg 'StructName \{' crates/` 看所有构造点，
再决定是补字段还是加 `#[non_exhaustive]`；别让编译错误替你找漏。

## 2026-11-21 · 冻结的解法不是空数组，是把运行时的脸露出来

**症状**：`GET /v1/extensions` 恒 `{items:[]}`，`capabilities.ext` 写着 frozen，
可 `ext/src/spawn.rs` 里 `Command::new` 早在、六个样例扩展也都在仓里 —— 客户端显示「没有扩展」。
**根因**：声明关着、代码开着，观测面没有读运行时的事实，两张真相分叉。
**解**：清单改回 enabled；`items` 直接由 Runtime 的清单/进程/状态机投影，失败项也留一行带 error；
reload 走同一个实例的唯一入口。别在 server 里再存一张注册表。

## 2026-11-21 · 命中行里出现字符串不等于打中了执行路径

**症状**：`if false && libc::prctl(libc::PR_SET_PDEATHSIG, …)` 让父死收割彻底失效，
只查 `source.contains("PR_SET_PDEATHSIG")` 的静态判据照样绿——哨兵在空转。
**根因**：字符串命中不区分注释、短路和被包在 `cfg!` 里的死分支；进程级测试又被
管道 EOF / `Runtime::drop` 两级兜底盖住，单独关掉第三级也看不出差别。
**解**：静态判据只读含 prctl 的那一行，要求它恰是 `if libc::prctl(...) != 0` 的真执行条件；
负向哨兵用 `false &&` 短路验红。进程级判据保留，作为另两级的正交覆盖。

## 2026-11-20 · 判据 3 的 `../../` 打不中 realpath 检查

**症状**：把 `files.rs` 里两处 `resolved.starts_with(&real_root)` 改成 `if false && …`，
`path_escape_is_rejected_without_creating_any_file` 照样绿——负向哨兵没变红，判据在空转。
**根因**：`../../etc/passwd` 被 `Component::ParentDir` 词法检查挡在 realpath 之前；真正需要
realpath 的是「路径片段全合法、但目录是仓内指向仓外的符号链接」。
**解**：补 `symlink_escape_is_rejected_by_realpath_check`（仓内 `link` → 仓外，写 `link/escaped.txt`），
关掉 realpath 检查时它在 `仓外不许出现文件` 断言红；同一招也用于 `git add .` 哨兵
（加 `git diff --cached --name-only` 空断言，`commit -- pathspec` 挡不住误 staged 的搭车文件）。

## 2026-11-21 · 加路由忘了更新能力快照，全 workspace 红在看起来无关的测试

**症状**：#77 加了 12 条 `/v1/repos/skills/sources|upstream/*` 路由后，单个 crate 测试全绿，
全 workspace 挂在 `capabilities_shapes::capabilities_declares_modules_and_reasons` 的 JSON 快照漂移。
**根因**：`meta.rs` 的 `ROUTES` 和 `tests/snapshots/capabilities_declares_modules_and_reasons.json`
是同一份路由表的两个副本；单 crate 迭代只跑 `nous-repo` / `nous-server` 的目标测试，不比快照。
**解**：任何加路由的票，收尾前必须跑一次全 workspace；快照漂移时从失败输出里取 `actual`
写回去（不要手敲），再复跑确认绿。
## 2026-11-20 · abort 测试里两个 Session 指向同一份库，取消信号却到不了 provider

**症状**：`POST /abort` 落了 `user_cancel` 终态、submission 也变 `unanswered`，但慢 provider
没停；断言「取消信号到达 provider」的红/绿随 `Session` 实例数量漂移。
**根因**：`Session` 的取消 token 是**实例字段**，不是库级状态。测试用 `AppState::durable()`
自己开一个 `Session`，driver 用另一个 `Session`，两者写同一份 sqlite 却各拿一个 token。
**解**：测试按生产形状装配——`DurableCommands::with_session(session.clone(), registry)` 与
driver 共用同一个 `Session`，再断言 provider 见过取消信号；负向哨兵去掉 `cancel()` 才会真红。

## 2026-11-21 · ManualClock 的 wait_until 在 set_ms 之后会丢唤醒

**症状**：测试先把假 Clock 跳到目标时刻，再调带 `wait_until` 的节拍函数，线程永久挂住；
去掉等待才过，判据不再覆盖「等待由注入时钟完成」。
**根因**：`Notify::notified()` 是先注册再检查的实现，单次 `notify_waiters()` 没有持久许可；
先 set 再 wait 时，唤醒已经过去，wait 侧看不到它。
**解**：`wait_until` 里先读时钟，再注册 `notified()`，注册后再读一次并 `tokio::pin!` 住 future
才 await；这样先跳时段也只做一次布尔检查，不依赖通知时序。
## 2026-11-22 · 运行时的五态名和 wire 的清单状态不是一套词

**症状**：install 回包写 `state:"normal"`（Runtime 内部状态），判据和 `/v1/extensions` 的 `items[]` 都要 `running`。
**根因**：`ExtState::Normal` 是状态机内部名，`/v1/extensions` 的投影在 `runtime::items` 里翻成 wire 的 `running`，install 是新路径，容易漏翻。
**解**：所有出 HTTP 的状态都过一次 `normal → running` 映射；实现写在端点翻译层，不在 CLI/AdminStore 里。
**推广**：新写端点回 state 时先看 `runtime::items()` 的同一段映射，别直出 `ExtState::as_str()`。
## 2026-11-21 · 归档墓碑有两套形状，四态判据会悄悄退化

**症状**：#64 的 purge 把墓碑写成 `purged:{id,at}`，#67 的 `/v1/sessions` / `/v1/archive`
读的是 `archived[id]={at,gen,purged}`；两边各自测试都绿，合起来时「已销毁」永远显示成
「无墓碑」，purge 也过不了「先归档」的前置。
**根因**：两张票分头落，墓碑是同一份 `qrd3wh.sessions` 文档的两个键，没有单一真相。
**解**：#67 收口为唯一形状 `archived[id]={at,gen,purged}`；archive 写 `purged:false`、
purge 只翻这一位且必须先有墓碑、restore 抹掉整个键；wire.md 同步改口径。

## 2026-11-20 · 变体分支没有 upstream，裸 `git push` 报假失败

**症状**：`publish` 在变体 worktree 里 commit 成功，但 `pushed=false`，远端根本没有变体分支。
**根因**：`git push --porcelain` 在 `push.default=simple` 下要求当前分支有 upstream；
worktree 新建的 `skill/<id>` 分支没有 upstream，push 直接失败。
**解**：显式 `git push origin HEAD:refs/heads/<branch>`；没有远端才回 `Unknown`（wire null），
推送失败仍回 `false`，绝不冒成功。哨兵：把 refspec 改成 `main` → 判据 2 的远端主干不变断言红。

## 2026-11-21 · #88 的清单快照判据是「备注」列当「API 面」用

**症状**：合并 #88 后给 `docs/plugins.md` 的 `repo` 行加说明文字，
`capabilities_manifest::manifest_metadata_matches_the_plugins_table_cell_by_cell` 变红，
报「六列一行都对不上」，但六列肉眼看全对。
**根因**：该测试把 `cells[8]`（备注）与 `ManifestEntry.description` 比，而行里 `cells[6]`（API 面）
才是描述列；集成前两格恰好相同，所以空转地绿。
**解**：改清单表格时若撞上这条红，先逐列打印 `cells[i]` 与 entry 字段确认索引；
保持 `cells[8]` 与 `MANIFEST.description` 逐字相同，#77 的上游追踪说明放 `cells[6]`。
## 2026-11-22 · 发布闸有两层，负向哨兵只拆 HTTP 那层不会红

**症状**：#80 做「坏 schedule → 400」的哨兵时，把 HTTP 层的 `validate_rules` 拆掉，
判据仍然绿 —— 坏定义在 `Trigger::new` 里又被 `parse_five_field_cron` 拒了一次。
**根因**：闸被写了两遍（HTTP 翻译层 + 定义层 conformance），只拆外层时内层还在守门，
判据测的是「有没有被拒」，不是「哪一层拒的」。
**解**：哨兵要拆到**唯一真正的写入闸**（这里是 `Trigger::new` 的 cron 校验）；两层都保留，
外层给好人话 400、内层是 conformance 的物理保证。判据注释里写清哨兵该拆哪一层。

## 2026-11-23 · sleeping 任务里的 kind 在库里是编码字符串，不是原文

**症状**：进程级判据 `SELECT count(*) FROM tasks WHERE kind='nous.dispatcher.tick'` 恒为 0，
但 durable 的 record JSON 里明明有这条任务；误以为节拍没跑。
**根因**：durable 按 `docs/storage.md` 的索引列规矩写 `kind`，`task_row` 用
`encode_indexed_string` 编码（带引号），字面量匹配不上；`record` 才是原文真相。
**解**：判据改成 `WHERE record LIKE '%nous.dispatcher.tick%'`，或先经 dispatcher 的
`read_sleeping_ticks` 解析 record；不要拿索引列的字面量当 JSON 里那个字符串。

## 2026-11-22 · 试跑的「问触发器」与手动 launch 不能共用判定入口

**症状**：#82 两个动作看起来都是「起一条会话」，把 launch 也接到 `evaluate` 后，
不命中的 cron 让手动 launch 变成 `{fired:false}`，会话和血缘都不写。
**根因**：`forced` 不是回包上的一个词，它是「有没有经过触发器」这件事本身；
判定一旦共用，跳过判定的入口就不存在了，只能靠回包自说自话。
**解**：`fire` 单独持有判定与「不 fire 就不建会话」的早退，`launch` 只进共用的 launch
路径（建会话 → 血缘 → 回执），判定不参与；哨兵分别弄坏两处，各有对应判据变红。
## 2026-11-21 · 契约图表内嵌在判据脚本里，文档补图追不上

**症状**：#89 按票面在 `docs/harness.md` 补了 7 条 `nous-server` 依赖边，
`verify.sh` §4e 仍红，且脚本里那份 allow 表与文档是两份真相。
**根因**：判据读的是内嵌 dict，不是它声称要读的契约文档；改文档对判据没有任何影响。
**解**：机器版图表移进 `docs/dependency-graph.toml` 由脚本直接读，文档 §7 是同一张表；
补“空表 = RED”哨兵。断言循环逐字未变；哨兵：从表里删一条边 → §4e 红。
## 2026-11-24 · 归档的 generation 文件与墓碑库写不是同一事务，失败会留孤儿

**症状**：generation 已写好但墓碑提交失败 → 盘上有 `gen-*.db`，库里没墓碑；restore 反向失败 → 行已写回但墓碑还在。
**根因**：generation 是独立 SQLite 文件，墓碑是活库文档写，`Storage::commit` 的事务只覆盖后者。
**解**：按顺序写（临时文件 → rename → 墓碑），失败补偿反向删除；restore 失败保留 generation。注入哨兵验盘/库都不留半截。

## 2026-11-22 · 删插件装配时把别的端点一起带走了（#90）

**症状**：删掉 `documents::attach` 后 `/v1/_routes` 变 404，`route_registry_and_enabled_endpoints_are_the_same_multiset` 红，而改动看起来只是删 documents。

**根因**：`documents::attach` 顺手注册了 `_routes` 并做了 `with_state`；删模块时那两件事也必须归位到主 Router，顺带的职责不会自己搬家。

**怎么防**：删一个 `attach` 前先看它注册了哪些路由 / 做了哪种 state 收敛，逐条找新家；判据必须包含 `_routes` 与 capabilities 的一致性，不能只测被删的路径 404。

## 2026-11-22 · 同批写入里 id 撞了不会报错，是静默 refused（#91）

**症状**：测试里提交 `Conversation{id:1}` + `Task{id:1..6}`，`commit` 返回 `Ok`，库里的 `tasks` 却是空的，`--active-tasks` 数出 0。

**根因**：`record_ids` 是**跨表**的唯一表，同批里 task id 撞上 conversation id 时那一笔按「重复」处理，进 `refused` 而不是报错。

**怎么防**：造种子数据时各表 id 错开（conversation 1 / task 10+）；断言要落到「库里真有几行」，不能只看 `commit` 返回 Ok。

## 2026-11-22 · 老模板没有 `spec`：combos 写开关 400，presets 再存一遍也 400

**症状**：`PATCH /v1/combos/:section/:name` 回 `400 preset.json 没有 spec 对象`；对同一条走 `PUT /v1/presets/:section/:name` 回 `400 preset.json 缺字段 spec`。库里 5 条 0.1.7 之前写下的模板就是这样（顶层 `v/name/summary/category/...`，没有 `spec`）。

**根因**：`spec` 从「扩展位」被当成了必需字段；combos 的 `view` 用 `unwrap_or(Null)` 兜住了，写路径却直接 400 —— 读宽容、写严格，两边不一致。

**怎么防**：写路径与读路径同宽：`spec` 缺了就补 `{}` 再写（combos `patch`、presets `validate_body` 两处），`name`/`summary`/`category` 才保持必需；判据是「老形状能读、能改、能存」三条一起测。

## 2026-11-22 · 调度器每一拍都写库失败：`IdAlreadyBelongs { id: N, table: Conversation }`

**症状**：j1900 日志每 60s 一条 `nous-dispatcher: 第 N 拍写库失败：tick task commit refused: Some(IdAlreadyBelongs { id: 2, table: Conversation })`；`lastTickAt` 永远 null、`lagMs` 恒 0，但服务本身不报错 —— 健康判据是假绿。

**根因**：`arm_next_tick` 拿拍号（进程内 0、1、2…）当 tick 任务行的 `id` 直接提交；`record_ids` 是**跨表**全局唯一表，这些 id 早被真实会话 / 文档 / entry 占了，`check_ids` 整批拒绝。单测用的空库没有这些行，所以一直是绿的。

**怎么防**：id 只能来自 `Storage::mint_id`（新增 `nous_durable::session::commands::arm_sleeping_tick` 统一铸造 + 提交），拍号只住 `input.seq`；回归判据是「先建两条会话占掉 id 2/3，再写第 2 拍必须成功」——直接复现线上拒绝，不许再拿空库测 tick。

## 2026-11-22 · 页面上凭空多一条「加载失败：read …/data/nous.json: No such file」

**症状**：扩展清单里冒出一条名字叫 `data` 的「坏插件」，错误是读不到 `nous.json`；去查那条
扩展本身永远查不出问题（因为它根本不是扩展）。

**根因**：变体绑定表住在 `<NOUS_EXTEND>/data/extensions.local.json`（`repos::variant_tables_from_env`），
而 `manifest::scan` 把扩展根下的**每个目录**都当成候选扩展。运行时的数据目录被当成坏插件上报。

**怎么防**：`manifest::RESERVED_DIRS = ["data", "artifacts", "trash"]` 在扫描时跳过（点开头照旧跳过）；
判据 `runtime_data_dir_is_not_an_extension`——根下同时放 `data/` 和一条真扩展，只许扫出真扩展。

## 2026-11-24 · 真供应商收到的请求 `messages: []`：模型对着工具清单自说自话

**症状**：j1900 上真发一条「1+1 等于几」，助手回「你好！我在 /srv/nous/work 工作目录里，
目前没有收到具体任务」。抓供应商请求体：`{"messages":[],"tools":[…]}` —— 连 system 都没带。

**根因**：轮次装配上下文时从 `id = 1` 往上逐个 `get_entry`，**遇到第一个缺失就 `break`**。
entry id 是**全局单调**的（一条序列被所有会话共用），真库里 id 从 24041 那种数字开始，
第一跳就是 `None` → 整段转录被丢掉。单测库的 id 恰好从 1 连续，所以一直是绿的。

**怎么防**：上下文只能按**会话**查（`nous_durable::session::transcript::entries_in`，
按 id 降序取最新 N 条再翻转）；判据写进 `providers_elements.rs::a_real_turn_answers_through_the_elements_provider`
—— 断言供应商请求体里有用户那句话（`contains("ping")`），不只看答案落库。

## rsync 到残留的 /tmp 目录会把 docs 多嵌一层（2026-11-24）

**症状**：第二次跑 `deploy.sh`，在「落产物」那步 `cp: /tmp/nous-docs/plugins 没有那个文件或目录`。

**根因**：`rsync -a $stage/docs $HOST:/tmp/nous-docs` —— 目标目录上次中断时还在，
rsync 把源目录**名字**也带过去：内容成了 `/tmp/nous-docs/docs/plugins`；脚本按
`/tmp/nous-docs/plugins` 取，于是落空。首次部署不会撞上，第二次才炸。

**怎么防**：同步前先 `rm -rf` 目标，源和目标都写尾斜杠（`$stage/docs/` → `/tmp/nous-docs/`），
语义才是「把里面的内容放到那里」；`upload_artifacts` 与 `--docs-only` 两处都按这个写。

## 共用 main 工作树里未提交的改动会被别人的 commit 卷走（2026-11-24）

**症状**：本地改的 `scripts/deploy.sh` 在 `git status` 里消失，却出现在别人推的 commit 里
（message 也跟内容无关）；`git log -S` 才查得到。

**根因**：`/opt/Nous-src` 是多会话共用的 main 工作树，别人 `git add -A` 不区分「谁改的」。

**怎么防**：动代码前先 `git worktree add /opt/Nous-src-<主题> -b fix/<主题>`，共用 main
上一秒都不留未提交文件（AGENTS.md 的「一个 worktree 一个分支」就是为这个）。

## 远端开发机的 `cargo` 是 wrapper，会盖掉你传的 `CARGO_TARGET_DIR`（2026-11-25）

**症状**：在远程工作树里 `CARGO_TARGET_DIR=/workspace/av-target cargo test --workspace`，
`nous-device/tests/manifest.rs` 报 `cargo metadata 可跑: NotFound`；单跑 `-p nous-device` 却是绿的。
`verify.sh` 同一条（第 1 段「有失败」），其余全绿。

**根因**：`/usr/local/bin/cargo` 是个 bash 包装：`export CARGO_TARGET_DIR=/workspace/nous/target`
之后才 `exec` 真 cargo。于是「工作树 A 的测试二进制」配「工作树 B 的产物目录」，
测试里 `Command::new("cargo")` 又继承了这个被改写的环境 → 找不到可执行文件 / 读到别人的源。

**怎么防**：远程跑判据时显式绕过 wrapper —— `PATH=/workspace/cargo/bin:$PATH` +
`CARGO_TARGET_DIR=<本工作树专用>`，别依赖 `/usr/local/bin/cargo`。共享一个 target 目录
还会让两个工作树的 `build.rs` 生成物互相顶掉（`plugin_versions.rs` 就是其中之一）。

## axum 同一 path + 同一 method 声明两次 = 启动期 panic（不是「后者覆盖」）

**症状**：`Route::new("GET", "/v1/x/file", .. || get(a).put(b))` 再补一条
`Route::new("PUT", "/v1/x/file", .. || put(b))` —— 编译过、单元测试过，
真起 `nous-app` 时 axum 在 `route()` 里 panic（`Overlapping method route`）。

**根因**：`Router::route()` 对已存在的 path 会**合并** `MethodRouter`，合并到同一个 method
才报错；`GET` 那条里 `.put()` 已经把 PUT 用掉了。

**怎么防**：一条路径上每个 method 只声明一次 —— `GET /file` 那条只写 `get(file)`，
`PUT /file` 另起一条 `put(save)`（`library-skill` / `nous-prompts` / `library-extend` 都是这个写法）。
声明条数也因此等于端点数，别为了「看起来聚合」把两个 method 塞进一条。

## 搬家 + 改名放在一张票里：先跑一遍「只改名」的编译，再搬端点

**症状**：一次改完目录名 + 包名 + 15 条端点，编译错误混在一起分不清是改名漏了
（`nous_ext::` 没换）还是端点搬错。

**根因**：改名是**全局机械替换**（crate 标识、Cargo 依赖、`plugin.toml`、判据脚本里的
路径字面量），搬家是**语义改动**；两者失败的信号长得一样。

**怎么防**：先把改名做完、单独过一次 `cargo check --workspace --all-targets`（绿了再搬），
要不要拆成两个提交随意 —— 关键是「改名没漏」这个信号别被搬家的错误淹掉。
判据脚本里的路径字面量（`verify.sh` 的 spawn 哨兵、
`docs/boundary-baseline.txt`、`*_endpoints.rs` 里 `workspace_root().join(...)`）
改名时最容易漏 —— 它们不在 React/cargo 的视野里，只有 `verify.sh` 会红。

## 单段 PascalCase 全名被版本判据跳过（2026-11-26）

**症状**：`Skill` / `Models` 这类无 `_` 的 PascalCase 全名在 `plugin_versions.rs` 的
`module_name_ok()` 里既不匹配 `Vendor_Module` 也不是小写连字符名 → `plugin_dirs()`
跳过它，版本比对空转而测试仍绿。

**根因**：规则 12 允许单段全名，但判据只实现了两段与 legacy 两支；空集不报红。

**怎么防**：判据补 `single_pascal` 一支，并钉正反哨兵：`dir_of("Skill")=="Skill"`、
`dir_of("Models")=="Models"`、`module_name_ok("Skill")` 为真，同时
`Nous_device` / `nous-device_Extra` 仍为假。新增全名形态时先补判据再改清单。

## 2026-11-26 · `docs/plugins.md` 的表格行里不许出现裸 `|`

**症状**：`capabilities_manifest` 报「六列在 docs/plugins.md 里一行都对不上」，
打印出来的行看着完全正确。

**根因**：那一行里写了 `` `PATCH|PUT /v1/triggers/{code}` `` —— 表格用 `|` 分列，
裸的竖线把一行切成两半，`split('|')` 得到的不是 10 格，六列比对自然全错。
同一类还有**行尾漏了 `|`**（少一格，cells 长度也跟着变）。

**怎么防**：端点列表里要写两个方法就写「`PATCH / PUT`」，不要用竖线；加完行先数
`awk 'NR==行号' | grep -o '|' | wc -l` 应该是 9 个。

**症状**：只把 `plugin.toml` 的 `name` 从 `Element_Models` 改成 `Models`（目录、包名、capability、
组 id 一个字没动），`scripts/plugin-versions.py --doc-check` 立刻红两条，`capabilities_shapes`
的字段快照也跟着漂。

**根因**：这两处都按「插件身份」算，不看目录 —— `MANIFEST_KEYS` 里有 `name`，改它就是 minor
（考古从 `0.2.0` 跳到 `0.3.0`）；快照里 `modules[].version` 报的是插件自己的版本（规则 8），
版本一涨快照就漂。改名票的「只改一行」是假的：还要改 `meta.rs` 的 `MANIFEST`、`/v1/_routes`
的 `MODULE`、版本线表（`Models` 与顺带 +patch 的 `server` 两行）、快照。

**怎么防**：改全名前先按这五处列清单（`plugin.toml` / `MODULE` / `MANIFEST` / 版本线表 /
快照），改完依次跑 `check-plugins.sh`、`plugin-versions.py --check --doc-check`、
`cargo test -p nous-server`。别用 `--write-doc` 图省事 —— 它会顺手改写其它插件那几行。
