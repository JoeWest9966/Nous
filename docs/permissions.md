# 权限 · 多主体与钥匙（设计讨论稿）

> **状态**：方向已定，**未开票、未设计完**。这份文件是讨论的落脚处，不是规格。
> 决定一旦拍板 → 搬进 `docs/decisions.md`（带反悔点）或变成票；那时的代码以票面为准。
>
> **来源**：2026-11-18，用户在「部署拷问」的过程中想到的方向（原话见下）。**同一轮拷问里冒出来的**，
> 说明它不是拍脑袋加的功能 —— 是"装上工控机、谁在跑、谁能碰"这个问题问到底时必然撞上的东西。

---

## 0 · 用户原话（不许改写成更顺耳的话）

> 我们这个项目要做多用户，这个是一个非常重要的点，或者也可以理解成**多种 api 密钥权限**。
> 什么意思呢，就是不是说就我用 —— **AI 也是用户**，比如 AI 用这个有 NOUS 的 api 密钥，
> 但是这个密钥有特定的权限，比如不能创建模板，比如只能用哪些模型。
>
> 原因是将来我可能要用 **AI 构建 NOUS**，比如创建 skill、创建模板等等。这是 NOUS 之前没有的，
> 但确实是我打算开发的一个 **NOUS 特有的功能**。

> 如果把 NOUS 理解成 Magento 2，那么我说的这个架构功能就类似这个插件
> <https://amasty.com/user-guides/magento-2-advanced-permissions/>。

## 1 · 一句话

`principal` 有三种：**人 · AI · 系统**。钥匙（API key）不是通行证，是**一束能力**：
能碰哪些端点族、能用哪些模型、能调哪些工具、能看哪些会话、烧多少预算。
**"AI 拿受限钥匙改自己的宿主"是常规操作，不是边缘情况。**

**为什么这是 NOUS 特有的**：Magento 的 ACL 主体只有人；pi 是单用户单钥匙；
SaaS 那套是"用户 = 计费单位"。这里反过来：**主体是执行者，钥匙是它的权限切片。**

---

## 2 · 参照物：Magento 2 核心 ACL + Amasty Advanced Permissions

**链接**（用户给定，2026-11-18 备份）：
<https://amasty.com/user-guides/magento-2-advanced-permissions/>

**抓取备注**：直连被 Cloudflare 拦（返回挑战页）；经 `r.jina.ai` 取到正文，要点如下手抄。
原文是英文，摘录保留原词，不改写成中文术语，免得以后对不上。

### 2.1 它实际做了什么（据原文）

它是一个**角色（Role）上的六个"维度页签"**，挂在核心 ACL 的资源树里：

| 页签 | 原文要点 | 等价于 |
|---|---|---|
| **Advanced: Scope** | `Limit Access To` 限定 **store view / website**；限定后该站点/store view 的 dashboard 与 CMS 页自动受限 | **作用域**（不是树，是集合） |
| **Advanced: Categories** | 全部 / 仅选定分类与子分类 | 实体级允许清单 |
| **Advanced: Products** | 全部 / 仅选定 / **own created** / **created by users with the same role** | **记录级所有权**（`own` / `own-role`） |
| **Advanced: Product Attributes** | `Allow Access To = Selected Attributes`，限定可改的字段；另有"限制 Sources 数据" | **字段级**（同一条记录里哪些键可写） |
| **Advanced: Product Page Tabs** | 控制编辑页**页签**的可见性（列出 tab 标识） | **UI 面片级**（比字段更粗一层） |
| **Advanced: Admin User Role** | 允许该角色**创建哪些角色**的新用户 | **委派签发**（谁有权造钥匙） |

另外：可**阻断特定实体的 import / export**；新增权限挂在资源树的
`System → Amasty Role Permissions` 分支下（**它是加分支，不是换掉核心 ACL**）。

### 2.2 从它读出来的三条（这才是要抄的东西）

1. **核心 ACL 只有一棵资源树**（`System → Permissions → User Roles → Role Resources`，
   `Magento_Backend::admin` 往下继承，allow/deny 逐层合并）。树能表达"能不能进这个菜单/调这个动作"，
   **表达不了作用对象**。Amasty 整个产品就是为此存在 —— 这是最强的需求证据。
2. **它把"树外的维度"做成了数据页签，而不是代码分支**。六个页签 = 六个字段，
   加一维不改求值逻辑。**这是唯一可抄的实现方式**。
3. **它的实现方式是"一堆 plugin 挂在各处拦"**（Magento 的扩展机制决定的）。
   **这一半不抄**：那会让我们每一维都在不同层被拦一次，求值逻辑碎成十几处。

### 2.3 抄一半

| 抄 | 不抄 |
|---|---|
| **数据模型**：规则表 + 一个求值器 | 每加一维就加一个特判 / 一个拦截点 |
| **正交维度是数据** | 它的具体产品语义（store view、商品属性在 NOUS 里没有对应物） |

---

## 3 · NOUS 的模型（草案，未拍板）

**一张表、一个求值器**：

```
Rule { principal, resource, selector, effect, window }
```

| 字段 | NOUS 里是什么 | Magento/Amasty 对应 |
|---|---|---|
| `principal` | `{ kind: human | agent | system, id, label }`。`agent` 的 id 指向会话/扩展，审计里能回答"这条 entry 是哪个 agent 写的" | 核心只有 admin user（**我们多两类**） |
| `resource` | 端点族 + action 的**树**：`documents.presets.write`、`documents.skills.publish`、`sessions.message`、`tools.bash`、`models.<id>`、`keys.issue` | 核心 ACL 资源树 |
| `selector` | 作用对象的**谓词**：document `kind` / `section` / 字段允许清单 / 会话归属（`own` / `own-role` / `shared`）/ `workdir` 前缀 | Amasty 的 Scope / Categories / Products / Attributes 四个页签 |
| `effect` | `allow` / `deny` / **`propose`** | 核心只有 allow/deny |
| `window` | 有效期（`expires_at`）+ 预算（token / 调用次数） | Amasty 的"限时权限" |

**求值顺序**：树继承 allow/deny → `selector` 过滤 → `effect` 合并（**`deny` 优先**）→ `window` 判过期/超预算。
**未知 `resource` 一律 `deny`**（fail-closed，和 `SchemaTooNew` 同一套脾气）。

### 3.1 `propose` 是 Magento 没有、而 AI 主体必须的一维

AI 可以"写"，但落到 `#48` 的 `pending`，**人类 `publish` 才生效**。它不是新机制 ——
就是 `documents` 已有的两段（pending / publish），只是现在有了一个**权限词**去描述它。

```json
{ "principal": "agent:opencode-1",
  "resource": "documents.skills.write",
  "selector": { "kind": ["skill"] },
  "effect": "propose",
  "window": { "expires_at": 1765000000 } }
```

人类那把钥匙是 `effect: "allow"`（可以直接 publish）。**同一条路径，两种 effect。**

### 3.2 委派签发 → 必须补一条反向硬判

Amasty 的 `Admin User Role` 页签说明"谁能造钥匙"是真需求（不然只能一个 root 造）。
但产品里没有对应的是：**不能给自己提权**。NOUS 必须有这条，否则 AI 拿到的第一件事就是
给自己开权限：

- 权限变更（规则增删、钥匙签发）只接受 `kind == human` 的 principal；
- 一把钥匙**不能**修改规则里涉及它自己的条目（连"换一把更宽的钥匙"也不行）；
- 审计里权限变更单独一类事件，和业务写分开。

---

## 4 · 求值器是**纯函数**（内核硬约束逼出来的好事）

`crates/kernel` 的规矩：零进程、零网络、**零直接时钟**、可嵌入无 runtime 宿主。授权求值正好满足：

```rust
pub fn decide(rules: &RuleSet, principal: &Principal, request: &Request, now_ms: i64) -> Decision
```

时钟当参数传 → **能穷举测试**：规则集 × 主体 × 请求 × 时间点的组合可枚举，
不用起服务、不用等真时间过。最值钱的四条判据：

| # | 判据 | 负向哨兵 |
|---|---|---|
| 1 | 同一 `resource` 上 `deny` 永远压过 `allow` | 改成"后写覆盖" → 红 |
| 2 | **空规则集 ≠ root**：没规则 = 什么都不能做 | 默认 allow → 红 |
| 3 | `now_ms` 卡在 `expires_at` 前后一秒，判定必须不同 | 比 `<=` 写成 `<` 仍然过 → 红 |
| 4 | 同一请求在 `agent` 与 `human` 主体下得到不同 `Decision`（`propose` vs `allow`） | — |

**`#50` 的 bootstrap token 在这里是一条显式规则**（`principal=root, resource=*, effect=allow`），
不是"没有规则就全通"。**空规则集是全关，这是这套东西的地基。**

---

## 5 · 检查点放在哪（三层，绝不许进第四层）

| 层 | 管什么 | 不许做什么 |
|---|---|---|
| **HTTP 边界** | 认证（哪把钥匙）+ 端点族（`resource`）+ 数据范围（`selector` 里的会话归属） | 不许在 handler 里逐条 `if` —— 统一走 `decide()` |
| **工具执行前** | 工具白名单（`tools.bash` 禁 / `tools.read` 许）、`workdir` 前缀 | 不许在工具实现里再判断一次（一处判定） |
| **查询** | `own` / `shared` 的行级过滤 | 不许在 SQL 里散落 `WHERE`，要由 `selector` 生成 |
| ~~`durable` / `storage`~~ | —— | **绝不进**：内核是零网络零进程的契约层，在那里写 `if principal.can(...)` 就把层变成业务层 |

---

## 6 · 钥匙本身

- 生成时**显示一次明文**，库里只存**哈希 + scope**；前缀 `nous_sk_…` 便于肉眼识别与 grep 泄漏。
- `#50` 的单 token = 「一个 root principal 的钥匙」= 这套的**退化情形**，不冲突。
- 轮换：`#50` 已有 `--rotate-token`；多钥匙后要支持**宽限期**（旧钥匙在窗口内仍有效，便于无停机换）。

---

## 7 · 还没定的硬问题（讨论时按这个顺序问）

1. **`actor` 字段现在加还是等？** 每条 `entry` / `submission` / `document revision` 目前**没有 actor**。
   权限系统可以后做，**actor 字段不能后做** —— 数据模型变更成本随时间指数上升，历史数据补不出"谁写的"。
   **倾向：现在就在 `durable` 的记录里留 `actor`，缺失当 `system`。**
2. `scope` 存哪儿：`durable`（跟着库走、可备份）还是宿主配置文件（可热改、不进备份）？
   区别是"换库就换权限"还是"换库不换权限"。
3. 模型白名单与 `/v1/capabilities` 的关系：`capabilities` 是"服务端有什么"（编译期、可缓存），
   权限是每把钥匙不同（随请求走）。**倾向分开**：另起一个"我是谁、我能干什么"的响应。
4. **预算**：token / 时间 / 调用次数，超了是 429 还是"这一轮直接终态拒绝"？终态拒绝要不要落 entry？
5. **会话归属**：`own` 范围下，AI 起的会话人看不看得见（人应该看得见，但不该默认混进人的列表）？
6. **系统主体**：触发器（还没设计）以哪个 principal 起轮次？它必须有一个，否则审计是断的。
7. 委派签发的深度：允许"造钥匙的人"再委派下去吗？（Amasty 的 `Admin User Role` 是一层还是可递归？）
8. `propose` 的粒度：pending 里能不能**只改某几个字段**被批准（部分批准），还是整体接受/拒绝？
9. 冲突诊断：人问"我为什么看不到这个按钮"时，怎么把 `decide()` 的推导链打出来？（可观测性）
10. UI：NousDesk 要不要长出一个"钥匙管理"页面，还是只在配置里手写？（**倾向后者，先不要页面**）

---

## 8 · 与现有票的关系（**不阻塞，但要留位**）

| 票 | 现在要留的一寸 |
|---|---|
| `#50` | 单 token = root principal 的退化；token 记录里留 `principal` 字段 |
| `#47` | `capabilities` 保持"服务端有什么"，**不要往里塞权限** |
| `#48` | `pending` / `publish` 就是 AI 的提案通道，**别做成 UI 专用小功能** |
| NousDesk `#8` | `capability.rs` 的 `gate()` 签名留成 `gate(feature, action)`，先用 `Action::All` 占位（现在是零成本，以后返工就是 41 个调用点） |

## 9 · 最小验收闭环（将来开票时用这条钉住，别做半套）

用一把**受限钥匙**（`presets:write` 禁、模型只许一个、工具只许 `read`）：

- `POST /v1/presets/:section/create` → **403**，且库里**一条 revision 都没多**
  （负向哨兵：把权限检查注释掉 → 变 201，红）
- `POST /v1/sessions/:id/messages` 指定白名单外的模型 → **403/400**，且不写 `pi.user`
- `/v1/models` 里**看不到**白名单外的模型
- 同一把钥匙发起的会话，审计里 `actor` == 该钥匙的 `principal` id
- AI 主体 `effect: propose` 写 skill → 进 `pending`，**`publish` 被 403**

## 10 · 风险与反悔点

- **风险：权限做成第二套真相。** 一旦有人在 handler 里手写 `if role == "admin"`，这套东西就废了。
  反悔点：`decide()` 之外出现第二处权限判断 → 立刻收敛回 `decide()`，不许"临时绕过"。
- **风险：actor 加晚了。** 反悔点：如果这件事拖到有了真用户数据才做，就只能接受"历史条目 actor 未知"，
  那要在 About/审计里明确写出来，不能假装有。
- **风险：AI 自我提权。** 反悔点：任何"AI 生成的规则/钥匙"进库前必须由 human principal 签，
  发现一次直接回滚该类规则并审计。
