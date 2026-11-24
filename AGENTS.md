# Nous


**架构一句话**：NOUS 不是代码，是一张清单（发行版 metapackage）。内核只剩三样 ——
**契约**（`wire.md`）· **装载器 + 版本闸** · **调度**。其余全是插件，由 `docs/plugins.md`
那张表组成；表管装什么，`crates/` 管怎么实现。

先看三份，按这个顺序，别跳：`wire.md`（对外契约，权威）→ `docs/plugins.md`（模块与四层的
唯一真相）→ `docs/pitfalls.md`（踩过的坑，三行一条）。

**加端点 / 搬端点之前**还要看 `docs/magento2-migration.md`：端点归插件（模块自带 handler），
`server` 只做监听与分发；那份文档同时是搬家进度表。

## 布局

```
crates/<vendor>/<module>/   ← 插件边界就是 crate 边界；目录只表达所有权
exts/                       ← 运行时外挂（按帧协议走 stdio，以 `exts/*` 编进 workspace）
wire.md                     ← 对外契约（宪法）：只在这里改契约
docs/                       ← 设计与判据（plugins / deploy / storage / triggers / permissions / decisions / pitfalls）
scripts/                    ← 判据与部署，唯一的"做完了"来源
deploy/nous-app.service     ← j1900 上的那个单元
```

旧面板的壳子（`server/*.ts`、`templates/`、`vite.config.ts`、`tsconfig.json`、`package*.json`、
`.env.example`、`deploy/nous-panel.service`）**已在 #59 从仓里删掉**。要回旧面板就从 git 历史读
（别把壳子请回来）；j1900 上的 `nous-panel.service` 还留在 `/etc/systemd/system`，可回。

**盘面/文档/进程形态（#86 拍板，细节见 `docs/decisions.md` 2026-11-20 四条）**：v1 的文档真相是
`documents` 数据库文档（`kind` / `section` / `name` + 修订），不复活 `content/<board>/tree.json`、
`map/<node>/*.md` 文件树、`.trash/`、`.nous/locks.json` 与 `/p/:board/:id/*` 反代。树/文件夹语义
由 **#87** 在 documents 上加（不是文件形态）；单用户无并发写，冲突走修订版本与 409，多人同盘
编辑是重开服务端锁的反悔条件；块进程统一由 `ext` 承担（**#81** 解冻），不另起按 node 的路由。
`content/` 已于 2026-11-18 归档出仓库，盘面节点/块不是 v1 的概念。

## 常用命令

```bash
cargo check                                     # 日常
cargo test --workspace                          # 测试
env -u CARGO_TARGET_DIR ./scripts/verify.sh     # 唯一的"做完了"定义（跑不通 = 没做完）
./scripts/check-plugins.sh                      # 模块/层/版本 与 docs/plugins.md 逐行对齐
./scripts/check-separation.sh                   # 两库分离（nous.db / nous-plugins.db）
./scripts/judge.sh                              # 附录 A 的 case 执行器（SKIP 不是绿）
./scripts/layer-paths.sh kernel                 # 取某一层的模块路径
env -u CARGO_TARGET_DIR ./scripts/deploy.sh     # 编 musl → 送 j1900 → 装 unit → 重启
NOUS_APP_BIN=/path/nous-app ./scripts/deploy.sh # 用现成静态产物（编译在别处做），跳过编译
ssh j1900 '…' scripts/deploy-check.sh --wait=20 # 部署后的真行为判据（在 j1900 上跑）
```

`env -u CARGO_TARGET_DIR` 不是装饰：这台机器的全局值指向仓外，不摘掉产物就落在别处。

## 硬形状（动代码之前先认这几条）

- **归位制**：新增任何东西（crate、扩展、数据目录、管理面），先在 `docs/plugins.md` 加一行，
  再落代码。清单里没有的目录 = 红。
- **`layer` 四值**：`kernel`（真相层，**零进程、零网络、零直接时钟**，要时间就注入 `Clock`）·
  `platform`（能力层，允许 IO/进程，但必须有"无 IO 替身"）· `app`（唯一 spawn 处、唯一监听处）·
  `dev`（夹具与探针，不进任何产品产物）。`kernel` 的依赖闭包里不许出现 `platform` / `app`。
- **`plugin.toml` 是清单数据，API 章是生成物**：`name` / `layer` 逐字等于 `docs/plugins.md`；
  `capabilities` 的 `enabled` = 清单 ∩ 代码，`reason.code` 四值封闭。`check-plugins.sh` 是判据。
- **协议是哑的**：客户端只转发 `/v1/*`，不加业务字段、不在客户端算状态。未知字段用
  `serde_json::Value` 收着原样透传 —— `deny_unknown_fields` 会让引擎每加一个字段这边就编译不过。
  唯一例外：本地配置的 `Spec`（fail-fast）。
- **存储分离**：`nous.db`（durable，会话与任务）与 `nous-plugins.db`（store，插件自己的表）
  各有独立迁移链、单向依赖、跨库只有一处且顺序写死（先 durable 再 store，幂等 + 补偿）。
  要加第二写者先签字，见 `docs/storage.md`。
- **触发器**：`Trigger::new` 是唯一构造入口；`code` 是不可变 slug；一切规则是触发器自己的列；
  模板是零件没有生命周期；一次点火只起一轮。见 `docs/triggers.md` §8。
- **观测 = 每个插件对自己那一半的交代**，不是全局 telemetry 服务；`counts` 从 `fires` 表
  `GROUP BY state` 算，不用内存计数器。
- **调度不自己写循环**：定时/唤醒走 durable 的 `sleeping` 任务 + 注入的 `Clock`。

## 判据的规矩

判据比代码重要 —— 空跑的绿比红更危险。

- **SKIP 不是 PASS**：路径不存在、载体缺失，一律打印 `SKIP` 到 **stderr** 并算未完成。
- **断言打命中行**：`… | tee` 输出，不用 `grep -c`（计数绿了，看不出命中的是哪一行）。
- **不同源比**：版本号这类值必须三方给出（磁盘 sha8 / CLI 自哈希 / `/v1/device` 自报），
  不是读同一个常量三次。
- **负向哨兵**：故意弄脏一次（`lag` 恒 0、`counts` 不读库、CLI 换 pretty），必须变红；
  不变红就是判据在空转 —— 先修判据，再修代码。
- **排除项只许是目录或整块**（减层），单行豁免一律是 smell。

## 部署与运行

```bash
# unit 里这三行是一对：绑非回环就必须有 token，缺了进程 refuse 退出 78（EX_CONFIG）
Environment=NOUS_LISTEN=0.0.0.0:4789
Environment=NOUS_TOKEN_FILE=/etc/nous/token
Environment=NOUS_DB=/var/lib/nous/nous.db
```

- **退出码 78 = 拒绝启动**（库比代码新 / 没 token / workdir 不可写），配 `RestartPreventExitStatus=78`
  让它停在 failed，而不是每 3 秒拉一次。崩溃仍是 2。
- **只出 musl 静态产物**；j1900 **只跑产物**，不 clone、不编译、不装 `sqlite3` CLI
  （运维走二进制自己的 `--backup` / `--check` / `--active-tasks` / `--triggers`）。
- 数据全在 `/var/lib/nous/`（`nous.db`、`nous-plugins.db`、`backups/`）。那是**线上会话记录**，
  只读、只经 CLI 动。
- 部署幂等 = 产物 + unit 两样都比 sha：只看二进制，改 unit 会静默不生效。
- 环境变量入口见 `grep -o 'NOUS_[A-Z_]*' -r crates/ | sort -u`；不要新增"只在一个分支里读过"的变量。

## 分支与交付

- **一票一 worktree**：`git worktree add /opt/nous-<主题> -b feat/<主题> origin/main`，
  干完 `--no-ff` 并进 `main`，再 `git worktree remove`。`main` 只由合并进入。
- **做完就停**：本地提交、跑完判据，把分支与验证方法交回用户；用户说「没问题」之后才合并、
  打 tag、部署、关票。
- **tag 是语义化版本**，默认 patch +1（升 minor/major 要用户明说），一处真相三处同步：
  `Cargo.toml` 的 `[workspace.package] version`、每个 `plugin.toml`、tag 本身。

## 红线

- **密钥只在内存**：不落盘、不进日志、不进会话记录、不进库。
- `fixtures/agent.sqlite` 只读；`/var/lib/nous/` 只经 CLI 动。
- 不为过判据开第二条写通道；协议加字段先改 `wire.md` 与类型，再让客户端跟。
- **编译丢给专用开发机，本机只跑产物**：`ssh <开发机>` 上 `CARGO_TARGET_DIR` 指到工作区盘
  （512G），别让它落根分区（256G，容易满）；产物拿回来用 `NOUS_APP_BIN=… scripts/deploy.sh` 部署。
- 只做 Linux（开发机是 Debian 13 + Wayland）。Windows 的源码与打包配置原地冻结：不改、不删、不出包。
- 踩一个坑就往 `docs/pitfalls.md` 追加一条（症状 / 根因 / 解，三行以内）；选了什么、为什么、
  什么时候该反悔，写进 `docs/decisions.md`。两份都是追加式：冲突时**两边都留**，不许整份取一边。

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.
