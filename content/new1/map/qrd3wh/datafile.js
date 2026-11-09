// 一小张落 `data/` 的表：读进来、动了就攒着写回去。`activity.json` 和 `read.json` 共用这一份。
//
// 为什么共用：两边是同一台机器 —— 先写临时文件再 rename（读到半截 json 的代价是整张表归零）、
// `writing` 链防两次写撞车、`timer.unref()` 别把进程钉住、落盘失败只记一行日志。
// 抄两份迟早会漂（这一盘的文档名就漂过一次：客户端补 `.md`、服务端不补），所以只留这一份。
//
// 单独一个文件是为了能被单独验：不 import server.js（那个有顶层 await + sqlite）。

import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";

/** 读盘。读不懂 / 没有文件就当空表 —— 这张表丢了不是数据坏了，是「还不知道」。 */
const readTable = async (file, parse) => {
  try {
    return parse(JSON.parse(await readFile(file, "utf8")));
  } catch {
    return {};
  }
};

/**
 * @param file    落盘位置（`data/xxx.json`）
 * @param version 文件里的 `v`
 * @param field   表存在文件里的哪个字段下
 * @param parse   从盘上那份 json 里取出这张表（顺手丢掉读不懂的条目）
 * @param tag     日志前缀（`[activity]` / `[unread]`）
 * @param flushMs 攒多久写一次盘
 */
export const openDataFile = async (file, { version, field, parse, tag = "data", flushMs = 500 }) => {
  const table = await readTable(file, parse);
  let timer = null;
  let writing = null;
  let dirty = false;

  const write = async () => {
    if (!dirty) return;
    dirty = false;
    const text = JSON.stringify({ v: version, [field]: table }, null, 2) + "\n";
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

  /** 表刚动过：攒一小会儿再写。一次回复有几十条 entry、水位一次也只动几笔，不必每条都 fsync。 */
  const touch = () => {
    dirty = true;
    if (timer) return;
    timer = setTimeout(() => {
      timer = null;
      void flush().catch((error) => console.error(`[${tag}] 落盘失败`, error));
    }, flushMs);
    // 别为了等一张表把进程钉住：退出前没写完就算了，下次动了还会写。
    timer.unref?.();
  };

  return { table, touch, flush };
};
