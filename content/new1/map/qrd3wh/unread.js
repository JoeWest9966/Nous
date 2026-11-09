// 未读水位 —— 每条会话「读到哪了」，落 `data/read.json`。
//
// 存的是 **EntryId 水位**，不是条数：fork 和 compaction 会让「条数」变成另一件事
// （同一条 entry 可能出现在两条会话里、老条目可能被压掉），水位单调，不会因此错位。
//
// 两种入口语义不同，别混：
// - 没有水位 = 第一次见这条会话（老会话 / 刚建的会话）→ 先把水位种在「当时最新」，
//   报 0 条未读。不这么做的话，老会话一开张就几百条未读，新会话的未读则永远算不出来。
// - 有水位 → 未读 = 「水位之后还有几条」，由 durable 那边数（我们只传水位）。
//
// 水位只进不退：拿一个更小的「最新 id」回来（fork 出来的新会话、回滚）不许把水位往回拨。

import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

const FILE_VERSION = 1;

const read = async (file) => {
  try {
    const saved = JSON.parse(await readFile(file, "utf8"));
    const table = {};
    for (const [id, mark] of Object.entries(saved?.marks ?? {})) {
      if (!Number.isFinite(mark?.lastReadId)) continue;
      table[id] = { lastReadId: mark.lastReadId, readAt: Number.isFinite(mark.readAt) ? mark.readAt : null };
    }
    return table;
  } catch {
    return {};
  }
};

/**
 * @param file    落盘位置（`data/read.json`）
 * @param now     时钟
 * @param entries durable 那一半：`latest(id)` 最新 EntryId、`countAfter(id, afterId)` 之后有几条
 */
export const openUnread = async (file, { now = Date.now, entries } = {}) => {
  const table = await read(file);
  let timer = null;
  let writing = null;
  let dirty = false;

  const write = async () => {
    if (!dirty) return;
    dirty = false;
    const text = JSON.stringify({ v: FILE_VERSION, marks: table }, null, 2) + "\n";
    const tmp = `${file}.tmp`;
    await mkdir(path.dirname(file), { recursive: true });
    await writeFile(tmp, text);
    await rename(tmp, file);
  };

  const flush = async () => {
    if (timer) {
      clearTimeout(timer);
      timer = null;
    }
    writing = writing ?? write().finally(() => (writing = null));
    await writing;
    if (dirty) await flush();
  };

  // 水位一次只动几笔（标记已读、种一条），攒一小会儿再写，别每个请求 fsync 一次。
  const schedule = () => {
    dirty = true;
    if (timer) return;
    timer = setTimeout(() => {
      timer = null;
      void flush().catch((error) => console.error("[unread] 落盘失败", error));
    }, 500);
    timer.unref?.();
  };

  const key = (id) => String(id);

  /** 没有水位就把水位种在「当时最新」上。返回有没有种。 */
  const seed = async (id) => {
    if (table[key(id)]) return false;
    const latest = await entries.latest(id);
    // 一条 entry 都没有的会话：没有可种的位置，等它有了再说（下次进来就是 0 条未读）。
    if (latest === null) return false;
    table[key(id)] = { lastReadId: latest, readAt: null };
    schedule();
    return true;
  };

  return {
    /** 原样给出水位；没有就是 null。 */
    watermark: (id) => table[key(id)] ?? null,

    /** 种水位（启动时给老会话补、首次见面时顺手补）。幂等：已有的不动。 */
    seed,

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
      schedule();
      return { id: Number(id), readAt, unread: 0 };
    },

    flush,
  };
};
