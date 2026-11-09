// 会话「最后一次落库」的时刻表 —— 一条会话一个 ms，落 `data/activity.json`。
//
// 为什么得自己攒：durable 的 `EntryRecord` 只有 id / kind / data，`agent.sqlite` 里没有
// wall-clock 列。所以「17:42」这种绝对时间**不可能**从库里的数据算出来，只能在每条 entry
// 写进来的时候顺手记一下。
//
// 两条纪律：
// - 记过才有数，没记过返回 `null`。宁可这个会话「不知道什么时候动的」，也不要拿
//   别的时间（文件 mtime、启动时刻）冒充 —— 那会让列表把三个月前的会话画成「刚刚」。
// - 落盘 debounce：一条回复有几十条 entry，每条都 fsync 一次不值得。攒着，稍后一起写。
//
// 落盘那台机器在 datafile.js（跟未读水位共用一份）。单独一个文件是为了它能被单独验：
// 不 import server.js（那个有顶层 await + sqlite）。

import { openDataFile } from "./datafile.js";

const FILE_VERSION = 1;

/** 盘上那份表：`{ "<会话 id>": <epoch ms> }`。读不懂的条目丢掉 —— 宁可空，不要假时间。 */
const parse = (saved) => {
  const table = {};
  for (const [id, at] of Object.entries(saved?.at ?? {})) {
    if (Number.isFinite(at)) table[id] = at;
  }
  return table;
};

/**
 * @param file    落盘位置（`data/activity.json`）
 * @param now     时钟，注入是为了测试能拿到确定的数
 * @param flushMs 攒多久写一次盘
 */
export const openActivity = async (file, { now = Date.now, flushMs = 500 } = {}) => {
  const { table, touch, flush } = await openDataFile(file, {
    version: FILE_VERSION,
    field: "at",
    parse,
    tag: "activity",
    flushMs,
  });

  return {
    /** 这条会话最后写库的时刻（epoch ms）。没记过 = null。 */
    at: (id) => table[String(id)] ?? null,
    /** 刚写了一条 entry。返回记下的时刻。 */
    note: (id) => {
      const at = now();
      table[String(id)] = at;
      touch();
      return at;
    },
    /** 把攒着的写下去（测试和关停用）。 */
    flush,
  };
};
