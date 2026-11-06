// 差异计算：行级对齐 + 改动行内的词级高亮 + 折叠未变区域。
//
// 只做“数和拼”，不碰 DOM —— 渲染在 app.js 那边，方便 resize / 切换视图时重画。
// 库用 vendor/diff（jsdiff 8，逐行对齐和词级 diff 都是它算的；我们只负责配对与折叠）。
import { diffLines, diffWordsWithSpace } from "./vendor/diff/index.js";

const MAX_LINES = 4000; // 超过就退化：只做行级，不算词级（不然卡的是浏览器）

const splitLines = (value) => {
  const lines = String(value ?? "").split("\n");
  if (lines.length && lines[lines.length - 1] === "") lines.pop();
  return lines;
};

/** 行内词级差异：返回 [{text, type: "same"|"add"|"del"}]。 */
const wordDiff = (before, after) => {
  const segs = [];
  for (const part of diffWordsWithSpace(before, after)) {
    const type = part.added ? "add" : part.removed ? "del" : "same";
    // 相邻同类型合并，避免一堆碎片 span。
    const last = segs[segs.length - 1];
    if (last && last.type === type) last.text += part.value;
    else segs.push({ text: part.value, type });
  }
  return segs;
};

/**
 * 把「删除块 + 新增块」按行配对成「修改行」，顺手算行内词级差异。
 * 不设相似度门槛：整行换掉时 segs 会退化成「整行红 + 整行绿」，看起来跟一删一增没区别，
 * 但行数少的时候配对比不配对更好读（行号对得上）。
 */
const pairChanges = (rows, { words }) => {
  const out = [];
  for (let i = 0; i < rows.length; ) {
    if (rows[i].kind !== "del") {
      out.push(rows[i++]);
      continue;
    }
    let j = i;
    while (j < rows.length && rows[j].kind === "del") j++;
    let k = j;
    while (k < rows.length && rows[k].kind === "add") k++;
    const dels = rows.slice(i, j);
    const adds = rows.slice(j, k);
    let paired = 0;
    if (words) {
      const n = Math.min(dels.length, adds.length);
      for (let x = 0; x < n; x++) {
        out.push({
          kind: "mod",
          old: dels[x].old,
          new: adds[x].new,
          text: adds[x].text,
          oldText: dels[x].text,
          segs: wordDiff(dels[x].text, adds[x].text),
        });
        paired++;
      }
    }
    for (let x = paired; x < dels.length; x++) out.push(dels[x]);
    for (let x = paired; x < adds.length; x++) out.push(adds[x]);
    i = k;
  }
  return out;
};

/** 未变区域超过 2*context+1 行就折起来，保留首尾各 context 行。 */
const foldContext = (rows, context) => {
  const out = [];
  for (let i = 0; i < rows.length; ) {
    if (rows[i].kind !== "ctx") {
      out.push(rows[i++]);
      continue;
    }
    let j = i;
    while (j < rows.length && rows[j].kind === "ctx") j++;
    const run = j - i;
    if (run > context * 2 + 1) {
      for (let x = i; x < i + context; x++) out.push(rows[x]);
      out.push({ kind: "fold", count: run - context * 2, from: i, to: j });
      for (let x = j - context; x < j; x++) out.push(rows[x]);
    } else {
      for (let x = i; x < j; x++) out.push(rows[x]);
    }
    i = j;
  }
  return out;
};

/**
 * @returns {{
 *   rows: Array,            // 折叠后的显示行（含 fold）
 *   stats: {adds, dels},    // 与 git 同样的口径：修改行 = +1 -1
 *   marks: Map<number, string>,   // 新行号 → "add" | "mod"（编辑态左侧槽用）
 *   gaps: Map<number, number>,    // 新行号 → 这行之上被删了几行（显示一条红签）
 *   changed: number,        // 有改动的行数（新侧）
 *   coarse: boolean,        // 太大，没算词级
 *   identical: boolean,
 * }}
 */
export function buildDiff(baseText, nextText, { context = 3 } = {}) {
  const base = String(baseText ?? "");
  const next = String(nextText ?? "");
  const lineCount = splitLines(base).length + splitLines(next).length;
  const coarse = lineCount > MAX_LINES;

  let oldNo = 1;
  let newNo = 1;
  const raw = [];
  for (const part of diffLines(base, next)) {
    const lines = splitLines(part.value);
    if (part.removed) for (const text of lines) raw.push({ kind: "del", old: oldNo++, new: null, text });
    else if (part.added) for (const text of lines) raw.push({ kind: "add", old: null, new: newNo++, text });
    else for (const text of lines) raw.push({ kind: "ctx", old: oldNo++, new: newNo++, text });
  }

  const rows = pairChanges(raw, { words: !coarse });

  let adds = 0;
  let dels = 0;
  const marks = new Map();
  for (const row of rows) {
    if (row.kind === "add") {
      adds++;
      marks.set(row.new, "add");
    } else if (row.kind === "mod") {
      adds++;
      dels++;
      marks.set(row.new, "mod");
    } else if (row.kind === "del") {
      dels++;
    }
  }

  // 删除没有「新行号」：记在它下面第一行的上方，免得左侧槽上一片空白看不出删过东西。
  // 文末删除挂到最后一行之后（键 = 总行数 + 1）。
  const gapAtNew = new Map();
  let pending = 0;
  for (const row of rows) {
    if (row.kind === "del") {
      pending++;
    } else if (pending && row.new != null) {
      gapAtNew.set(row.new, pending);
      pending = 0;
    }
  }
  const totalNew = splitLines(next).length;
  if (pending) gapAtNew.set(totalNew + 1, pending);

  return {
    rows: foldContext(rows, context),
    allRows: rows,
    stats: { adds, dels },
    marks,
    gaps: gapAtNew,
    changed: marks.size,
    coarse,
    identical: adds === 0 && dels === 0,
    totalNew,
    from: oldNo - 1,
    to: newNo - 1,
  };
}
