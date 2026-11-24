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

## 插件清单与文档：`plugin.toml` + 双语 md（API 表是生成物）

> **2026-11-24 已改向**：插件 README 定为「中文 ≤300 字人话」，正文随产物发布、
> 不进二进制（`docs/plugins.md` 规则 7；`decisions.md` 2026-11-24）。下面这节的双语 /
> 生成 API 表方案只作历史，不要再按它落代码。

**一句话**：每个插件（= 一个 crate）自己带一份 `.toml` 清单（名字 / 版本 / 依赖版本约束 / 声明的端点 /
权限需求），再带一份**双语**的 `md`；md 里 **API 那一章是生成的**，其余是手写的。

**这就是 "NOUS 是 Magento 2" 的落地形态**：Magento 的模块有 `composer.json`（版本 + require 约束）
+ `README.md` + `etc/acl.xml`（权限）+ `webapi.xml`（端点）。我们把这些收成两样：`plugin.toml` 和一份 md。

### 形状草案（未定稿）

```toml
# crates/nous/documents/plugin.toml
name   = "documents"
vendor = "nous"
version = "1.2.0"                  # 与 Cargo.toml 的 version 必须一致（构建期校验，见硬问题 3）
layer  = "app"
crate  = "nous-documents"

[requires]                         # 版本闸：编译期内置吃 cargo，运行时外挂吃这里
nous-wire  = "^1.4"
nous-durable = "^2.0"

[provides]
capability = "documents"           # 必须与 docs/plugins.md 的 module 列同名（capabilities 的 name）
endpoints  = ["GET /v1/documents/:kind", "PUT /v1/documents/:kind/file", "…"]   # 注册表生成，不手写

[permissions]                      # 这台机器上它需要什么（不是"谁能用"，反方向）
endpoints  = ["documents.*"]
tools      = []
```

### md 骨架（章节固定，人和 agent 都能 grep）

```
# documents
> 中文在这页上半 · English below     ← 顶部两个锚点，单文件内切换

## 1 它是什么 / What it is
## 2 什么时候用 / When to use（与相邻模块的边界：为什么不是 prompts）
## 3 清单 / Manifest        ← 内嵌 plugin.toml 全文（生成，保证不漂）
## 4 API                     ← 表格：方法 / 路径 / 请求体 / 响应 / 错误码 · **生成**
## 5 权限 / Permissions      ← 需要哪个 resource / selector 维度
## 6 失败模式 / Failure modes ← 每个错误码的现场、怎么查、谁的责任
## 7 判据 / Judgement        ← 指向 verify.sh / deploy-check.sh 里哪几条
## 8 反悔点 / Reversal
```

固定章节号的好处：agent 可以只读 `## 4 API` 那一章，不用读整篇；`grep -n '^## '` 就知道这份文档有没有缺章。

### 三条我坚持的（写进这里防止以后被"简化"）

1. **API 表必须是生成物，不许手写。** `scripts/gen-plugin-docs.sh` 从 `/v1/_routes` 的注册表 +
   `plugin.toml` 生成 `## 3` 和 `## 4` 两章。手写必然漂，而「文档说有这么个端点、实际没有」比没有文档更坏。
   这是 `#47`/`#48` 那条「endpoints 由注册表生成」的自然延伸：**连文档也只写一遍**。
2. **双语在同一个文件里，不做两个文件。** 两个文件必然只更新一份（真实痛过）。顶部两个锚点
   `[中文](#zh) · [English](#en)`，中文在上、英文在下。生成器只填表格，散文人写两遍 —— 这是唯一
   保证两边不漂的写法。将来若要"点一下就切"，那是生成静态站的事，源文件仍是这一份。
3. **版本号不许两处真相。** `plugin.toml` 的 `version` 与 `Cargo.toml` 的 `version` 由构建期校验
   比对，不一致直接失败。同理 `provides.capability` 与 `docs/plugins.md` 的 `module` 列同名，
   也是一条测试。

### 还没定的硬问题

1. **双语覆盖策略**：现在 `docs/` 几十份全是中文。一次性全译会漂（译完代码就动）。倾向：
   **新格式（plugin md + 新文档）双语起步**，旧文档按需迁移，不搞大迁移。要拍。
2. **语言切换的形式**：(a) 单文件双段 + 锚点（GitHub 原生可读，零工具链）；
   (b) 两个文件互链（`README.md` / `README.zh.md`，GitHub 生态习惯，但会漂）；
   (c) 单文件 + `<details>` 折叠（省屏幕，但 agent 抓取要处理折叠）。
   倾向 (a)，理由：agent 抓取最省事、diff 不分裂。
3. `plugin.toml` 与 `Cargo.toml` 的 `[dependencies]` 是重复。是**生成 `plugin.toml` 的 requires 段**
   （从 `cargo metadata` 抽），还是手写并校验？倾向：`requires` 段生成 + 手写段只有 name/layer/permissions。
4. `permissions` 段的方向：它是"这个插件需要什么权限"（用于发行版装配时算权限上界），
   还是"这个插件暴露什么 resource"（用于 ACL 树生成）？两者都需要，但混在一起会长成一个上帝字段。
5. 文档里的 **错误码表** 是生成的还是手写？错误码在代码里是枚举 —— 能生成，但"现场与怎么查"是散文。

## 多主体 · 钥匙即权限（**AI 也是一个主体**）

**一句话**：`principal` 有人 / AI / 系统三类；钥匙不是通行证，是**一束能力**
（端点族 × 模型白名单 × 工具白名单 × 数据范围 × 预算）。**"AI 拿受限钥匙改自己的宿主"是常规操作。**

**全文在 [`docs/permissions.md`](permissions.md)**（含 Magento 2 + Amasty Advanced Permissions 的参照、
`Rule{principal,resource,selector,effect,window}` 草案、纯函数求值器、10 条未定问题、最小验收闭环）。

这里只留一条最急的：**`actor` 字段现在就要，权限系统可以后做** ——
`entry` / `submission` / `document revision` 今天没有 actor，历史数据补不出"谁写的"。
