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

**一句话**：`principal` 有人 / AI / 系统三类；钥匙不是通行证，是**一束能力**
（端点族 × 模型白名单 × 工具白名单 × 数据范围 × 预算）。**"AI 拿受限钥匙改自己的宿主"是常规操作。**

**全文在 [`docs/permissions.md`](permissions.md)**（含 Magento 2 + Amasty Advanced Permissions 的参照、
`Rule{principal,resource,selector,effect,window}` 草案、纯函数求值器、10 条未定问题、最小验收闭环）。

这里只留一条最急的：**`actor` 字段现在就要，权限系统可以后做** ——
`entry` / `submission` / `document revision` 今天没有 actor，历史数据补不出"谁写的"。
