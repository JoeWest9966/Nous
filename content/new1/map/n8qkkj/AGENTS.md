# n8qkkj · trigger

Nous **触发器**：每个触发器是一个**独立的 Rust 项目**，按自己的条件决定「现在该不该起一条
新对话」。这一页管理这些项目（列出、改源码、编过才提交），但**不跑它们** —— 跑它们的是
qrd3wh 的调度器（`scheduler.js`），起什么对话的是终极模板（`jtjzw5` / `Ultimate-Template` 段）。

一个触发器只管一件事：**回答「现在该不该 fire」**。它不认识模板、不认识会话、不自己开会话。

---

## 触发器契约（写触发器的人必须读这一节）

这是整个触发器系统唯一一份「触发器长什么样」的契约。将来开发新触发器的 agent，和「制作
触发器」的 skill，都以这一节 + 仓库里的 `single-shot/` 样例为准。

### 输入 / 输出

```
stdin  一行 JSON：{"now": <epoch 秒>, "params": {…}}
stdout 一行 JSON：{"fire": true|false, "reason": "…", "data": {…}}
退出码 0 = 正常（fire 真假都算正常）；非 0 = 出错
```

三条铁律：

1. **`fire` 是触发器唯一的裁决。** `true` = 调度器该起会话；`false` = 什么都不发生。
2. **退出码 0 和非 0 必须分开。** 「今天不该跑」是 `{fire:false}` + 退出码 0；「跑挂了」
   是退出码非 0（+ stderr 一句话）。调度器靠这个区分「不该跑」和「出错了要报红」。
3. **`reason` 是给人看的，不是给程序判的。** 程序只认 `fire` 和退出码。

### 三个关键设计

- **`now` 由调用方给**（stdin 里那个字段），没有才退回本机时钟。同一批输入永远得出同一个
  结论，才谈得上重放和测试。测试用 `--now <秒>` 覆盖。
- **状态（跑没跑过）落本机，不进仓库。** `$NOUS_STATE_DIR` 或 `$XDG_STATE_HOME/nous-trigger/`
  是调度器统一给的地方。触发器自己在里面记「这次已经 fire 过了」—— 换一台机器同一个时刻
  仍会 fire 一次，那是**对的**（「这台机器上还没跑过」才是它该判断的事）。
- **过时不补。** 到了该 fire 的那分钟机器关机了，之后再醒来就是「错过了」，**不偷偷补**。
  补跑得显式开（xyops 叫 catchup），以后再加。机器关了三天、开机起三十条会话不是惊喜。

### 参数声明（`nous.json` 的 `params`）

触发器要人填什么，写在 `nous.json` 的 `params` 里。**面板（jtjzw5）照这份声明画表单**，
所以触发器加参数 = 改这里，面板一个字节都不用动。

```json
{
  "name": "single-shot",
  "version": "0.1.0",
  "summary": "指定的那一分钟里发起一次 —— 过时不补，跑过不再跑",
  "category": ["时间", "时间/一次"],
  "params": [
    { "name": "epoch", "type": "epoch", "label": "什么时候", "required": true,
      "help": "精确到分钟：落在这一分钟里就发起一次。" }
  ]
}
```

参数**类型**是封闭词汇表，一个 param 一项：

| type | 值 | 页面画什么 |
|---|---|---|
| `epoch` | UTC 秒（正整数） | 日期时间选择（本地时间 ↔ UTC 秒） |
| `number` | 数字 | 数字框 |
| `string` | 字符串 | 文本框 |
| `text` | 字符串 | 多行框 |
| `boolean` | `true` / `false` | 开关 |
| `select` | 字符串 | 下拉（必须带 `options: ["…"]`） |

每个 param 的字段：`name`（必填，小写字母数字 `_ -`）、`type`（必填）、`label`（给人看的
名字）、`required`（布尔，默认 false）、`help`（一句话）、`default`（默认值）、`options`
（只 `select` 有）。

**加一个 type 是「说清楚了才加」的决定，不是给某个触发器开后门。** 页面里永远不许出现
`if (触发器名 === "single-shot")` 这种代码 —— 那是两份代码迟早对上的死法。

### 最小可用的样子

```rust
use serde_json::json;
fn main() {
    println!("{}", json!({"fire": false, "reason": "还没写条件"}));
}
```

能编过、退出码 0、stdout 一行合法 JSON。完整范本看 `Nous-Trigger/single-shot/src/main.rs`
—— 它演示了：读 stdin、解析 `params`、纯函数判断（可测）、状态文件、`--now` 测试钩子、
5 个单元测试。

---

## 三条不许越过的线

1. **保存 ≠ 发布。** 保存只落盘；发布才 `cargo check` → `node index.mjs` → commit → push。
   **发布还会 `cargo build`**（出 `target/debug/<name>`）—— 调度器跑的是这个产物，只 check
   不 build，触发器在调度器眼里就是「没编译、跑不了」。
2. **发布只发一个。** `publish` 只 `git add -A -- <name>/ index.json`。别人的脏文件一个都
   不许卷进来。
3. **类别自动推，代码不自动推。** 类别写回仓库后 8 秒 debounce 走 `syncMeta`（只 add
   `categories.json`、`index.json`、`*/nous.json`）。`.rs` 永远得人去点发布。

## 身份与改名

`name` 是**目录名 + 清单 name + 二进制名**三合一，`index.mjs` 会拒掉三者不一致的目录。
页面上**不提供改名** —— 改它等于换一个项目，也让「终极模板里引用的触发器」对不上账。

## 索引是扫出来的

`index.json` 的唯一作者是仓库里的 `index.mjs`。服务端每次发布 / 归档 / 新建之后都跑它。
手维护索引的必然下场：加了目录忘了登记、删了目录忘了划掉。

## 归档

- **发布过的**：`git mv <name> .archive/<name>` + 提交 + 推送。备份留在仓库里，能恢复。
- **没发布过的**：直接 `rm -rf`。仓库里从来就没有它，没有备份也没有历史 —— 确认框说的是
  「删掉」而不是「归档」。
- `.archive/archived.json` 记着归档时的版本 / 说明 / 类别。

## 界面上的取舍（照抄 vbe4pt，别自己发明）

- 状态只住顶栏筛选条（已发布 / 有改动 / 未发布 / 清单坏了）；左栏只管类别。
- 卡片只放身份：色砖、名字、版本、一句说明、类别、产物名。动作全在详情和 ⋯ 菜单里。
- 破坏性动作要手输名字；详情是浮层、代码是抽屉（抽屉高一层）。
- 没装过的概念一个都不留：没有「已装载」「默认」「变体」「本机装卸」。

## 谁在跑触发器

**不是这一页。** 这一页是清单 + 编辑器。真正每分钟跑触发器、`fire` 了去起会话的，是
`qrd3wh/scheduler.js`（调度器）+ 终极模板（`Ultimate-Template` 段）。触发器作者**不需要**
知道这些细节，只需要守住上面的契约：stdin 进、stdout 出、fire 是唯一裁决。

## 端到端一把跑

```bash
cd /opt/Nous-Trigger && node index.mjs --check          # 清单跟盘上对不对得上
cd /opt/Nous-Trigger/<name> && cargo test               # 纯函数判断的测试
cargo build                                            # 出 target/debug/<name>（调度器跑这个）
echo '{"now":1800000060,"params":{"epoch":1800000060}}' | ./target/debug/<name>
```
