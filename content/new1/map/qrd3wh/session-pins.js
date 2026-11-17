// session-pins.js —— 会话置顶：一份提交日志，不是手机端的私有 pins.json。
//
// 票 #15 / spec §3.7：置顶进内核。同一 requestId 重放十次只落一条提交，重启还在，
// 删客户端配置也不变。状态在 durable 的 Sessions 文档里（names/machines/archived
// 的邻居），每次改动自带 commit_seq。本模块不 import server.js（有顶层 await + 起服务）。
//
// 形状（wire.md 只加标题，字段形状归测试）：
//   POST /v1/sessions/:id/pin  { pinned: boolean, requestId: string }
//   → { id, pinned, changed, requestId, version }
//   GET  /v1/sessions          items[].pinned + 排序：置顶在前，其余保持原顺序
//
// `version` 是置顶日志的**全局**第几次真提交（跨会话单调；重放不涨）。
// 重放锚按会话限额保留（每个会话最近 50 个），淘汰只影响极旧请求的重放幂等，
// 不影响 pins.ids 里的置顶状态。

/** 每个会话保留最近 50 个重放锚；更老的按插入顺序（= version 顺序）淘汰。 */
const KEEP_PER_SESSION = 50;

/**
 * 淘汰每个会话最旧的重放锚。置顶状态在 pins.ids，不在这里 —— 丢锚只是让极旧请求
 * 不再幂等（再来一次会落成新提交），不影响任何会话的置顶/排序。
 */
const pruneRequests = (requests) => {
  const bySession = new Map();
  for (const [anchor, response] of Object.entries(requests)) {
    const key = String(response?.id);
    if (!bySession.has(key)) bySession.set(key, []);
    bySession.get(key).push({ anchor, version: Number(response?.version) || 0 });
  }
  for (const entries of bySession.values()) {
    if (entries.length <= KEEP_PER_SESSION) continue;
    entries.sort((a, b) => a.version - b.version);
    for (const { anchor } of entries.slice(0, entries.length - KEEP_PER_SESSION)) {
      delete requests[anchor];
    }
  }
};

/** 老盘没有这些键：和 machines / archived 一个待遇，一律 `??=`。 */
const ensure = (doc) => {
  doc.pins ??= { ids: [], requests: {} };
  doc.pins.ids ??= [];
  doc.pins.requests ??= {};
  return doc.pins;
};

/**
 * @param {object} deps
 * @param {(change: Function) => Promise<any>} deps.commit 一次 durable 提交，回调拿 tx
 * @param {(token: any, anchor: string) => Promise<any>} deps.peek 只读重放锚
 */
export const openSessionPins = ({ commit, peek }) => {
  /**
   * 置顶 / 取消置顶。requestId 是这次决定的锚，重放拿回第一次的回包。
   * 读-改-写**全在事务内**（tx.doc 给草稿）：并发两单不会互相覆盖。
   */
  const setPin = async ({ id, pinned, requestId, token }) => {
    const key = String(id);
    const anchor = String(requestId ?? "");
    if (!anchor) throw Object.assign(new Error("requestId 是空的"), { status: 400 });

    const seen = await peek(token, anchor);
    if (seen) return { ...seen };

    // version = 置顶日志的全局第几次真提交（跨会话单调；重放不涨）。
    // 从现存锚的最大 version 推：淘汰会删旧锚，不能再用 Object.keys(requests).length。
    // 不报 durable 的 commit_seq —— 提交线里别人也在推，事务里读不到这一单的真值。
    let response;
    const result = await commit(async (tx) => {
      const pins = ensure(await tx.doc(token));
      const cached = pins.requests?.[anchor];
      if (cached) return { cached };

      const ids = [...(pins.ids ?? [])];
      const was = ids.includes(key);
      response = {
        id: Number(id),
        pinned: Boolean(pinned),
        changed: was !== Boolean(pinned),
        requestId: anchor,
        version: Object.values(pins.requests).reduce((max, r) => Math.max(max, Number(r?.version) || 0), 0) + 1,
      };
      if (pinned && !was) ids.unshift(key); // 新置顶的排最前 —— 跟微信一致
      if (!pinned && was) ids.splice(ids.indexOf(key), 1);
      pins.ids = ids;
      pins.requests[anchor] = response;
      pruneRequests(pins.requests);
      return {};
    });
    if (result.cached) return { ...result.cached };
    return { ...response };
  };

  /** 置顶在前（ids 的先后），其余保持输入顺序 —— 等价于 bjes6b 的 pins.ids 语义。 */
  const order = (records, ids) => {
    const rank = (record) => {
      const at = ids.indexOf(String(record.id));
      return at === -1 ? ids.length : at;
    };
    return records
      .map((record, index) => ({ record, index }))
      .sort((a, b) => rank(a.record) - rank(b.record) || a.index - b.index)
      .map(({ record }) => record);
  };

  /** 给 tree() 用：每条会话带 pinned，返回的数组已排好序。 */
  const decorate = (records, ids) =>
    order(records, ids).map((record) => ({ ...record, pinned: ids.includes(String(record.id)) }));

  return { setPin, decorate };
};
