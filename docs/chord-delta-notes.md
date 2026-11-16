# Chord Delta 语义边界调研（issue #4）

- 调研对象：`pi@v1.5.2` 的 `packages/chord/src/delta/**`（tracker 2319 / index 742 / diff 525 / pending 173 / apply-immutable-trusted 125 / revision-validator 75 / draft 10 / README 262 行）。上游检出 `/tmp/pi-check` 挂在 tag `v1.5.2`，**只读**，未改一行。
- 本票只出文档，不提货代码。台账 `SOURCES.md:47` 仍是「待提货」；`crates/nous-chord/src/delta/**` 尚不存在。
- 立场：**本库是事件驱动的会话内核**（客户端操作经 delta 应用到文档），判据是序列化后的语义与信任边界，不是 TS 的对象形状。下表所有位置均为 `packages/chord/src/delta/<file>.ts:<line>`；README 位置写作 `README.md:<line>`。
- 宪法优先：op 落到 SSE / 存储时的字段名与事件形状以本库 `wire.md` 为准，上游 README 的 TS 词汇只作语义输入。

## 一、必须保住的不变式清单

### 1.1 Op 词表与不变量（地基，index.ts）

| 不变式（一句话） | 上游位置 | 破了会怎样 | Rust 里怎么保 |
|---|---|---|---|
| Op 共七种：`r` 整根替换、`s` 设值、`d` 删除、`a` 字符串追加、`t` 截头 `count` 个 UTF-16 码元、`p` 数组 splice、`m` 置换（`new[i]=old[permutation[i]]`）；`r` 是唯一能替换整根的 op，`s/d/a/t` 不得指向根，`p/m` 可以。 | index.ts:24–49 | 多一种/少一种 op 就与上游批次不互通；根被 `s` 写成半个值会让副本无法重建。 | `enum Op { R(JsonValue), S(NonEmptyPath, JsonValue), D(NonEmptyPath), A(NonEmptyPath, String), T(NonEmptyPath, usize), P(Path, usize, usize, Vec<JsonValue>), M(Path, Vec<usize>) }`，用 `NonEmptyPath` 新类型在类型层禁止 `S/D/A/T` 空路径。 |
| 批次「以 `r` 开头」是可判定的事实（flush 保证 `r` 只在 index 0 或不存在），不是启发式；读者据此把 `r` 当恢复点。 | index.ts:74–76, index.ts:587–598 | 把中途的 `r` 当普通 op，重放得到错误状态；恢复点失效。 | `is_base(&[Op]) -> bool` 只查首元素；encoder/decoder 遇 `r` 清空字典（见 codec 条）。 |
| **批次精确但不规范**（exact but not canonical）：同一变化可以发出不同 op 组合，大改可以折成区域 splice、祖先 `s` 或 `r`；「op 形状」没有契约，只有结果值有契约。 | README.md:198–200（"Batches are exact but not canonical…"），index.ts:40–49 | 拿 op 序列做一致性断言会写出永远在抖的测试；按 op 形状推导语义会随上游实现变化而错。 | 一致性测试只断言「对同一 base 重放后 JSON 相等」；不固定发出的 tuple。 |
| 路径段只能是字符串键或非负整数下标；危险段 `__proto__` / `constructor` / `prototype` 必须拒绝（`UnsafePathError`），因为 op 是数据、可能来自模型输出。写值必须用定义数据属性而非赋值，避免原型 setter。 | index.ts:143–157, index.ts:293–300, index.ts:379–386, README.md:214–218 | `["s",["__proto__","isAdmin"],true]` 污染整个进程原型；继承的 setter 在写入时执行。 | Rust 的 `serde_json::Map`/`Vec` 没有原型链，**结构上天然免疫**；但仍要在解码边界显式拒绝这三个段名，让错误可见且跨实现一致。 |
| 数组下标写入只能命中现有元素或恰好 `length`（`index > len` 拒绝）；`arr[7]=x` 在长度 3 的数组上会造出稀疏数组，序列化成 `null` 后副本永久分叉；同时它让单个 op 分配 40 亿元素的 DoS 成为可能。 | index.ts:303–320, index.ts:383–387 | 副本分叉 / 磁盘与内存爆炸。 | `set_index` 用 `if idx > len { UnsafePath }`；`Vec` 无洞天然保证其余部分。 |
| 验证器分词汇：`assertValidOp`（路径内联）与 `assertValidWireOp`（id/短写合法）各管各的；未知动词一律报错，绝不静默跳过。 | index.ts:166–223, index.ts:225–292（两条 default 分支） | 新版生产者的 op 被旧版消费者无声丢弃，状态静默分叉。 | `match verb { … verb => Err(...) }`；decode 后再跑一次 decoded-op 校验。 |
| `m` 的置换必须是双射（长度一致、下标在界内、每个下标恰好一次）。 | index.ts:212–223（`assertPermutation`） | 非双射会让元素重复/丢失，副本与发布值不同。 | `validate_permutation`：长度匹配 + `HashSet` 查重，O(n)。 |

### 1.2 PendingOps 排队与折叠（pending.ts）

| 不变式（一句话） | 上游位置 | 破了会怎样 | Rust 里怎么保 |
|---|---|---|---|
| 四个状态 `empty / batch / folding / replace`；单个批次按引用入队（跟得上的消费者零折叠成本），第二批起进入 folding；`replace` 状态吞掉后续一切，直到 `take()` 或以 `r` 开头的批次。 | pending.ts:25–33, pending.ts:55–84, pending.ts:108–134 | 慢消费者要么付不必要的折叠代价，要么在溢出后继续攒 op 导致无限积压。 | `enum PendingState { Empty, Batch(Arc<Vec<Op>>), Folding{ops,leaves}, Replace }`；状态转换照搬。 |
| 以 `r` 开头的批次**丢弃全部积压（包括 replace）**，并原样保留该批次。 | pending.ts:59–63, pending.ts:111–114 | 积压里旧 op 覆盖新的整根替换，副本回退到旧值。 | `push` 首行检查 `ops.first()==R`，直接替换状态。 |
| 折叠规则：同路径上 `a+a` 合并、`s` 覆盖 `s` 或 `a`、`a` 跟在字符串 `s` 后接进它；任何其他 op（含对象/数组 `s`、`p/m/d/t`）清空叶子索引；`r` 清空队列。 | pending.ts:108–133, pending.ts:167–173, README.md:101–108 | 折叠前后重放到同一 base 得到不同值 → 慢消费者看到的文档和权威不一致。 | 以 `HashMap<PathKey, usize>` 记「该路径最后一个叶子 op 的下标」；遇非叶子 op 即 `clear()`。 |
| 叶子索引的 key 必须单射：`["a.b"]` 与 `["a","b"]` 是不同路径，不能因字符串拼接撞在一起；数字 `1` 与字符串 `"1"` 可以共键（同一索引生命周期内父节点类型不变，结构 op 会清索引）。 | pending.ts:156–163 | 合并两个不同路径的写入，静默丢一条更新。 | `#[derive(Hash,Eq)] enum Seg { Key(String), Index(u64) }`，`Path = Vec<Seg>` 直接做 key；**不要**用 `format!("{:?}", path)` 以外的方式拼字符串键。 |
| 折叠计数按折叠后的 op 数算；`10_000` 溢出后只留「用当前值替换」；单批超长在**第二批到达时**才溢出（单批按原样计一次）。`take()` 原子交付并清空。 | pending.ts:12, pending.ts:81–87, pending.ts:93–106, pending.ts:126–131 | 积压无界增长；或提前丢弃还能表达的批次。 | 常量 `MAX_PENDING_OPS=10_000`；`fold` 返回 bool 表示是否溢出；`take` 用 `mem::replace(&mut state, Empty)`。 |
| 入队 op 与 op tuple **永不被修改**，合并只造新 tuple；同一批次喂养多个消费者必须安全。 | pending.ts:18–22, pending.ts:44–49, README.md:109–110 | 一个消费者折叠时改坏另一个消费者看到的批次。 | `Arc<[Op]>` 共享不可变批次，合并时构造新 `Op`；`take` 返回 clone 过的 `Arc`。 |

### 1.3 revision-validator 的版本校验（revision-validator.ts）

> 语义澄清：这里的「版本校验」不是数字版本号比对，而是**「已验证容器跨不可变修订复用」的验证器**：按对象身份记住已验证的容器，只对第一次见到的容器做严格校验。本库没有引用身份语义，用内容哈希/结构等价替代即可，别把 WeakSet 照搬。

| 不变式（一句话） | 上游位置 | 破了会怎样 | Rust 里怎么保 |
|---|---|---|---|
| 只接受严格 JSON：null/bool/string/finite number 为叶子；数组必须稠密且只有下标数据属性；对象必须是 plain 或 null-prototype，且属性必须可枚举的数据属性；拒绝 symbol 属性、accessor、环。 | revision-validator.ts:44–69 | 非法值进入不可变修订，序列化结果在不同实现间分叉。 | 用 `serde_json::Value` 作为唯一表示（它已经排除了 NaN/Inf/undefined/稀疏/环/原型），入口反序列化即校验；不要再引入自定义 `Any`。 |
| 环检测按**当前递归祖先**（`ancestors`），校验通过的节点进 `finished`，整个跨调用缓存进 `#validated`（WeakSet）；同一容器第二次出现直接放行，不重复遍历。 | revision-validator.ts:9–39 | 深度嵌套共享结构被重复验证 O(n²)；或把 DAG 共享误判成环。 | Rust 值树没有共享/环，每次调用就是一次线性遍历；若将来引入 `Arc` 结构共享，用 `HashSet<*const …>` 或指针地址做「已验证」缓存，并单独维护祖先集合。 |
| 校验返回**原值**而不克隆（`validate<T>(value:T):T`），校验与所有权转移是两件事。 | revision-validator.ts:10–13 | 误以为验证会净化数据而在后面继续可变使用。 | `fn validate(&mut self, value: &Value) -> Result<(), ValidateError>` 之类，明确不产生新值。 |

### 1.4 apply-immutable-trusted 的信任边界（apply-immutable-trusted.ts）

| 不变式（一句话） | 上游位置 | 破了会怎样 | Rust 里怎么保 |
|---|---|---|---|
| 该函数只接受**自己产生的**（tracker/diff/appendText）op；它不做完整 op 校验、不校验 payload 严格性，路径/类型错就直接抛。外部来源的批次必须先走 `decoder()`/`assertValidOp()`。 | apply-immutable-trusted.ts:14, README.md:184–188, README.md:218–219 | 把不可信 op 喂进来会静默写出非法 JSON；或把「可信路径」误当「已验证路径」。 | Rust 里用两个类型区分：`TrustedOps` 只能由本 crate 的 tracker/diff 构造；`WireOps` 经 `decode` 后才可转 `TrustedOps`。类型层阻止外部直接调 trusted apply。 |
| 每个触及的容器在整批内**只浅拷贝一次**（`WeakSet<object>` = `owned`），copy-on-write 共享未触及的子树；批内后续 op 可以就地改已拷贝的节点。 | apply-immutable-trusted.ts:15–17, apply-immutable-trusted.ts:63–84 | 少拷 → 改动泄漏到 base revision；多拷 → O(batch × depth) 内存和性能塌陷。 | 用 `HashSet<Path>` 记「本批已物化的容器路径」，沿路径自顶向下：第一次到某节点就 clone 并挂回父，之后直接取可变引用。Rust 借用检查器需要「先收集路径、再逐条 apply」或 `Rc<RefCell<...>>` 局部内部可变性；这与 JS `WeakSet` 是同一角色的不同形状。 |
| 输入 target 与 op payload **不被修改**；结果可以与 base/payload 共享容器，因此对结果的任何再写入都是契约违反。 | apply-immutable-trusted.ts:98–103, README.md:26–44 | 发布值被下游改动 → 权威与副本分叉（README 开头：「illegal mutation is not detected, silently corrupts state」）。 | Rust  ownership 天然保证：函数签名 `fn apply_immutable_trusted(base: &Value, ops: &[Op]) -> Value` 全返回新值，调用方拿不到对 base 的 `&mut`。 |
| `p` 用 splice 原地插入/删除（分段避免一次性展开大数组），`m` 先 snapshot 再置换；`d` 对数组按数字下标 splice、对对象删键；`a/t` 要求目标是字符串，`t` 是**从头**截 UTF-16 码元。 | apply-immutable-trusted.ts:104–121, index.ts:36–48 | 截断方向/单位搞反（从头 vs 从尾、UTF-16 vs Unicode scalar）会让字符串副本悄悄不同。 | `t` 在 Rust 里要显式决定单元：为与 wire 互通，按 UTF-16 码元计数实现（可用 `encode_utf16` 计数再切），不要把 `count` 当 `chars().count()` 或字节数。 |
| 不可变重放跨多个批次共享一次 copy-on-write 作用域，不暴露中间修订（index.ts:427–432）。 | index.ts:427–432, index.ts:433–455 | 中途把半拷贝的 root 暴露给消费者，消费者保留后看到被后续批次改写的值。 | `apply_immutable_batches` 内部维护一个 `owned` 集合贯穿所有批次，只在最后返回 root；不要包装成迭代器 yield 中间值。 |

### 1.5 tracker 中 load-bearing 的结构（tracker.ts）

「load-bearing」= 破坏后**会改变发出的 op / 可见状态 / 生命周期语义**；其余是 JS 世界的复用/GC/性能技巧。

| 不变式（一句话） | 上游位置 | 破了会怎样 | Rust 里怎么保 |
|---|---|---|---|
| 所有权一次性转移：`track(initial)` 拿走 root 且 O(1) 不遍历；root 必须无别名、无环、严格 JSON（别名会让 `prepared.value` 与按 ops 重放的值分叉）。 | tracker.ts:357–359, README.md:119–126 | 权威值与副本值不一致且没有报错。 | `fn track(initial: Value) -> Tracker` 取得所有权；无别名由类型保证（Rust 值树不可内部共享）。 |
| 状态机：`open → prepared/consumed/aborted/stale`；`prepare()` 不改权威、`adopt()` 只做一次指针交换并 `revision += 1`；竞争提交用 `baseRevision` + `base` 身份双重判定，过期的一律拒绝。 | tracker.ts:26, tracker.ts:199–224, tracker.ts:284–307 | 并发编辑覆盖彼此；存储失败污染权威；旧 prepared 被重复 adopt。 | 显式 `enum Status` + `base_revision: u64` + `base: Value` 的内容等价比对；`adopt` 里先校验再替换、绝不部分更新。 |
| adopt 后所有竞争草稿立即变 `stale`，且必须在 O(1) 内释放其 overlay 强引用，不遍历节点。 | tracker.ts:329–346, tracker.ts:348–356 | 大草稿竞争时 adopt 变 O(草稿大小)；或旧草稿还能写入。 | 竞争上下文放进 `Vec<Weak<Context>>`/句柄表，adopt 时遍历的是上下文数量（常数级）并把状态置 stale；节点释放用 `Rc` 引用计数自动完成。 |
| 一个 change 必须以 `prepare()` 或 `abort()` 结算；draft handle 在 change 结束后**每次使用都抛错**；被删除元素上的 handle 写入被忽略。 | tracker.ts:193–235, README.md:28–37 | 拿过期 handle 写入脏数据，或静默丢失写入。 | Rust 里用带生命周期的 `Draft<'a>`（`&'a mut` 或 `PhantomData<&'a ()>`），编译期禁止 handle 逃逸；运行期用 `status` 兜底。 |
| 放置（placement）一律深拷贝并校验；`undefined` 对象属性=删除、数组里=抛；`push(valid, invalid)` 必须**一个都不插入**（先全量校验再改 draft）。 | tracker.ts:635–681, tracker.ts:1149–1164, README.md:127–148 | 部分插入留下半合法状态；严格 JSON 被污染。 | `clone_placement` 先 `validate` 全部输入，成功后一次性移入；Rust 无 `undefined`，对象属性删除显式 `remove`。 |
| `appendText(draft,key,chunk)` 直接记录 `["a",path,chunk]`：同 key 多次追加在 prepare 时合成一个 op，普通赋值/删除同 key 会退回前缀比较；它把流式追加从 O(N×length) 降到 O(N×chunk)。 | tracker.ts:363–388, tracker.ts:1913–1926, README.md:215–249 | 流式回答/工具输出逐 chunk 追加时二次复杂度，800KB/2 万次从 ~1ms 变成 ~2.4s。 | 在 draft 上存 `appends: HashMap<Key, String>`，`emit` 时优先发 `A`；`emitChangedValue` 的前缀检测只在无 append 记录时走。这是**Nous 事件驱动 + 流式 SSE 的核心路径**，优先级最高。 |
| 对象键顺序是可见语义：删除再插回同一个键必须显式发出 `d` 再 `s`，否则不可变副本的 key 顺序与 draft 不同；深度相等赋值保留旧版本的 key 顺序与原型。 | tracker.ts:1930–1942, README.md:173–177 | 序列化后的 JSON 文本不同，客户端 diff/快照比对抖动。 | 若 Rust 用 `serde_json::Map`（默认 BTreeMap 排序）则天然稳定；若用 `preserve_order`（IndexMap）必须复刻 delete-readd 规则。二选一要写进设计决策。 |
| 保留段折叠：改 `__proto__` 等保留键时，不发出该段，折叠成最近安全祖先的 `s`，根上是 `r`。 | tracker.ts:1712–1717, README.md:201–205 | op 路径里带危险段，被其他实现拒绝或触发原型污染。 | 与 1.1 的安全路径检查同一处实现；`emitOperations` 前统一折叠祖先。 |
| 空批次仍推进 `revision` 并让竞争者过期；op 上限 `4096` 超限退回 `r`；`prepareReplace` 深度相等时发空 ops 并保留旧根，否则发 `r`。 | tracker.ts:130, tracker.ts:1740, tracker.ts:2226–2234, README.md:169–188 | 重复 adopt 同一值却没让竞争者失效；巨大 diff 生成海量 op。 | 常量照搬；`emit` 超限立刻返回 `vec![R(root)]`；revision 在 adopt 中无条件递增。 |

**只是 JS 对象复用 / GC / 缓存优化（不是语义，Rust 不必照搬）**：

| 上游结构 | 真实作用 | 上游位置 | Rust 里怎么办 |
|---|---|---|---|
| `WeakRef` / `#competitors` / `#pruneBudget` / `registryRef` | 让被丢弃的草稿能被 GC，同时维持「最新一个强引用」的快速路径 | tracker.ts:236–283 | `Rc<Weak<…>>` / arena 索引 + 定期清 dead weak；或干脆强引用持有（草稿数量本来有界），语义不变。 |
| `rawNodes: WeakMap` / `NODE` symbol / `target` + `proxy` 双对象 | raw→node 反查、代理的目标对象 | tracker.ts:62–115, tracker.ts:497–534 | 显式 `NodeId`（arena index）+ `HashMap<NodeId, Node>`；不需要 symbol 或代理目标。 |
| `stored: Container[]` + `StoredRef {index}` + `RELEASED` 哨兵 | 把「最后一次写入值」间接存储，O(1) 覆盖，释放时打洞 | tracker.ts:118, tracker.ts:543–571 | `Option<Value>` 直接存在节点里；`RELEASED` 只是 JS 里避免悬垂引用的技巧。 |
| Piece tree（treap split/merge、`priority`、`pieces` 扁平缓存、`seed`） | 大数组插入/删除的 O(log n) 序列结构 | tracker.ts:35–58, tracker.ts:874–1014 | 保留**行为**（handle 跟随元素移动、能生成 `p/m`），内部换成 `Vec<Element>` + 稳定 id 或 rope/crdt；本库会话文档规模下 `Vec` 往往够用。 |
| `baseOverrides` / `insertOverrides` / `baseLocations` / `insertLocations` / `plan` / `denseRegions` | 把「位置→原元素」映射缓存起来，避免重算路径和重排计划 | tracker.ts:43–58, tracker.ts:1965–2050 | 按需重算的纯函数，或用 `HashMap` 缓存；缓存失效规则要保（结构变动清缓存），但缓存本身不是语义。 |
| `MAX_SIMPLE_OBJECT_NODES=128` / `simpleObjectMaterialization` | 常见小对象走快速 materialize 路径 | tracker.ts:131, tracker.ts:1750–1784 | 可保留为优化，不影响正确性。 |
| `overflowedBatches: WeakSet<Op[]>`（diff.ts） | 防止已溢出的批次继续 append | diff.ts:5–14 | 用 `Result`/显式状态返回，不需要 WeakSet。 |
| `owned: WeakSet`（apply-immutable-trusted / index） | copy-on-write 的「本批已拷贝」集合 | apply-immutable-trusted.ts:17, index.ts:433–434 | `HashSet<Path>`（见 1.4）。 |

### 1.6 diff / codec 里必须保住的语义

| 不变式（一句话） | 上游位置 | 破了会怎样 | Rust 里怎么保 |
|---|---|---|---|
| `MAX_DELTA_OPERATIONS=4096` 溢出 → 整批退回 `[["r", after]]`；否则按估算的 op 体积与整根快照体积比大小，选更省的一种。 | diff.ts:5–14, diff.ts:490–524 | 巨大编辑时发出比 snapshot 还大的 delta。 | 照搬两个阈值与 `jsonCost/opCost` 估算；不必照搬常量名以外的细节。 |
| 字符串 diff：前缀相同发 `a`、公共前后缀重叠发 `t`+`a`（带上限 65,536/候选 8 的启发式）、否则 `s`；`ownedSlice` 让切片成为独立字符串。 | diff.ts:63–79, index.ts:85–121 | 从长字符串切出的 op 在 V8 里持有整条 rope（内存泄漏）；或字符串副本不同。 | Rust `String`/`&str` 的 `to_owned()` 天然是独立分配；`t` 必须与 1.4 一样按 UTF-16 计数。 |
| 数组 diff 按 identity（引用相等）→ semantic alignment → LCS 的优先级选锚点，最后才发 `p`；置换只在长度>1、首尾都变且能构成双射时发 `m`。 | diff.ts:88–405, diff.ts:405–420 | diff 发出的 op 不稳定/不够紧凑，或产生与 draft 不同的顺序。 | 保留「精确但不规范」豁免：Rust 可以先用更简单的 LCS/splice，只要重放结果相同；`m` 的双射校验不可省。 |
| reserved key 的对象不能逐键 diff，整块折成祖先 `s`（变化时）或空。 | diff.ts:422–433 | 发出带危险段的路径。 | 同 1.5 保留段折叠。 |
| Wire 压缩只有三种：路径第二次使用时定义 id（`#`）、id 引用、短写复用上一 op 的路径；`r` 恢复点清空两侧字典；**短写作用域按批次，不跨批**；一个 encoder/decoder 对只服务一条有序状态流。 | index.ts:52–73, index.ts:565–663, index.ts:665–742 | 跳批/换流后解到错误路径；恢复后引用了读者没见过的 id。 | `Codec{ seen, ids, next_id, previous: Option<Path> }` 每批重置 previous、遇 R 全清；解码器逐 op 校验并拒绝未知动词。 |

## 二、TS/Node 偶然 —— Rust 里该换的形状

| TS/Node 机制 | 为什么只是偶然 | Rust 替代 | 上游位置 |
|---|---|---|---|
| `Proxy` + `PropertyDescriptor` + `sharedObjectHandler/sharedArrayHandler`（get/set/has/ownKeys/getOwnPropertyDescriptor…） | 为了把 JS 的 `a.b.c = x` / `arr.push()` 语法接到 overlay 上；descriptor 陷阱是为让 JS 反射 API 看起来正常。 | 显式命令式 API：`ctx.set(&path, Value)` / `ctx.delete(&path)` / `ctx.append_text(&path, &str)` / `ctx.array_splice(...)`。本库客户端发的是操作事件，不需要模拟 JS 属性语法。 | tracker.ts:447–493 |
| `WeakMap rawNodes` + `NODE` symbol + raw/proxy 双对象 | 身份映射与 GC，JS 没有稳定引用；Rust 有所有权。 | arena `Vec<Node>` + `NodeId(usize)` 索引；或树形 `&mut` 递归借用。 | tracker.ts:62–115, tracker.ts:497–534 |
| `WeakRef` / `WeakSet` / `WeakMap` 缓存（validated、owned、competitors、overflowedBatches） | 全是 GC 驱动的内存管理，不是语义。 | `HashSet` / `HashMap` / `Rc::Weak` / 直接内容哈希；语义等价即可。 | revision-validator.ts:9, apply-immutable-trusted.ts:17, tracker.ts:245, diff.ts:5 |
| `structuredClone` / `copyJson` / `Object.assign(Object.create(null),…)` / `{...value}` / `Array.from` | JS 深拷贝要绕原型、稀疏数组、rope 等坑；Rust 的 `Value: Clone` 已是独立树。 | `Value::clone` + 局部 `clone()`（只在要写的分支上）；跨线用 serde_json 序列化天然 detach。 | json.ts:31–60, apply-immutable-trusted.ts:87–96 |
| V8 rope / `slice` 持有大字符串 / `ownedSlice` 的 `"x ".slice(0,-1)` 技巧 | V8 特定表示问题；`+=` 的 O(N×length) 成本。 | `String` 切片用 `to_owned()` 就是独立分配；保留 `appendText` 的**语义**（直接发 `a` op），不保留 V8 的拷贝技巧。 | index.ts:118–121, tracker.ts:363–388, README.md:215–249 |
| `JSON.stringify(path)` 作 key | 单射需求本身是语义（见 1.2），但字符串编码不是。 | `#[derive(Hash,Eq)] enum Seg` + `Vec<Seg>` 作 `HashMap` key。 | pending.ts:162–163, index.ts:563–564 |
| `Object.defineProperty(DATA_DESCRIPTOR)` / `Reflect.ownKeys` / `getOwnPropertyDescriptor` | 防止原型 setter 与 getter 副作用；Rust 数据模型无原型。 | 直接 `map.insert(k, v)`；保留的是「拒绝危险段名」的检查。 | apply-immutable-trusted.ts:98–103, index.ts:379–386 |
| `Uint8Array` bitset / treap `priority` / `seed` / piece tree | 性能结构，服务大数组编辑；不是对外行为。 | `Vec<bool>` / `Vec<Element>` / `BTreeMap`；保留 handle 跟随元素移动的语义。 | diff.ts:88–89, tracker.ts:874–1014 |
| `then` 陷阱（避免 draft 被 `await` 当成 Promise） | JS 的 thenable 特殊规则。 | 不适用。 | tracker.ts:455–460 |
| `sort` comparator 可变草稿 / coercion 回调 | README 明确列为契约外；Rust 不需要。 | 公开 API 不暴露可变闭包给排序过程。 | README.md:259–260 |

## 三、Rust 文件划分建议

**建议：不逐一对应。** 上游按 JS 实现细节切（tracker 一个文件塞了状态机、Proxy、piece tree、emit），Rust 应按「语义责任」重新切；`nous-chord` 无内部依赖（AGENTS.md crate 契约图），这些模块只依赖 `serde_json` 与本 crate 的 `json` 模块。

建议布局（`crates/nous-chord/src/delta/`，括号内为预估行数，合计约 1,500–1,900 行 Rust 可覆盖上游 3,969 行 TS）：

```
delta/
  mod.rs            ~120   Op/WireOp/PendingTake 类型、Path/Seg/NonEmptyPath、is_base、RESERVED_SEGMENTS 与公开 re-export
  op.rs             ~220   assert_valid_op / assert_safe_path / assert_permutation / UnsafePathError / PathError；Rust 里用 Result 替代 assert
  diff.rs           ~380   diff_revisions + overlap + json_cost/op_cost（保留「精确但不规范」）
  pending.rs        ~220   PendingOps 四状态 + 折叠 + 10_000 溢出（几乎一一对应，因为它本身就是纯算法）
  validator.rs      ~90    revision validator：严格 JSON 校验 + 已验证缓存（Value 语义下可简化）
  apply.rs          ~280   apply_immutable_trusted + apply_immutable_batches 的 owned 集合；trusted/wire 两个入口
  codec.rs          ~240   encoder/decoder + 路径字典 + 批次恢复点
  tracker.rs        ~400   Tracker/Change/Prepared 状态机 + revision + 竞争失效 + appendText
  draft.rs          ~260   Draft 的显式操作 API（set/delete/array mutators/appends 记录）+ 写入校验
  overlay.rs        ~300   节点树 + 位置/路径解析 + 数组元素稳定 id（吸收 piece tree 的 load-bearing 部分）
  emit.rs           ~200   dirty 收集 → op 发射（含保留段折叠、dense region、4096 上限）
  [tests/]          ~400   from-upstream 语义一致性用例（fold/apply/tracker 生命周期/UTF-16 t）
```

取舍说明：

- **合并**：`revision-validator.ts`(75) + `draft.ts`(10) 的公共入口语义分别并进 `validator.rs` / `draft.rs`；`apply-immutable-trusted.ts`(125) 与 `index.ts` 里的 `apply*`(约 200 行) 合成 `apply.rs`，因为两者共享「owned 集合」。
- **拆分**：`tracker.ts` 2319 行不要照搬成一个文件；把**生命周期状态机**、**draft 操作**、**数组 overlay**、**op 发射**四件事分开，才能分别测试。
- **`index.ts` 742 行**里三类东西分开：op 类型放 `mod.rs`，校验放 `op.rs`，codec 放 `codec.rs`，mutable/immutable applier 放 `apply.rs`。
- **依赖方向（单向，不成环）**：

```
json / serde_json（叶子）
   ↑
mod.rs (types)  ←  op.rs (validation)  ←  codec.rs (wire ↔ op)
   ↑                   ↑
   └── diff.rs ────────┘        pending.rs（只依赖 op 类型）
                   ↑
              validator.rs
                   ↑
              apply.rs (trusted + immutable replay)
                   ↑
       overlay.rs → emit.rs → draft.rs → tracker.rs (顶层，组装以上)
```

  即：`types → validate/codec/diff/pending → validator → apply → overlay/emit/draft → tracker`。箭头只向上；`tracker` 是唯一组装者，`op.rs` 不反向依赖 `tracker`。`pending.rs` 只依赖 `mod.rs` 的类型，可与 `apply` 并行开发。
- **与 Nous 内核的接缝**：`tracker` 产出的 `Prepared{ base, value, ops, base_revision }` 交给 `nous-durable`；SSE 出口经 `nous-wire`（`wire.md`），**不要**把 chord 的 codec id 直接暴露到 `/v1/*` 事件里。

## 四、上游 README / 注释里写明、值得进 `docs/pitfalls-from-pi.md` 的坑

> 本票只列候选，不修改 `docs/pitfalls-from-pi.md`（该文件「只追加」，按 AGENTS.md 4.1 的 changelog 采样格式维护；这些是 README 里的设计警告，不是 changelog 条目）。下一轮落地票可择要追加。

| 坑（上游原话要点） | 现象 / 为什么中 | 上游位置 | 我们落地时的判定 |
|---|---|---|---|
| **Immutability is an ownership contract. Nothing is frozen… illegal mutation silently corrupts state.** | 没有冻结，越权修改不报错、只静默污染权威。 | README.md:6–8, README.md:253 | **中**。Rust 用所有权/`&` 挡住大部分；但凡跨 FFI/共享缓存暴露 `Value` 的 `&mut` 就要立规矩。 |
| **`draft.text += chunk` 是二次复杂度**；2 万次追加 40 字到 800KB：带检查 2.4s，无检查 1ms。用 `appendText` 把 chunk 交给 delta。 | 流式回答、工具输出、日志逐 chunk 写会烧穿 CPU。 | README.md:215–249, tracker.ts:363–388, index.ts:64–83 | **中且最高优先级**：Nous 的 SSE 合帧铁律（`wire.md`）正是逐 token 流；Rust 版必须提供 `append_text` 等价 API 并让 emit 直接发 `a`。 |
| **A change inside a large flat array copies that array's pointer storage for the new revision.** | 大数组每次改都复制整块指针存储。 | README.md:257, index.ts:448–460 | **中（性能）**：会话 entries 数组会很长；overlay 用稳定 id/rope 而不是整段 clone。 |
| **Placement validation on bulk inserts**：一次 unshift 10 万个三字段对象多花 ~19ms / 20MiB 临时堆。 | 批量插入校验成本。 | README.md:258, tracker.ts:1149–1164 | **中（性能）**：Rust 无 JS 原型检查，损耗小得多；但仍应一次校验整批再插入。 |
| **Draft reads go through proxies**：2 万次热嵌套读取 9ms vs 裸对象 0.08ms。 | 代理读放大 ~100×。 | README.md:259 | **不中**：Rust 不走 Proxy，显式 API 无此税。 |
| **decode() does not copy payloads**；同一 `WireOp[]` 解给两个可变副本会让它们别名；mutable replay 必须给每个副本 detach 一份 batch。 | 一个 batch 扇出到多副本会互相串改。 | README.md:67–80, index.ts:344–356 | **中**：Nous 可能把一条 SSE 事件扇出给多个客户端视图；扇出点必须 clone/序列化，或只用不可变 apply。 |
| **Object identity is not replicated** — 每个路径是独立 value placement；root 里同一容器出现在两个键下会让 `prepared.value` 与副本分叉。 | 别名根导致权威自相矛盾。 | README.md:261, README.md:122–124 | **中**：进入 tracker 前必须拒绝/展开别名；Rust 值树天然无别名，但如果引入 `Rc` 共享要立禁令。 |
| **`r` 是恢复点，解码器必须在 `r` 时清空路径字典**；跨批复用 id 会引用读者没见过的定义，恢复失败。 | SSE 重连从最近的 `r` 重放会解不开。 | index.ts:587–598, index.ts:685–688, README.md:206–210 | **中**：`wire.md` 的 `snapshot` 帧就是恢复点；codec 重置规则必须一致。 |
| **Appliers reject reserved path segments** (`__proto__`/`constructor`/`prototype`)；op 可能回显模型输出，不可信。 | 原型污染 / 继承 setter。 | README.md:201–205, index.ts:143–157 | **中（降为显式校验）**：Rust 无原型链，但要在解码边界保留拒绝，保持跨实现一致。 |
| **`undefined` 在 placement 里不丢弃而是抛错**；对象属性 `= undefined` 是删除，数组里抛。 | `JSON.stringify` 会悄悄丢 `undefined`，导致候选值与 op 重放不一致。 | README.md:129–148 | **不中（结构不同）**：Rust/JSON 无 `undefined`；但要保证反序列化时 `null` 不被当「缺失键」处理。 |
| **No-op 也推进 revision 并让竞争者 stale；Replicated state 不发布 no-op。** | 漏推进会让旧 prepared 被误采纳。 | README.md:169–179 | **中**：tracker 状态机照搬。 |
| **One tracker is one revision sequence. Delivery order and persistence belong to the surrounding protocol.** | 把排序/持久化塞进 delta 会与 `nous-durable` 的落库顺序打架。 | README.md:262 | **中**：`nous-durable` 负责落库与 SSE 顺序；delta 只产出 ops，不碰存储。 |
| **External proxies / mutable sort comparators / non-numeric comparator results 是契约外。** | 比较器在排序时 prepare/adopt 会造成未定义行为。 | README.md:259–260 | **不中**：Rust API 不暴露这种回调。 |

## 五、给落地票的三条验收建议

1. 以「对同一 base 重放后 JSON 相等」为一致性判据，不用 op tuple 逐项比对（README.md:198–200）。
2. 必测四条端到端语义：pending 折叠、UTF-16 `t` 截断、`appendText` 多 chunk 合并、以 `r` 为恢复点的 codec 重置。
3. 信任边界用例：`WireOp → validate → TrustedOp → apply` 的单向转换必须能在类型层表达；不可信路径段在解码时被拒。
