// 未读水位 —— 每条会话「读到哪了」，落 `data/read.json`。
//
// 存的是 **EntryId 水位**，不是条数：fork 和 compaction 会让「条数」变成另一件事
// （同一条 entry 可能出现在两条会话里、老条目可能被压掉），水位单调，不会因此错位。
//
// 水位的来历只有两处，都不是「问一眼」问出来的：
// - 新会话出生时种 0（`begin`）—— 之后进来的每条 entry 都算未读；
// - 老会话启动时种在「当时最新」（`seed`）—— 不一开张就几百条未读。
// 没有水位（盘上没这份、两处都没轮到的会话）→ 老实回 `{ unread: 0, readAt: null }`，
// **不顺手种**：`GET /v1/unread` 是读端点，不许有落盘副作用。
// 有水位 → 未读 = 「水位之后还有几条」，由 durable 那边数（我们只传水位）。
// `readAt: null` = 这个水位是引擎种的，还没被「标已读」推进过。
//
// 水位只进不退：拿一个更小的「最新 id」回来（fork 出来的新会话、回滚）不许把水位往回拨。
//
// 落盘那台机器在 datafile.js（跟 updatedAt 那张表共用一份）。

import { openDataFile } from "./datafile.js";

const FILE_VERSION = 1;

/** 盘上那份表：`{ "<会话 id>": { lastReadId, readAt } }`。读不懂的条目丢掉。 */
const parse = (saved) => {
  const table = {};
  for (const [id, mark] of Object.entries(saved?.marks ?? {})) {
    if (!Number.isFinite(mark?.lastReadId)) continue;
    table[id] = { lastReadId: mark.lastReadId, readAt: Number.isFinite(mark.readAt) ? mark.readAt : null };
  }
  return table;
};

/**
 * @param file    落盘位置（`data/read.json`）
 * @param now     时钟
 * @param entries durable 那一半：`latest(id)` 最新 EntryId、`countAfter(id, afterId)` 之后有几条
 */
export const openUnread = async (file, { now = Date.now, entries } = {}) => {
  const { table, touch, flush } = await openDataFile(file, {
    version: FILE_VERSION,
    field: "marks",
    parse,
    tag: "unread",
  });

  const key = (id) => String(id);

  /**
   * 老会话补种：没有水位就把水位种在「当时最新」上。返回有没有种。
   *
   * 一条 entry 都没有的会话不在这儿种 —— 那是新会话，走 `begin` 种 0。
   */
  const seed = async (id) => {
    if (table[key(id)]) return false;
    const latest = await entries.latest(id);
    if (latest === null) return false;
    table[key(id)] = { lastReadId: latest, readAt: null };
    touch();
    return true;
  };

  /**
   * 新会话出生：水位种在 0（= 「一条都还没读过」）。幂等，已有的不动。
   *
   * 不能像老会话那样种在「当时最新」：新会话出生时还没有 entry，等第一条消息进来再种
   * 就已经晚了 —— 那条消息会被吞成 0 条未读。种 0 之后，进来的每条都算未读。
   */
  const begin = (id) => {
    if (table[key(id)]) return false;
    table[key(id)] = { lastReadId: 0, readAt: null };
    touch();
    return true;
  };

  return {
    /** 原样给出水位；没有就是 null。 */
    watermark: (id) => table[key(id)] ?? null,

    /** 老会话的种子（启动时补一次）：种在「当时最新」。幂等：已有的不动。 */
    seed,

    /** 新会话出生：种 0 = 之后进来的每条 entry 都算未读。幂等：已有的不动。 */
    begin,

    /**
     * { unread, readAt }。没有水位 = `{ unread: 0, readAt: null }`。
     *
     * **只读**：不顺手种。`GET /v1/unread` 拿它算，读端点不许有落盘副作用；
     * 种水位是启动时那次（老会话）和新会话出生时（种 0）的事。
     */
    unreadOf: async (id) => {
      const mark = table[key(id)] ?? null;
      if (!mark) return { unread: 0, readAt: null };
      return { unread: await entries.countAfter(id, mark.lastReadId), readAt: mark.readAt };
    },

    /** 把 id 标成读到最新，回 { id, readAt, unread: 0 }。水位只进不退。 */
    markRead: async (id) => {
      const latest = await entries.latest(id);
      const prev = table[key(id)] ?? null;
      const readAt = now();
      // 拿更小的 latest 回来（回滚 / fork）也不许把水位往回拨。
      const lastReadId = prev && (latest === null || prev.lastReadId > latest) ? prev.lastReadId : latest;
      table[key(id)] = { lastReadId: lastReadId ?? prev?.lastReadId ?? 0, readAt };
      touch();
      return { id: Number(id), readAt, unread: 0 };
    },

    /** 把攒着的写下去（测试和关停用）。 */
    flush,
  };
};
