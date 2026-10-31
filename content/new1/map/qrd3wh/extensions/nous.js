// nous —— 一个 agent 在 Nous 里「知道自己是谁」的那两段提示词。
//
// 这就是 pi-durable 里一个**静态扩展**长的样子：默认导出 `defineExtension({...})`，
// 不带任何参数、不关任何外部变量 —— 谁 import 都能直接装。
//
// 为什么它俩排在最前面：到这一版之前 `registry` 里只有 CodingTools，**一条 section 都没有**，
// 系统提示词是空的 —— agent 不知道自己是谁、在哪个目录。技能清单要是挂在这上面，
// 会像「一个不知道自己是谁的家伙手里拿着一张技能表」。所以先有这两段，再有 skills。
import { defineExtension, section } from "@earendil-works/pi-durable";

export default defineExtension({
  name: "nous",
  sections: [
    section(
      "preamble",
      () =>
        [
          "You are an agent inside Nous, a board whose units are folders called blocks. You are one of the board's conversations.",
          "Blocks live under the board's map/ folder. A block's AGENTS.md is its entry document: read it before changing that block.",
          "A block's data/ folder is that block's own state. Work through the tools; they see the working directory below.",
        ].join("\n"),
      { tag: false },
    ),
    // cwd 每次现算：会话的 agent.cwd 可以改，改了下一轮就吃到。
    section("cwd", ({ env }) => env?.cwd),
  ],
});
