# <module>

> [中文](#zh) · [English](#en)

<!--
插件文档模板（issue #55）。复制本文件到 crates/<vendor>/<module>/README.md，
替换 <module> 与各章内容。规矩：
  - 第 3 章由 check-plugins.sh 从相邻 plugin.toml 生成，禁止手写（这里只留指向）。
  - 第 4 章只写一行「待生成」，等 #48 的 /v1/_routes；禁止手写 API 表格。
  - 每章中英各一段；先写中文，再写英文。
-->

<a id="zh"></a>

## 1 它是什么 / What it is

<!-- 中文：一句话说清这个模块的身份与边界。 -->
<!-- EN: one sentence on identity and boundary. -->

中文：`<module>` 是什么、负责什么、不负责什么。

EN: What `<module>` is, what it owns, and what it explicitly does not own.

## 2 什么时候用 / When to use

<!-- 中文：该用的场景，以及不该用的场景。 -->
<!-- EN: when to reach for it, and when not to. -->

中文：该用它的场景……；不该用它的场景……。

EN: Reach for it when …; do not reach for it when ….

## 3 清单 / Manifest

由脚本从相邻 `plugin.toml` 生成，禁止手写。

Generated from the adjacent `plugin.toml` by the checker; do not hand-write.

## 4 API

> 待生成：见 #48 的 /v1/_routes（禁止手写占位表格）

## 5 权限 / Permissions

<!-- 中文：谁能调用、需要什么权限、有没有跨层调用限制。 -->
<!-- EN: who may call it, required permissions, cross-layer limits. -->

中文：调用权限与限制……

EN: Permissions and limits …

## 6 失败模式 / Failure modes

<!-- 中文：出错时怎么表现、怎么恢复。 -->
<!-- EN: how failures surface and how to recover. -->

中文：失败模式与恢复……

EN: Failure modes and recovery …

## 7 判据 / Judgement

<!-- 中文：怎么判断做完了、测试在哪。 -->
<!-- EN: how to judge done, where the tests live. -->

中文：判据与测试入口……

EN: Judgement and test entry points …

## 8 反悔点 / Reversal

<!-- 中文：哪些决定可逆、哪些不可逆，回退路径是什么。 -->
<!-- EN: which decisions are reversible and what the way back is. -->

中文：可反悔处与回退路径……

EN: Reversible decisions and the way back …

<a id="en"></a>
