# to-build · 方向台账（还没开票的东西）

这份文件记**方向**，不记任务：一个方向能被讨论、被拆票、被否决；一张票只能被做完或关掉。
有票的进「已在票上」，只在这里活着的是**还没想清楚的**。

规矩三条：

1. **一条一段**，末尾必须带「还没定的硬问题」—— 一个提不出硬问题的方向，说明还没想，
   不该出现在这份文件里。
2. 讨论出结论就搬走：进 `docs/decisions.md`（决定 + 反悔点）或变成票。
3. **不许在这里排期**。顺序由 `AGENTS.md` 的主线决定，不由本文件的书写顺序决定。

## 已在票上（不在本文件展开）

| 票 | 方向 |
|---|---|
| `#47` | 让 NousDesk 连上（7 端点 + `/v1/capabilities`） |
| `#48` | `documents` 模块（16 条 presets 收敛）+ `/v1/_routes` |
| `#49` | 砍 board/block/resolve —— 一个地址 + capabilities 服务表 |
| `#50` | `/v1/*` 认证（绑非回环就硬门槛） |
| `#51` | faux provider（验收用确定性回复） |
| NousDesk `#8` | 客户端切「一个地址 + capabilities」（已锁，等 `#47`/`#51`） |

---

## 多主体 · 钥匙即权限（**AI 也是一个主体**）

**一句话**：NOUS 里「谁在调」不只是人。`principal` 可以有三种：人、**AI**、系统。
钥匙（API key）不是通行证，是**一束能力**：能碰哪些端点族、能用哪些模型、能调哪些工具、
能看哪些会话、烧多少预算。**AI 拿着受限钥匙干活，包括拿它来建 NOUS 自己**（写 skill、写模板）。

**为什么这是 NOUS 特有的**：Magento 2 有 ACL 但主体只有人；pi 是单用户单钥匙；
SaaS 那套是「用户 = 计费单位」。这里反过来：**主体是执行者，钥匙是它的权限切片**，
而「AI 用钥匙改自己的宿主」是这套东西必须支持的**常规操作**，不是边缘情况。

### 已经能看出形状的部分（讨论的起点，不是结论）

- **principal**：`{ kind: human | agent | system, id, label }`。`agent` 的 id 指向某个会话/扩展，
  这样审计里能回答"这条 entry 是哪个 agent 写的"。
- **钥匙**：生成时显示一次明文，库里只存**哈希 + scope**（`nous_sk_…` 前缀便于肉眼识别）。
  `#50` 的单 token 就是「一个 root principal 的钥匙」，不冲突——`#50` 是它的退化情形。
- **scope 至少四维**（互相独立，不是层级）：
  1. **端点族**：`presets:write` 禁 / `skills:write` 许 —— 粒度到 `(module, action)` 而不是整个模块
  2. **模型白名单**：不在名单里的模型，`/v1/models` **就不该返回**（客户端因此不会误导人）
  3. **工具白名单**：`bash` 禁 / `read` 许；无人值守的触发器必须有这一维
  4. **数据范围**：`own`（只看自己起的会话）/ `shared`（看全部）
- **effective capabilities 是两层的乘积**：服务端有什么（`/v1/capabilities`）× 这把钥匙能用什么。
  客户端要先问「我是谁、我能干什么」（`/v1/whoami` 之类），再决定灰哪些入口。
- **AI 建东西走提案，不走直写**：AI 提交 proposal → 人类 principal 批准 → 才落地。
  `documents` 的 `pending` / `publish`（`#48` 已经要建的那条通路）**天然就是这条通道**，
  不需要为权限再造一套审批。这也顺手回答了"AI 改坏了自己怎么办"：它只能攒待发布。

### 参照物：Magento 2 核心 ACL + Amasty Advanced Permissions（用户 2026-11-18 指定）

用户的原话是：**"把 NOUS 理解成 Magento 2，那这个功能就相当于 Amasty 的 Advanced Permissions"**。
这个类比很有用，因为那个插件存在的**理由本身就是一条架构教训**：

- **Magento 核心 ACL 只有一棵资源树**（`role` → `resources`，`Magento_Backend::admin` 往下继承，
  allow/deny 逐层合并）。树能表达"能不能进这个菜单/调这个 action"，表达不了别的。
- **Amasty 那个插件补的全是树外的正交维度**：按 website / store view 限定管理员、
  只许碰**某些具体商品/分类**（实体级）、只许改**某些商品属性**（字段级）、
  订单按 store / 支付方式 / 配送方式限定、权限可以**限时**发放。
- 它的实现方式是**一堆 plugin 挂在各处**拦 —— 那是 Magento 的扩展机制决定的，**不是我们该抄的部分**。

所以抄法分两半：

| 抄 | 不抄 |
|---|---|
| **数据模型**：核心的「规则表 + 求值器」，而不是每加一维就加一个特判 | Amasty 的「每维一个 plugin 到处拦」 |
| **正交维度**必须是数据，不是代码分支 | 它的具体产品语义（store view / 商品属性这些我们没对应物） |

落到 NOUS 就是**一张规则表、一个求值器**：

```
Rule { principal, resource, selector, effect, window }
```

| 字段 | 是什么 | Magento 里的对应 |
|---|---|---|
| `resource` | 端点族 + action 的**树**（`documents.presets.write`、`sessions.message`、`tools.bash`、`models.<id>`） | 核心 ACL 资源树 |
| `selector` | 作用对象的**谓词**（哪个 document kind / 哪个 section / 哪些字段 / 哪些会话 / 哪个 workdir） | Amasty 的 store / 实体 / 字段限定 |
| `effect` | `allow` / `deny` / **`propose`** | 核心只有 allow/deny |
| `window` | 有效期 + 预算（token / 调用次数） | Amasty 的限时权限 |

**`propose` 是 Magento 没有的那一维，也是 AI 主体必须的那一维**：AI 可以"写"，但落到
`#48` 的 `pending`，人类 `publish` 才生效。它不是新机制 —— 就是 `documents` 已有的两段，
只是现在有了一个**权限词**去描述它。

求值：树继承 allow/deny → `selector` 过滤 → `effect` 合并（**deny 优先**）→ `window` 判过期/超预算。
**未知 `resource` 一律 deny**（fail-closed，和 `SchemaTooNew` 同一套脾气）。

### 求值器是**纯函数**（这条是内核硬约束逼出来的好事）

`crates/kernel` 的规矩是零进程、零网络、**零直接时钟**。授权求值正好可以满足：

```rust
pub fn decide(rules: &RuleSet, principal: &Principal, request: &Request, now_ms: i64) -> Decision
```

时钟当参数传进来 → 这条逻辑**能穷举测试**：规则表 × 主体 × 请求 × 时间点的组合是可枚举的，
不需要起服务、不需要等真时间过。判据里最值钱的是这几条：

- `deny` 永远压过 `allow`（同一 `resource` 上有两条冲突规则）
- 空规则集 **不等于** root：没规则 = 什么都不能做（`#50` 的 bootstrap token 是显式写了一条 `*` 规则）
- 过期一秒前后行为不同（`now_ms` 卡在 `expires_at` 上）
- 同一请求在 AI principal 与 human principal 下得到不同 `Decision`（`propose` vs `allow`）

### 还没定的硬问题（展开讨论时按这个顺序问）

1. **审计字段现在加还是等？** 每条 `entry` / `submission` / `document revision` 目前**没有 actor**。
   权限系统可以后做，**actor 字段不能后做**——数据模型变更的成本随时间指数上升，
   而且历史数据补不出"谁写的"。我的倾向：现在就在 `durable` 的记录里留 `actor`，缺失当 `system`。
2. `scope` 存哪儿：`durable`（跟着库走）还是宿主配置文件（可热改、可不进备份）？
   两者都合理，区别是"换库就换权限"还是"换库不换权限"。
3. **权限检查点放在哪几层**：HTTP 边界（认证 + 端点族）、工具执行前（工具白名单）、
   查询（数据范围）。**绝不许**进 `storage` —— 内核是零网络零进程的契约层，
   在那里写 `if principal.can(...)` 会把这个层变成业务层。
4. **自我提权**：AI 不能改自己的钥匙、不能改清单、不能批准自己的 proposal。
   实现上是"权限变更只接受 `kind == human`"这一条硬判，还是别的形式？
5. **模型白名单与 `/v1/capabilities` 的关系**：`capabilities` 是"服务端有什么"，钥匙过滤是"你能用什么"。
   两者混在一个响应里，还是分成 `capabilities` + `whoami` 两个？（我倾向分开，理由：
   capability 是编译期事实、可缓存；权限是每把钥匙不同、随请求走）
6. **预算**：token / 时间 / 调用次数，超了是 429 还是"这一轮直接终态拒绝"？终态拒绝要不要落 entry？
7. **会话归属**：`own` 范围下，AI 起的会话人看不看得见（人应该看得见，但不该默认混进人的列表）？
8. **系统主体**：触发器（还没设计）以哪个 principal 身份起轮次？它必须有一个，否则审计是断的。
9. **钥匙泄露**：轮换窗口、旧钥匙宽限期、`#50` 的 `--rotate-token` 要不要支持"同时两把有效"。
10. **UI**：NousDesk 是否要长出一个"钥匙管理"页面，还是只在配置里手写？（倾向后者，先不要页面）

### 与现有票的关系（**不阻塞，但要留位**）

| 票 | 现在要留的一寸 |
|---|---|
| `#50` | 单 token = root principal 的退化情形；token 记录里留 `principal` 字段，别写死"一个 token 就是一个世界" |
| `#47` | `capabilities` 保持"服务端有什么"，不要往里塞权限；另起一个响应给"我能干什么" |
| `#48` | `pending` / `publish` 就是 AI 提案通道，**别把它做成 UI 专用的小功能** |
| NousDesk `#8` | `capability.rs` 的 `gate()` 签名留成 `gate(feature, action)`，先用 `Action::All` 占位 —— 现在是零成本，以后返工就是 41 个调用点 |

### 最小验收闭环（将来开票时用这条钉住，别做半套）

用一把**受限钥匙**（`presets:write` 禁、模型只许一个、工具只许 `read`）：

- `POST /v1/presets/:section/create` → **403**，且库里**一条 revision 都没多**（负向哨兵：把权限检查注释掉 → 变 201，红）
- `POST /v1/sessions/:id/messages` 指定白名单外的模型 → **403/400**，且不写 `pi.user`
- `/v1/models` 里**看不到**白名单外的模型
- 同一把钥匙发起的会话，审计里 actor == 该钥匙的 principal id
