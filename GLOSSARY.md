# Nous

一个会开很多对话的 AI 面板：会话引擎（qrd3wh）、模板仓（Nous-Template）、触发器仓（Nous-Trigger）三边协作，把「什么时候、起一条什么对话」这件事说清并跑起来。

## 触发器与血缘

**触发器**：
Nous-Trigger 仓里的一个独立程序（一个目录 = 一个 Rust 项目），只回答「现在该不该 fire」。契约：stdin 一行 `{now, params}`，stdout 一行 `{fire, reason}`；退出码 0 = 正常（fire 不 fire 都算）。
_Avoid_: 启动器、launcher

**终极模板**：
Nous-Template 的 Ultimate-Template 段里的一条，等于「触发器 + 参数 + 会话模板」，钉「什么时候起、起什么」。
_Avoid_: 定时任务、cron、计划

**会话模板**：
Nous-Template 的 Session-Template 段里的一条，管「起什么对话」（模型、技能、提示词模板等初值）。
_Avoid_: 预设（两段条目共用的泛指）

**血缘**：
一条会话由哪条终极模板 fire 出来的历史事实，记在会话引擎 Sessions 文档的 `origins` 键里：`{ ultimate, trigger, at, forced }`。fire 当时抄下，不随引用删改而变；名字里的 `▶` 前缀只是它的投影。
_Avoid_: 出处、来源、启动记录
