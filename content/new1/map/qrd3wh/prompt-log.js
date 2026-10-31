// 提示词增量的回放 —— 从 `pi.system` 条目里还原「此刻生效的提示词」。
//
// 为什么需要它：durable 存提示词的方式是**增量**的。每个 `pi.system` 条目只带变了的段
// （`message.sections` 是 `{key: 文本 | null}`，`null` = 这一段被删了），其余的沿用前一条；
// 工具也一样（`toolsAdded` / `toolsRemoved`）。所以「这一条里有什么」永远答不了
// 「这一轮提示词里有什么」—— 后者只能靠按时间顺序回放整条会话。
//
// 规则跟 pi-durable 的 `replaySections` 一致：set 覆盖、null 删除、后加的追加。
// 单独一个文件是为了它能被单独验（纯读，不碰 HTTP、不碰扩展）。
export const SYSTEM_SCAN_MAX = 4000; // 调试用途：别为了看一眼提示词把大会话整个扫穿

export const replayPrompt = async (conversation, ctx) => {
  const sections = new Map();
  const tools = new Map();
  let cursor;
  let scanned = 0;
  let truncated = false;
  for (;;) {
    // order: ascending —— 回放必须按时间走。游标自带顺序，所以只在第一页声明一次。
    const page = await conversation.entries({ order: "ascending" }, 200, cursor, ctx);
    for (const entry of page.items) {
      if (entry.kind !== "pi.system") continue;
      const message = entry.model?.[0] ?? {};
      for (const [name, text] of Object.entries(message.sections ?? {})) {
        if (text === null) sections.delete(name);
        else sections.set(name, text);
      }
      for (const tool of message.toolsRemoved ?? []) tools.delete(tool.name);
      for (const tool of message.toolsAdded ?? []) tools.set(tool.name, true);
    }
    scanned += page.items.length;
    if (page.next === undefined) break;
    cursor = page.next;
    if (scanned >= SYSTEM_SCAN_MAX) {
      truncated = true;
      break;
    }
  }
  return { sections: Object.fromEntries(sections), tools: [...tools.keys()], scanned, truncated };
};
