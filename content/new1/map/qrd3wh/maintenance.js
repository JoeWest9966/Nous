// maintenance —— `agent.sqlite` 的空间回收（VACUUM）。
//
// 为什么单独一层：VACUUM 要**独占重写整个文件**，而 agent.sqlite 在进程活着的时候
// 一直被 durable 的 session 握着。所以这个文件只回答一件事：**什么时候动它**。
//
//   · 启动早期（storage 打开之前）—— 默认路径，零感知，不用问任何人；
//   · 手动端点（会话全闲时）—— 兜底，忙的时候直接拒。
//
// 判定写成纯函数：能裸测，不需要真库，也不需要起服务。

import { DatabaseSync } from "node:sqlite";

/** 空洞少于这么多页不值得为它重写整个库（1 页通常 4KB，2048 页 = 8MB）。 */
export const MIN_FREELIST_PAGES = 2048;

/** 空洞占比低于这个也不值得 —— VACUUM 期间库是锁住的，时间要花在刀刃上。 */
export const MIN_FREELIST_RATIO = 0.2;

/**
 * 要不要 VACUUM。**只看空洞，不看文件多大**：一个 100MB 的库没有空洞就不该动它，
 * 一个 2MB 的库空洞占 60% 就值得。
 */
export const vacuumPlan = ({ pageCount = 0, freelistCount = 0, pageSize = 0 } = {}) => {
  const pages = Number(pageCount) || 0;
  const free = Number(freelistCount) || 0;
  const size = Number(pageSize) || 0;
  const ratio = pages > 0 ? free / pages : 0;
  return {
    pageCount: pages,
    freelistCount: free,
    pageSize: size,
    // 能估出来多少字节的空洞；页大小读不到就是 0（不是「没有空洞」，是「算不出」）。
    wasteBytes: size > 0 ? free * size : 0,
    ratio,
    should: free >= MIN_FREELIST_PAGES && ratio >= MIN_FREELIST_RATIO,
  };
};

/** 读现况。只读连接，永远安全（库不存在会抛 —— 调用方决定是当错误还是当「还没建」）。 */
export const inspect = ({ dbPath }) => {
  const db = new DatabaseSync(dbPath, { readOnly: true });
  try {
    const one = (sql) => db.prepare(sql).get() ?? {};
    return vacuumPlan({
      pageCount: one("PRAGMA page_count").page_count,
      freelistCount: one("PRAGMA freelist_count").freelist_count,
      pageSize: one("PRAGMA page_size").page_size,
    });
  } finally {
    db.close();
  }
};

/**
 * 真跑一次 VACUUM。库被别人占着就抛（`SQLITE_BUSY`）—— 调用方决定是 409 还是记一行日志。
 *
 * 先量、后跑、再量：`freedBytes` 是**空洞**的减少量，不是文件大小的变化
 * （VACUUM 之后文件通常也会缩，但那是结果，不是判据）。
 */
export const vacuum = ({ dbPath, busyTimeoutMs = 5000 }) => {
  const before = inspect({ dbPath });
  const db = new DatabaseSync(dbPath);
  try {
    db.exec(`PRAGMA busy_timeout = ${Number(busyTimeoutMs) || 0}`);
    db.exec("VACUUM");
  } finally {
    db.close();
  }
  const after = inspect({ dbPath });
  return {
    freedBytes: Math.max(0, before.wasteBytes - after.wasteBytes),
    before,
    after,
  };
};
