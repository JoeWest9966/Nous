// extend · Nous 扩展清单。全部数据来自 qrd3wh 的 `GET /v1/extensions`。
//
// 块之间只走 HTTP，地址只从宿主的 `__nous/ctx` 来 —— 这里一个 `/p/…` 都不拼。
// 类别不在这一页存：真相是仓库里每个 nous.json 的 category（+ categories.json 词表），
// 这里只往 qrd3wh 发 POST，由它写盘、重扫 index.json，再（debounce 后）commit + push。

import { buildDiff } from "./diff.js";

const SELF = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
const $ = (id) => document.getElementById(id);
const $sel = (sel) => document.querySelector(sel);

async function readJson(res) {
  const body = await res.json().catch(() => null);
  if (!res.ok) {
    const error = new Error(body?.error ?? `HTTP ${res.status}`);
    error.status = res.status;
    throw error;
  }
  return body ?? {};
}

const get = (url) => fetch(url).then(readJson);
const post = (url, body, opts = {}) =>
  fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body ?? {}),
    ...opts,
  }).then(readJson);

let toastTimer;
function toast(msg, kind = "") {
  const node = $("toast");
  node.textContent = msg;
  node.dataset.kind = kind;
  node.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (node.hidden = true), 3200);
}

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const short = (sha) => (sha ? sha.slice(0, 7) : "?");
const categoriesOf = (ext) => (Array.isArray(ext?.category) ? ext.category.filter(Boolean) : []);

// 类别是路径：`工具/搜索`。父子关系不存，靠 `/` 推 —— 跟服务端同一套规矩。
const CAT_SEP = "/";
const ancestorsOf = (name) => {
  const parts = name.split(CAT_SEP);
  return parts.slice(0, -1).map((_, index) => parts.slice(0, index + 1).join(CAT_SEP));
};
const isUnder = (name, root) => name === root || name.startsWith(root + CAT_SEP);
const depthOf = (name) => name.split(CAT_SEP).length - 1;
const leafOf = (name) => name.split(CAT_SEP).at(-1);
const protectedNames = () => (Array.isArray(state.data?.protected) ? state.data.protected : []);
// 删/改名会动到这一枝：内置类别在它里面就算「动不得」。
const isProtected = (path) => protectedNames().some((locked) => isUnder(locked, path));
const compareCats = (a, b) => {
  const left = a.split(CAT_SEP);
  const right = b.split(CAT_SEP);
  const common = Math.min(left.length, right.length);
  for (let i = 0; i < common; i++) {
    const cmp = left[i].localeCompare(right[i], "zh-CN");
    if (cmp) return cmp;
  }
  return left.length - right.length;
};

// 商品砖：字母砖的色相由名字决定，同一个扩展永远同一个颜色。
const hueOf = (name) => {
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) % 360;
  return hash;
};
const initialsOf = (name) => {
  const clean = String(name).replace(/[^a-z0-9]+/gi, " ").trim();
  if (!clean) return "?";
  const parts = clean.split(/\s+/);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return clean.slice(0, 2).toUpperCase();
};

// ── 状态 ───────────────────────────────────────────────────────────────────

const state = {
  base: null,
  data: null,
  names: new Map(),
  filter: "", // "" = 全部；"__none__" = 未分类；其它 = 类别名
  query: "",
  creating: false, // 左栏「新建类别」的输入框开着
  createPrefix: "", // 「新建子类别」预填的 `工具/`
  renaming: null, // 正在原地改名的类别名
  editing: null, // 正在输入类别的扩展名（现在只住在详情里）
  detail: null, // 详情视图里是哪个扩展；null = 列表
  dirty: 0,
  codeDirty: false, // 抽屉里保存过、还没发布 —— 和类别的 dirty 不是一回事，别混
  ahead: 0,
  behind: 0,
  sync: "idle", // idle | pending | pushing | bad
  syncError: "",
  editor: null, // 代码抽屉：{name, files, path, text, base, pending, ...}
  variant: null, // 变体弹层：{ext, item}（item 为空 = 新增）
  duplicate: null, // 复制弹窗：{from}
  pending: null, // 未同步弹窗：pendingState() + 勾选/展开状态
};

const SYNC_DEBOUNCE_MS = 8000;
let syncTimer = null;

// ── 同步 ───────────────────────────────────────────────────────────────────

function renderSync() {
  const chip = $("sync");
  const dot = $sel("#sync .dot");
  const label = chip.querySelector(".sync-label");
  const loadMap = state.data?.load ?? {};
  const failedNames = Object.keys(loadMap).filter((name) => name !== "__reload__" && loadMap[name]?.state === "failed");
  const deletedNames = Object.keys(state.data?.local?.deleted ?? {});
  const unpublishedCount = (state.data?.catalog ?? []).filter(
    (ext) => ext.tracked === false && !deletedNames.includes(ext.name),
  ).length;
  const driftedCount = (state.data?.catalog ?? []).filter(
    (ext) => !failedNames.includes(ext.name) && !deletedNames.includes(ext.name) && sectionDrift(ext),
  ).length;

  let tone = "ok";
  let text = "已同步";
  let title = "改完 8 秒内自动提交并推送";

  if (state.sync === "pushing") {
    tone = "wait";
    text = "推送中…";
  } else if (state.sync === "bad") {
    tone = "bad";
    text = "同步失败";
    title = state.syncError || "点我重试";
  } else if (state.data?.repo?.pullError) {
    tone = "bad";
    text = "拉取失败";
    title = `git pull --ff-only：${state.data.repo.pullError}`;
  } else if (state.behind > 0) {
    tone = "bad";
    text = "远端有新提交";
    title = "origin 比本地新 —— 先拉一下再改，否则 ff-only 会卡住";
  } else if (failedNames.length) {
    // 最容易被骗的一档：代码没生效，但卡片看起来是好的。所以优先于其它状态说。
    tone = "bad";
    text = `${failedNames.length} 个扩展装载失败`;
    title = failedNames.map((name) => `${name}：${state.data.load[name]?.error ?? "未知错误"}`).join("\n");
  } else if (state.data?.load?.__reload__?.state === "failed") {
    tone = "bad";
    text = "扩展重装失败";
    title = state.data.load.__reload__.error ?? "核心扩展没装上，进程里还是旧的那一套";
  } else if (Object.keys(state.data?.local?.deleted ?? {}).length > 0) {
    // 删本地会留下 ` D`（tracked 文件被删的必然结果）。这不是「代码没发布」，
    // 也永远不会由 publish 推上去 —— 顶栏得把这件事原样说出来，别装成同步欠账。
    const gone = Object.keys(state.data.local.deleted).length;
    tone = "wait";
    text = `本地已删 ${gone} 个扩展`;
    title = "只在这台卸载了，仓库没动；灰卡上点「装回本机」可恢复";
  } else if (state.codeDirty && !state.dirty) {
    // 保存 ≠ 发布：代码只落了盘，没 commit。自动推会把半截文件带上 master。
    tone = "wait";
    text = "代码未发布";
    title = "改过的代码还没提交 —— 在代码抽屉里点「发布」";
  } else if (unpublishedCount > 0) {
    // 本地有、HEAD 没有：别人 pull 不到这一个扩展。
    tone = "wait";
    text = `${unpublishedCount} 个扩展未发布`;
    title = "磁盘上有、仓库里没有 —— 换台机器 pull 不到它；点卡片进抽屉发布";
  } else if (driftedCount > 0) {
    tone = "wait";
    text = `${driftedCount} 个扩展清单对不上`;
    title = "nous.json 里写的 sections 跟进程里真装的段落不一致 —— 卡片上有明细";
  } else if (state.dirty > 0) {
    // 两个不同的欠账：工作区没提交 vs 提交了没推。混成一个数字，弹窗再细也没用。
    tone = "wait";
    text = state.ahead > 0 ? `待同步（另有 ${state.ahead} 个提交没推）` : "待提交";
    title = state.ahead > 0 ? "工作区有改动没提交，还有提交没推 —— 点开看" : "工作区有改动没提交 —— 点开看";
  } else if (state.ahead > 0) {
    tone = "wait";
    text = `${state.ahead} 个提交没推`;
    title = "本地提交了，还没 push —— 点开看";
  }

  chip.hidden = !state.data;
  chip.dataset.state = tone;
  dot.dataset.state = tone;
  label.textContent = text;
  chip.title = title;
}

async function flushSync(force = false) {
  if (syncTimer) {
    clearTimeout(syncTimer);
    syncTimer = null;
  }
  if (!state.base) return;
  if (!force && !state.dirty && !state.ahead) return;

  state.dirty = 0;
  state.sync = "pushing";
  renderSync();
  try {
    const result = await post(`${state.base}v1/extensions/sync`, {}, { keepalive: true });
    state.ahead = result.ahead ?? 0;
    state.behind = result.behind ?? 0;
    if (result.pushed === false) {
      state.sync = "bad";
      state.syncError = result.pushError || "git push 失败";
      toast(`推送失败：${state.syncError}`, "bad");
    } else {
      state.sync = "idle";
      state.syncError = "";
      // 索引里有未发布的扩展 —— 那条自动提交没把它们带上（否则 GitHub 会说「我有它」，其实没有）。
      if (result.skippedIndex?.length) {
        toast(`已同步；索引里的 ${result.skippedIndex.join("、")} 还没发布，这次没跟着推`);
      }
    }
  } catch (error) {
    state.sync = "bad";
    state.syncError = String(error?.message ?? error);
    toast(`同步失败：${state.syncError}`, "bad");
  }
  renderSync();
}

function scheduleSync() {
  state.dirty++;
  state.sync = "pending";
  renderSync();
  if (syncTimer) clearTimeout(syncTimer);
  syncTimer = setTimeout(() => void flushSync(), SYNC_DEBOUNCE_MS);
}

function flushOnHide() {
  if (!syncTimer && !state.dirty) return;
  if (syncTimer) {
    clearTimeout(syncTimer);
    syncTimer = null;
  }
  if (!state.base) return;
  state.dirty = 0;
  state.sync = "pushing";
  try {
    void fetch(`${state.base}v1/extensions/sync`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: "{}",
      keepalive: true,
    });
  } catch {
    // 页面都在关了，发不出去也没办法更诚实一点。
  }
}

addEventListener("pagehide", flushOnHide);
addEventListener("beforeunload", flushOnHide);
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") void flushSync();
});

// ── 复制成新扩展 ───────────────────────────────────────────────────────────
//
// 复制的是**目录**，不是历史：新名字、新清单、干净起点，旧提交留在原扩展里。
// 唯一必须机器做对的事是入口里写死的 name —— 不改它，两个扩展注册同一个名字，
// 新的会悄悄顶掉旧的，而页面上两张卡看起来都好好的。所以这件事交给后端做，
// 它找不到那个字面量就拒绝复制（宁可失败，也别造出个会撞名的目录）。

function openDuplicate(from) {
  const ext = oursNow().find((item) => item.name === from);
  if (!ext) return;
  state.duplicate = { from };
  $("dup").reset();
  $("dup-from").textContent = from;
  $("dup-name").value = `${from}-copy`;
  $("dup-summary").value = ext.summary ?? "";
  $("dup-cat").value = categoriesOf(ext).join(", ");
  $("dup-err").hidden = true;
  $("dup-name-note").textContent = "";
  $("dup-name-note").className = "create-note";
  $("dup-where").textContent = `${state.data?.repo?.path ?? "/opt/Nous-extend"}/${from}-copy`;
  const datalist = $("dup-cats");
  datalist.textContent = "";
  for (const name of allCategoryNames(oursNow())) {
    const option = document.createElement("option");
    option.value = name;
    datalist.append(option);
  }
  $("dup-scrim").hidden = false;
  $("dup").hidden = false;
  updateDuplicateOk();
  setTimeout(() => {
    $("dup-name").focus();
    $("dup-name").select();
  }, 30);
}

function closeDuplicate() {
  $("dup").hidden = true;
  $("dup-scrim").hidden = true;
  state.duplicate = null;
}

function updateDuplicateOk() {
  const value = $("dup-name").value.trim();
  const taken = (state.data?.catalog ?? []).some((ext) => ext.name === value);
  const archived = (state.data?.archived ?? []).some((item) => item.name === value);
  const invalid = value && !/^[a-z0-9][a-z0-9._-]*$/i.test(value);
  const note = $("dup-name-note");
  if (invalid) {
    note.textContent = "名字只能用字母、数字、. _ -，且不能以符号开头";
    note.className = "create-note bad";
  } else if (taken) {
    note.textContent = "这个名字已经在清单里了 —— 换个名字";
    note.className = "create-note bad";
  } else if (archived) {
    note.textContent = "归档里有同名目录 —— 那是「恢复」不是「复制」";
    note.className = "create-note warn";
  } else if (value) {
    note.textContent = value === state.duplicate?.from ? "跟本体同名了" : "可以";
    note.className = "create-note " + (value === state.duplicate?.from ? "bad" : "ok");
  } else {
    note.textContent = "";
    note.className = "create-note";
  }
  $("dup-where").textContent = value
    ? `${state.data?.repo?.path ?? "/opt/Nous-extend"}/${value}`
    : "";
  $("dup-ok").disabled = !value || invalid || taken || archived || value === state.duplicate?.from;
}

async function submitDuplicate() {
  const dup = state.duplicate;
  if (!dup) return;
  const name = $("dup-name").value.trim();
  const ok = $("dup-ok");
  ok.disabled = true;
  ok.textContent = "复制中…";
  $("dup-err").hidden = true;
  try {
    const result = await post(`${state.base}v1/extensions/duplicate`, {
      from: dup.from,
      name,
      summary: $("dup-summary").value.trim(),
      category: $("dup-cat").value.trim(),
    });
    closeDuplicate();
    state.query = "";
    state.filter = "";
    await load({ quiet: true });
    const rewritten = result?.rewritten ?? [];
    toast(
      `已复制成 ${name}（未发布）\n${
        rewritten.length ? "入口里写死的名字改成了新名字 —— 抽屉里能看 diff" : "名字本来就不是写死的，代码一个字节没动"
      }`,
    );
  } catch (error) {
    $("dup-err").textContent = String(error?.message ?? error);
    $("dup-err").hidden = false;
  } finally {
    ok.textContent = "复制";
    updateDuplicateOk();
  }
}

$("dup-name").oninput = () => updateDuplicateOk();
$("dup").onsubmit = (event) => {
  event.preventDefault();
  if (!$("dup-ok").disabled) void submitDuplicate();
};
$("dup-cancel").onclick = () => closeDuplicate();
$("dup-scrim").onclick = () => closeDuplicate();

// ── 未同步：按扩展勾选提交 ─────────────────────────────────────────────────
//
// 顶栏那个 chip 以前点一下就把**整个工作区** commit + push —— 一次点击能把正在改的
// 半截代码推上 GitHub。现在它只开这个弹窗：改了什么、属于哪个扩展、勾哪个提交哪个。
// 代码默认不勾（勾了要过 node --check）；本地删除的文件不参与（那是装卸，不是发布）。

const PENDING_FILE_KIND = { code: "代码", manifest: "清单", other: "其它" };
const PENDING_UNIT_TAG = { archive: "归档", categories: "类别词表", other: "仓库文件" };
const isCodePath = (rel) => /\.(mjs|cjs|js)$/i.test(rel);

/** 安全项默认勾上：清单、类别、已发布/未发布扩展的非代码文件。代码和 README 得人自己点。 */
function pendingDefaultSelection(units) {
  const sel = new Set();
  for (const unit of units) {
    for (const file of unit.files) {
      if (file.deleted) continue; // 本地删除：装卸的私事，绝不随提交推
      if (file.kind === "code") continue;
      if (unit.kind === "other") continue;
      sel.add(file.path);
    }
  }
  return sel;
}

async function openPending() {
  if (!state.base) return;
  $("pending-scrim").hidden = false;
  $("pending").hidden = false;
  $("pending-body").textContent = "";
  $("pending-err").hidden = true;
  $("pending-summary").textContent = "读一下工作区…";
  $("pending-msg-wrap").hidden = true;
  $("pending-all").hidden = true;
  $("pending-none").hidden = true;
  const ok = $("pending-ok");
  ok.hidden = true;
  ok.disabled = true;
  $("pending-cancel").textContent = "关闭";
  try {
    const data = await get(`${state.base}v1/extensions/pending`);
    state.pending = {
      ...data,
      sel: pendingDefaultSelection(data.units ?? []),
      openUnits: new Set(),
      openCommits: new Set(),
      diffs: new Map(),
      busy: false,
      error: "",
      done: null,
      repush: false,
    };
    renderPending();
  } catch (error) {
    $("pending-summary").textContent = `读取失败：${error.message}`;
    state.pending = null;
  }
}

function closePending() {
  $("pending").hidden = true;
  $("pending-scrim").hidden = true;
  state.pending = null;
}

const pendingSel = () => state.pending?.sel ?? new Set();

function pendingToggle(paths, on) {
  const sel = state.pending.sel;
  for (const rel of paths) {
    if (on) sel.add(rel);
    else sel.delete(rel);
  }
  renderPending();
}

function updatePendingFooter() {
  const p = state.pending;
  if (!p || p.done) return;
  const sel = [...p.sel];
  const codeCount = sel.filter(isCodePath).length;
  $("pending-stat").textContent = sel.length
    ? `勾中 ${sel.length} 个文件${codeCount ? ` · ${codeCount} 个代码要过语法检查` : ""}`
    : "一个都没勾";
  const ok = $("pending-ok");
  const pushOnly = !p.units.length && p.ahead > 0;
  ok.hidden = false;
  ok.disabled = p.busy || (!pushOnly && !sel.length);
  if (p.repush) ok.textContent = p.busy ? "推送中…" : "重试推送";
  else if (p.busy) ok.textContent = "提交中…";
  else if (pushOnly) ok.textContent = `推送 ${p.ahead} 个提交`;
  else ok.textContent = sel.length ? `提交 ${sel.length} 个文件并推送` : "提交并推送";
}

function pendingUnitEl(unit) {
  const p = state.pending;
  const box = el("div", "pu");
  box.dataset.kind = unit.kind;
  if (unit.code) box.dataset.code = "1";

  const head = el("div", "pu-head");
  const all = unit.files.filter((file) => !file.deleted).map((file) => file.path);
  const picked = all.filter((rel) => p.sel.has(rel)).length;

  const check = el("input", "pu-box");
  check.type = "checkbox";
  check.disabled = !all.length;
  check.checked = all.length > 0 && picked === all.length;
  check.indeterminate = picked > 0 && picked < all.length;
  check.title = unit.kind === "extension" ? `勾上 ${unit.name} 的全部文件` : "勾上这一组";
  check.onchange = () => pendingToggle(all, check.checked);
  head.append(check);

  head.append(el("span", "pu-name", unit.kind === "extension" ? unit.name : PENDING_UNIT_TAG[unit.kind] ?? unit.key));
  if (unit.kind === "extension") {
    head.append(el("span", "pu-tag" + (unit.tracked ? " on" : ""), unit.tracked ? "已发布" : "未发布"));
  } else {
    head.append(el("span", "pu-tag on", PENDING_UNIT_TAG[unit.kind] ?? ""));
  }
  if (unit.hasDeleted) head.append(el("span", "pu-tag bad", "本地已删，不提交"));
  head.append(el("span", "pu-stat", `+${unit.added} −${unit.removed}`));

  const toggle = el("button", "pu-toggle");
  toggle.type = "button";
  toggle.textContent = `${unit.files.length} 个文件`;
  toggle.onclick = () => {
    if (p.openUnits.has(unit.key)) p.openUnits.delete(unit.key);
    else p.openUnits.add(unit.key);
    renderPending();
  };
  head.append(toggle);
  box.append(head);

  if (p.openUnits.has(unit.key)) {
    const files = el("div", "pu-files");
    for (const file of unit.files) files.append(pendingFileEl(file));
    box.append(files);
  }
  return box;
}

function pendingFileEl(file) {
  const p = state.pending;
  const row = el("div", "pu-file");
  const main = el("label", "pu-file-main");
  const check = el("input", "pu-box");
  check.type = "checkbox";
  check.checked = p.sel.has(file.path);
  check.disabled = file.deleted;
  check.onchange = () => pendingToggle([file.path], check.checked);
  main.append(check);
  main.append(el("span", "pu-path", file.path.slice(file.path.indexOf("/") + 1) || file.path));
  main.append(el("span", "pu-kind", PENDING_FILE_KIND[file.kind] ?? ""));
  row.dataset.kind = file.kind;
  if (file.deleted) row.dataset.deleted = "1";
  main.append(el("span", "pu-stat", `+${file.added} −${file.removed}`));
  row.append(main);

  const diffBtn = el("button", "pu-file-diff");
  diffBtn.type = "button";
  diffBtn.textContent = "看 diff";
  row.append(diffBtn);

  const box = el("div", "pu-diff");
  box.hidden = true;
  diffBtn.onclick = async () => {
    if (!box.hidden) {
      box.hidden = true;
      diffBtn.textContent = "看 diff";
      return;
    }
    box.hidden = false;
    diffBtn.textContent = "收起";
    if (box.childNodes.length) return;
    box.append(el("p", "pu-diff-note dim", "读…"));
    try {
      let data = p.diffs.get(file.path);
      if (!data) {
        data = await get(`${state.base}v1/extensions/pending/diff?path=${encodeURIComponent(file.path)}`);
        p.diffs.set(file.path, data);
      }
      box.textContent = "";
      box.append(pendingDiffEl(data, file));
    } catch (error) {
      box.textContent = "";
      box.append(el("p", "pu-diff-note bad", `读不到：${error.message}`));
    }
  };

  const wrap = el("div", "pu-file-wrap");
  wrap.append(row);
  wrap.append(box);
  return wrap;
}

function pendingDiffEl(data, file) {
  const wrap = el("div", "pu-diff-body");
  if (file.deleted) {
    wrap.append(el("p", "pu-diff-note dim", "这个文件只在这台被删了 —— 它不会随提交推上去。"));
    return wrap;
  }
  const diff = buildDiff(data.base ?? "", data.text ?? "", { context: 3 });
  const head = el("div", "pu-diff-head");
  head.append(el("span", "dim small", data.base ? "与 HEAD 比" : "还没进仓库（整份是新增）"));
  head.append(el("span", "pu-stat", `+${diff.stats.adds} −${diff.stats.dels}`));
  wrap.append(head);
  if (diff.identical) {
    wrap.append(el("p", "pu-diff-note dim", "与 HEAD 一致。"));
    return wrap;
  }
  const body = el("div", "pu-diff-rows");
  for (const row of diff.rows) {
    if (row.kind === "fold") {
      body.append(el("div", "dl-fold", `⋯ 省略 ${row.count} 行未变内容`));
      continue;
    }
    for (const node of diffRowEls(row)) body.append(node);
  }
  wrap.append(body);
  return wrap;
}

function pendingAheadEl() {
  const p = state.pending;
  if (!p.aheadCommits.length) return null;
  const box = el("div", "pu-group");
  const head = el("div", "pu-group-head");
  head.append(el("span", "", "已提交，没推"));
  head.append(el("span", "pu-group-n", String(p.aheadCommits.length)));
  box.append(head);
  for (const commit of p.aheadCommits) {
    const row = el("div", "pu-commit");
    const main = el("button", "pu-commit-main");
    main.type = "button";
    main.append(el("span", "mono pu-hash", commit.hash));
    main.append(el("span", "pu-subj", commit.subject));
    main.onclick = () => {
      if (p.openCommits.has(commit.hash)) p.openCommits.delete(commit.hash);
      else p.openCommits.add(commit.hash);
      renderPending();
    };
    row.append(main);
    if (p.openCommits.has(commit.hash)) {
      const detail = el("div", "pu-commit-files");
      detail.append(el("p", "pu-diff-note dim", "读…"));
      row.append(detail);
      void get(`${state.base}v1/extensions/pending/commit?hash=${commit.hash}`)
        .then((data) => {
          detail.textContent = "";
          for (const file of data.files ?? []) {
            const line = el("div", "pu-commit-file");
            line.append(el("span", "pu-path mono", file.path));
            line.append(el("span", "pu-stat", `+${file.added} −${file.removed}`));
            detail.append(line);
          }
          if (!(data.files ?? []).length) detail.append(el("p", "pu-diff-note dim", "空提交"));
        })
        .catch((error) => {
          detail.textContent = "";
          detail.append(el("p", "pu-diff-note bad", `读不到：${error.message}`));
        });
    }
    box.append(row);
  }
  return box;
}

function renderPendingDone(p) {
  const done = p.done;
  const body = $("pending-body");
  $("pending-summary").textContent = "";
  $("pending-msg-wrap").hidden = true;
  $("pending-all").hidden = true;
  $("pending-none").hidden = true;
  $("pending-stat").textContent = "";
  const ok = $("pending-ok");
  ok.hidden = true;
  ok.disabled = true;

  const box = el("div", "pu-done");
  const line = el("p", "pu-done-head");
  if (done.committed) {
    line.append(el("b", "", `已提交 ${short(done.hash)}`));
    line.append(el("span", "dim", ` · ${(done.files ?? []).length} 个文件`));
  } else {
    line.append(el("b", "", "没有新提交"));
  }
  line.append(el("span", done.pushed === false ? "bad" : "dim", done.pushed === false ? " · 没推上去" : " · 已推送"));
  box.append(line);

  if (p.error) box.append(el("p", "pu-done-err", p.error));

  const actions = el("div", "pu-done-actions");
  if (done.committed && /^https?:\/\//.test(p.remote ?? "")) {
    const link = el("a", "btn", "看这条提交 ›");
    link.href = `${p.remote}/commit/${done.hash}`;
    link.target = "_blank";
    link.rel = "noreferrer";
    actions.append(link);
  }
  if ((done.files ?? []).some(isCodePath)) {
    // 提交 ≠ 生效：跑在进程里的还是旧那一版。要不要重装，人自己说。
    const reload = el("button", "btn", "重装（让改动生效）");
    reload.type = "button";
    reload.onclick = async () => {
      reload.disabled = true;
      reload.textContent = "重装中…";
      try {
        const result = await post(`${state.base}v1/extensions/reload`, {});
        const failed = Object.entries(result?.load ?? {}).filter(([, item]) => item?.state === "failed");
        if (failed.length) toast(`重装完成，但 ${failed.length} 个扩展没装上：${failed.map(([name]) => name).join("、")}`, "bad");
        else toast("已重装 —— 进程里跑的就是这一版");
        await load({ quiet: true });
      } catch (error) {
        toast(`重装失败：${error.message}`, "bad");
      }
      reload.disabled = false;
      reload.textContent = "重装（让改动生效）";
    };
    actions.append(reload);
  }
  box.append(actions);
  body.append(box);
}

function renderPending() {
  const p = state.pending;
  if (!p) return;
  $("pending-body").textContent = "";
  if (p.done) return renderPendingDone(p);

  const body = $("pending-body");
  const files = p.units.reduce((sum, unit) => sum + unit.files.length, 0);
  const bits = [];
  if (files) bits.push(`工作区 ${files} 个文件有改动`);
  if (p.ahead) bits.push(`${p.ahead} 个提交没推`);
  $("pending-summary").textContent = bits.length ? bits.join(" · ") : "工作区干净";
  $("pending-msg-wrap").hidden = !p.units.length;
  $("pending-all").hidden = !p.units.length;
  $("pending-none").hidden = !p.units.length;
  $("pending-err").hidden = !p.error;
  if (p.error) $("pending-err").textContent = p.error;

  const ahead = pendingAheadEl();
  if (ahead) body.append(ahead);

  if (!p.units.length) {
    if (!p.aheadCommits.length) body.append(el("p", "pu-empty dim", "工作区干净，也没有没推的提交。"));
  } else {
    // 分三段：首次进仓库的扩展 / 改已有扩展 / 词表与其它。「新扩展首发」和
    // 「改一行」是不同的决定，别让人在同一列里靠药丸分辨。
    const groups = [
      { title: "未发布的扩展（首次进仓库）", units: p.units.filter((unit) => unit.kind === "extension" && !unit.tracked) },
      { title: "已发布的扩展", units: p.units.filter((unit) => unit.kind === "extension" && unit.tracked) },
      { title: "类别与仓库文件", units: p.units.filter((unit) => unit.kind !== "extension") },
    ].filter((group) => group.units.length);
    for (const group of groups) {
      const wrap = el("div", "pu-section");
      if (groups.length > 1) wrap.append(el("div", "pu-group-head", group.title));
      const list = el("div", "pu-list");
      for (const unit of group.units) list.append(pendingUnitEl(unit));
      wrap.append(list);
      body.append(wrap);
    }
    body.append(
      el("p", "pu-note dim", "只提交勾中的文件。代码默认不勾 —— 勾了要先过语法检查。"),
    );
  }
  updatePendingFooter();
}

async function submitPending() {
  const p = state.pending;
  if (!p || p.busy) return;
  if (!p.repush && !p.units.length && !p.ahead) return;
  const message = $("pending-message").value.trim();
  const paths = p.repush ? [] : [...p.sel];
  if (!p.repush && p.units.length && !paths.length) return;
  p.busy = true;
  p.error = "";
  updatePendingFooter();
  try {
    const result = await post(`${state.base}v1/extensions/sync`, { paths, message });
    if (result.pushed === false) {
      // 提交已经发生、推送没成：勾选清空，再点就是纯推送，别重复提交。
      p.busy = false;
      p.repush = true;
      p.sel = new Set();
      p.units = [];
      p.done = result.committed ? result : null;
      p.error = result.committed
        ? `本地提交成功（${short(result.hash)}），但推送失败：${result.pushError || "原因不明"}`
        : `推送失败：${result.pushError || "原因不明"}`;
      renderPending();
      return;
    }
    p.busy = false;
    p.done = result;
    await load({ quiet: true });
  } catch (error) {
    p.busy = false;
    p.error = String(error?.message ?? error);
  }
  renderPending();
}

// ── 数据查询 ───────────────────────────────────────────────────────────────

function oursNow() {
  const catalog = state.data?.catalog ?? [];
  const builtin = new Set(state.data?.builtin ?? []);
  return catalog.filter((ext) => !builtin.has(ext.name));
}

function allCategoryNames(ours) {
  const set = new Set();
  const add = (value) => {
    const name = String(value ?? "").trim();
    if (!name) return;
    for (const parent of ancestorsOf(name)) set.add(parent);
    set.add(name);
  };
  for (const name of Array.isArray(state.data?.categories) ? state.data.categories : []) add(name);
  for (const ext of ours) for (const tag of categoriesOf(ext)) add(tag);
  return [...set].sort(compareCats);
}

function categoryCounts(ours) {
  const counts = new Map();
  let none = 0;
  for (const ext of ours) {
    const tags = categoriesOf(ext);
    if (!tags.length) none++;
    // 父级计数含后代；同一个扩展同时挂父和子只算一次。
    const applied = new Set();
    for (const tag of tags) {
      for (const parent of ancestorsOf(tag)) applied.add(parent);
      applied.add(tag);
    }
    for (const name of applied) counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  return { counts, none };
}

// ── 类别操作 ───────────────────────────────────────────────────────────────

async function saveCategories(name, categories) {
  const ext = oursNow().find((item) => item.name === name);
  if (!ext || !state.base) return;

  const before = categoriesOf(ext).slice();
  const next = [...new Set(categories)].sort();
  ext.category = next;
  render();

  try {
    const result = await post(`${state.base}v1/extensions/category`, { name, category: next });
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    render();
    scheduleSync();
  } catch (error) {
    toast(`改类别失败：${error.message}`, "bad");
    ext.category = before;
    render();
    void load({ quiet: true });
  }
}

function addTag(name, value) {
  const tag = String(value ?? "").trim();
  if (!tag) return;
  const ext = oursNow().find((item) => item.name === name);
  if (!ext) return;
  const set = new Set(categoriesOf(ext));
  if (set.has(tag)) return;
  set.add(tag);
  void saveCategories(name, [...set]);
}

function removeTag(name, value) {
  const ext = oursNow().find((item) => item.name === name);
  if (!ext) return;
  void saveCategories(
    name,
    categoriesOf(ext).filter((tag) => tag !== value),
  );
}

async function addCategory(value) {
  const name = String(value ?? "").trim();
  if (!name || !state.base) return;
  try {
    const result = await post(`${state.base}v1/extensions/categories`, { op: "add", name });
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    render();
    scheduleSync();
  } catch (error) {
    toast(`新建类别失败：${error.message}`, "bad");
  }
}

async function removeCategory(name) {
  if (!state.base) return;
  const names = allCategoryNames(oursNow());
  const subs = names.filter((candidate) => candidate !== name && isUnder(candidate, name)).length;
  const count = categoryCounts(oursNow()).counts.get(name) ?? 0;
  const scope = `类别「${name}」${subs ? `连同 ${subs} 个子类别` : ""}`;
  const message = count ? `删除${scope}？会从 ${count} 个扩展上摘掉。` : `删除${scope}？（空类别）`;
  if (!confirm(message)) return;
  try {
    const result = await post(`${state.base}v1/extensions/categories`, { op: "remove", name });
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    if (state.filter === name) state.filter = "";
    render();
    scheduleSync();
  } catch (error) {
    toast(`删除类别失败：${error.message}`, "bad");
  }
}

// ── 右键 / 长按菜单 ────────────────────────────────────────────────────────
//
// 左栏类别行的菜单：行尾 ⋯ 是主入口，桌面右键 / 手机长按 / 键盘 ContextMenu 是快捷方式。
// 菜单是个空壳 div，条目现拼 —— 以后加「重命名」就多 push 一项，别处不用动。

const LONG_PRESS_MS = 520;
const LONG_PRESS_SLOP = 10;

let menuOpen = false;
let menuTeardown = null;
let swallowTimer = null;

function countOf(name) {
  return categoryCounts(oursNow()).counts.get(name) ?? 0;
}

function closeMenu() {
  if (!menuOpen) return;
  menuOpen = false;
  const menu = $("ctxmenu");
  menu.hidden = true;
  menu.textContent = "";
  if (menuTeardown) {
    menuTeardown();
    menuTeardown = null;
  }
}

function openCategoryMenu(x, y, name) {
  closeMenu();

  const menu = $("ctxmenu");
  menu.textContent = "";
  menu.hidden = false;
  menuOpen = true;

  const locked = isProtected(name);
  const head = el("div", "ctx-head");
  head.append(el("span", "ctx-head-name", name));
  if (locked) head.append(el("span", "ctx-badge", "内置"));
  menu.append(head);

  const createSub = el("button", "ctx-item");
  createSub.type = "button";
  createSub.setAttribute("role", "menuitem");
  createSub.append(el("span", "ctx-item-label", "新建子类别"));
  createSub.append(el("span", "ctx-item-hint", "在它下面"));
  createSub.onclick = () => {
    closeMenu();
    state.creating = true;
    state.createPrefix = name + CAT_SEP;
    render();
  };
  menu.append(createSub);

  // 内置类别像「全部」：没有改名、没有删除。往里挂扩展、建子类别照旧。
  if (!locked) {
    const rename = el("button", "ctx-item");
    rename.type = "button";
    rename.setAttribute("role", "menuitem");
    rename.append(el("span", "ctx-item-label", "编辑类别名称"));
    rename.append(el("span", "ctx-item-hint", "含子类别"));
    rename.onclick = () => {
      closeMenu();
      state.renaming = name;
      render();
    };
    menu.append(rename);

    menu.append(el("div", "ctx-sep"));

    const remove = el("button", "ctx-item ctx-danger");
    remove.type = "button";
    remove.setAttribute("role", "menuitem");
    remove.append(el("span", "ctx-item-label", "删除类别"));
    remove.append(el("span", "ctx-item-hint", countOf(name) ? `${countOf(name)} 个扩展` : "空类别"));
    remove.onclick = () => {
      closeMenu();
      void removeCategory(name, countOf(name));
    };
    menu.append(remove);
  }

  mountMenu(menu, x, y, createSub);
}

// 菜单件的公共骨架：贴边夹紧、点外面 / Esc / 滚动 / 改窗口大小都关。
// 左栏类别行和商品卡共用这一份 —— 两套各自算位置的代码必然一套对一套错。
function mountMenu(menu, x, y, focusTarget) {
  // 先贴左上角量一遍，再摆到点击处 —— 右键点在屏幕边上不能被切掉。
  menu.style.left = "0px";
  menu.style.top = "0px";
  const rect = menu.getBoundingClientRect();
  const pad = 8;
  const left = Math.max(pad, Math.min(x, innerWidth - rect.width - pad));
  const top = Math.max(pad, Math.min(y, innerHeight - rect.height - pad));
  menu.style.left = `${Math.round(left)}px`;
  menu.style.top = `${Math.round(top)}px`;

  focusTarget?.focus({ preventScroll: true });

  // 下一拍才挂「点外面关掉」：这一下 pointerdown 还在派发，挂早了会被自己关掉。
  let armed = false;
  const arm = setTimeout(() => (armed = true), 0);
  const onPointerDown = (event) => {
    if (armed && !menu.contains(event.target)) closeMenu();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      closeMenu();
    }
  };
  const onViewport = () => closeMenu();

  addEventListener("pointerdown", onPointerDown, true);
  addEventListener("keydown", onKey);
  addEventListener("scroll", onViewport, true);
  addEventListener("resize", onViewport);
  menuTeardown = () => {
    clearTimeout(arm);
    removeEventListener("pointerdown", onPointerDown, true);
    removeEventListener("keydown", onKey);
    removeEventListener("scroll", onViewport, true);
    removeEventListener("resize", onViewport);
  };
}

// 商品卡上的动作菜单。跟左栏类别行同一套件，只是挂在卡片上。
// 排序按「读 → 写 → 销毁」：写跟销毁之间一定有分隔线，别让手滑落在隔壁。
function openCardMenu(x, y, ext) {
  closeMenu();

  const menu = $("ctxmenu");
  menu.textContent = "";
  menu.hidden = false;
  menuOpen = true;

  const status = statusOf(ext);
  const ghost = status === "ghost";
  const core = isCore(ext.name);
  const view = STATUS_VIEWS.find((item) => item.key === status);

  const head = el("div", "ctx-head");
  head.append(el("span", "ctx-head-name", ext.name));
  head.append(el("span", "ctx-badge", status === "installed" ? `v${ext.version}` : (view?.label ?? `v${ext.version}`)));
  menu.append(head);

  const item = (label, hint, run, { danger = false, disabled = false, title = "" } = {}) => {
    const button = el("button", "ctx-item" + (danger ? " ctx-danger" : ""));
    button.type = "button";
    button.setAttribute("role", "menuitem");
    button.append(el("span", "ctx-item-label", label));
    if (hint) button.append(el("span", "ctx-item-hint", hint));
    button.disabled = disabled;
    if (title) button.title = title;
    button.onclick = () => {
      if (disabled) return;
      closeMenu();
      run?.();
    };
    menu.append(button);
    return button;
  };

  const first = item("查看详情", "类别 · 变体", () => openDetail(ext.name));
  item("看代码", ghost ? "先装回本机" : "读 / 改", () => void openExtensionEditor(ext), {
    disabled: ghost,
    title: ghost ? `${ext.name} 已从本机卸载，源码不在工作区里` : "",
  });
  if (status === "failed") item("重试装载", "立刻重来", () => void retryLoad(ext.name));
  item("复制成新扩展", "换名字，历史不带", () => openDuplicate(ext.name), {
    disabled: ghost,
    title: ghost ? `${ext.name} 已从本机卸载，源码不在工作区里，没得拷` : "",
  });

  menu.append(el("div", "ctx-sep"));
  item("类别", "在详情里勾", () => openDetail(ext.name, { editTags: true }));

  if (ghost) {
    menu.append(el("div", "ctx-sep"));
    item("装回本机", "仓库不动", () => void installExtension(ext.name));
    mountMenu(menu, x, y, first);
    return;
  }

  menu.append(el("div", "ctx-sep"));
  item("卸载本机", "仓库不动", () => void uninstallExtension(ext.name), {
    disabled: core,
    title: core ? `${ext.name} 是引擎启动路径上硬编码 import 的核心扩展，卸了下次起不来` : "",
  });
  const unpublished = ext.tracked === false;
  item(unpublished ? "删掉" : "归档", unpublished ? "没有历史可找" : "可从归档恢复", () => void archiveExtension(ext.name, { unpublished }), {
    danger: true,
    disabled: core,
    title: core ? `${ext.name} 是核心扩展，归档会让引擎起不来` : "",
  });

  mountMenu(menu, x, y, first);
}

// 长按（手机）/ 右键（桌面）都走同一个回调：卡片和左栏类别行用的是同一套手势。
function attachMenuGesture(node, open) {
  let timer = null;
  let startX = 0;
  let startY = 0;

  const cancel = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  node.addEventListener("pointerdown", (event) => {
    if (event.pointerType === "mouse") return; // 桌面走 contextmenu
    if (event.button !== 0) return;
    cancel();
    startX = event.clientX;
    startY = event.clientY;
    timer = setTimeout(() => {
      timer = null;
      open(startX, startY);
      swallowNextClick();
    }, LONG_PRESS_MS);
  });

  // 手指一挪就当滚动，别在滑动列表时蹦出菜单。
  node.addEventListener("pointermove", (event) => {
    if (timer === null) return;
    if (Math.abs(event.clientX - startX) > LONG_PRESS_SLOP || Math.abs(event.clientY - startY) > LONG_PRESS_SLOP) {
      cancel();
    }
  });

  for (const type of ["pointerup", "pointercancel", "pointerleave"]) node.addEventListener(type, cancel);
}

// 长按松手时浏览器还会补一个 click；落在刚弹出的菜单上就是误触。吞掉这一下。
function swallowNextClick() {
  const kill = (event) => {
    event.preventDefault();
    event.stopPropagation();
    drop();
  };
  const drop = () => {
    removeEventListener("click", kill, true);
    clearTimeout(swallowTimer);
  };
  addEventListener("click", kill, true);
  clearTimeout(swallowTimer);
  swallowTimer = setTimeout(drop, 1200);
}

async function renameCategory(name, next) {
  if (!state.base) return;
  try {
    const result = await post(`${state.base}v1/extensions/categories`, { op: "rename", name, to: next });
    if (Array.isArray(result.categories)) state.data.categories = result.categories;
    if (Array.isArray(result.catalog)) state.data.catalog = result.catalog;
    if (state.filter === name) state.filter = next;
    else if (state.filter.startsWith(name + CAT_SEP)) state.filter = next + state.filter.slice(name.length);
    render();
    scheduleSync();
  } catch (error) {
    toast(`改名失败：${error.message}`, "bad");
  }
}

// ── 卡片上的类别编辑 ───────────────────────────────────────────────────────

function tagRow(ext, { readOnly = false, className = "product-tags" } = {}) {
  const wrap = el("div", "tags " + className);

  for (const tag of categoriesOf(ext)) {
    const chip = el("span", "tag");
    const nameEl = el("span", "tag-name");
    const cut = tag.lastIndexOf(CAT_SEP);
    if (cut > 0) {
      nameEl.append(el("span", "tag-prefix", tag.slice(0, cut + 1)));
      nameEl.append(tag.slice(cut + 1));
    } else {
      nameEl.append(tag);
    }
    chip.append(nameEl);
    if (!readOnly) {
      const remove = el("button", "tag-x", "×");
      remove.type = "button";
      remove.title = `从 ${ext.name} 摘掉「${tag}」`;
      remove.onclick = () => removeTag(ext.name, tag);
      chip.append(remove);
    } else {
      chip.classList.add("tag-ro");
    }
    wrap.append(chip);
  }

  if (readOnly) return wrap;

  if (state.editing === ext.name) {
    const input = el("input", "tag-input");
    input.type = "text";
    input.placeholder = "类别，回车";
    input.setAttribute("list", "tag-suggestions");
    let done = false;
    const commit = () => {
      if (done) return;
      done = true;
      const value = input.value;
      state.editing = null;
      render();
      if (value.trim()) addTag(ext.name, value);
    };
    input.onkeydown = (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        done = true;
        state.editing = null;
        render();
      }
    };
    input.onblur = commit;
    wrap.append(input);
  } else {
    const add = el("button", "tag-add", "+ 类别");
    add.type = "button";
    add.onclick = () => {
      state.editing = ext.name;
      render();
    };
    wrap.append(add);
  }

  return wrap;
}

// ── 本地装卸 ───────────────────────────────────────────────────────────────
//
// 只有 qrd3wh 知道本机删了谁（extensions.local.json 在它的 data/ 里）。这一页只是
// 把那份名单显示成灰卡，动作全部走 HTTP，自己不碰文件系统，也不 commit/push。

const localDeleted = (name) => Boolean(state.data?.local?.deleted?.[name]);
const isCore = (name) => Boolean(state.data?.local?.core?.includes(name));

// 状态只有一种优先级：灰卡 > 装载失败 > 未发布 > 未装入 > 已装。卡片和筛选条共用这一份。
const statusOf = (ext) => {
  if (localDeleted(ext.name)) return "ghost";
  if (state.data?.load?.[ext.name]?.state === "failed") return "failed";
  if (ext.tracked === false) return "unpublished";
  if (!state.data?.installed?.includes(ext.name)) return "absent";
  return "installed";
};
const statusValueOf = (ext) => `__${statusOf(ext)}__`;
const STATUS_VIEWS = [
  { key: "installed", value: "__installed__", label: "已装", tone: "ok", title: "进程里已经装着的扩展" },
  { key: "absent", value: "__absent__", label: "未装入", tone: "bad", title: "仓库里有，但这个进程里没装上" },
  { key: "ghost", value: "__ghost__", label: "仅仓库", tone: "gone", title: "本机卸载了，仓库里还在" },
  { key: "unpublished", value: "__unpublished__", label: "未发布", tone: "warn", title: "磁盘上有，HEAD 里没有" },
  { key: "failed", value: "__failed__", label: "装载失败", tone: "bad", title: "代码没生效，进程里还是旧版本" },
];
const isStatusFilter = (value) => STATUS_VIEWS.some((view) => view.value === value);

async function uninstallExtension(name) {
  closeMenu();
  try {
    await post(`${state.base}v1/extensions/uninstall`, { name });
    toast(`已从本机卸载 ${name} · 灰卡上点「装回本机」可恢复`);
    await load({ quiet: true });
  } catch (error) {
    toast(`卸载失败：${error.message}`, "bad");
  }
}

async function retryLoad(name) {
  try {
    const result = await post(`${state.base}v1/extensions/reload`, {});
    await load({ quiet: true });
    const entry = result.load?.[name];
    if (entry?.state === "failed") toast(`${name} 仍然装载失败：${entry.error}`, "bad");
    else if (result.pullError) toast(`已重装，但 git pull 失败：${result.pullError}`, "bad");
    else toast(`${name} 已重新装载`);
  } catch (error) {
    await load({ quiet: true });
    toast(`重装失败：${error.message}`, "bad");
  }
}

async function installExtension(name) {
  closeMenu();
  try {
    await post(`${state.base}v1/extensions/install`, { name });
    toast(`${name} 已装回本机`);
    await load({ quiet: true });
  } catch (error) {
    toast(`装回本机失败：${error.message}`, "bad");
  }
}

// 破坏性且会推上 GitHub 的动作：要求手输扩展名。卸载本机不要确认 —— 灰卡一点就回来。
function confirmName({ title, hint, name, label = "确定", onConfirm }) {
  const modal = $("danger");
  const input = $("danger-input");
  const ok = $("danger-ok");
  $("danger-title").textContent = title;
  $("danger-hint").textContent = hint;
  $("danger-name").textContent = name;
  $("danger-ok").textContent = label;
  $("danger-err").hidden = true;
  input.value = "";
  ok.disabled = true;
  modal.hidden = false;
  $("danger-scrim").hidden = false;
  setTimeout(() => input.focus(), 30);

  const cleanup = () => {
    modal.hidden = true;
    $("danger-scrim").hidden = true;
    input.oninput = null;
    ok.onclick = null;
    $("danger-cancel").onclick = null;
    $("danger-scrim").onclick = null;
    document.removeEventListener("keydown", onKey, true);
  };
  const go = async () => {
    if (ok.disabled) return;
    cleanup();
    await onConfirm();
  };
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      cleanup();
    } else if (event.key === "Enter" && !ok.disabled) {
      event.preventDefault();
      void go();
    }
  };
  input.oninput = () => {
    ok.disabled = input.value.trim() !== name;
  };
  ok.onclick = () => void go();
  $("danger-cancel").onclick = cleanup;
  $("danger-scrim").onclick = cleanup;
  document.addEventListener("keydown", onKey, true);
}

async function archiveExtension(name, { unpublished = false } = {}) {
  confirmName({
    // 未发布的扩展没有历史可留 —— 归档它会是「先把半成品推上去再归档」，所以后端直接删掉。
    title: unpublished ? `删掉 ${name}？` : `从仓库里归档 ${name}？`,
    hint: unpublished
      ? "它还没发布过，仓库里从来就没有它 —— 所以没有备份、没有历史可找。这一步就是把这台磁盘上的目录删掉。"
      : "源码会移到仓库的 .archive/ 并提交 + 推送。GitHub 上不再有它，但备份还在仓库里，随时可以恢复。",
    name,
    label: unpublished ? "删掉" : "归档并推送",
    onConfirm: async () => {
      try {
        const result = await post(`${state.base}v1/extensions/archive`, { name });
        if (result.dropped) {
          toast(`已删掉 ${name}（未发布过，仓库里本来就没有）`);
          await load({ quiet: true });
          return;
        }
        if (result.pushed === false) {
          toast(`已在本地提交，但推送失败：${result.pushError || "未知原因"}`, "bad");
        } else {
          toast(`已归档 ${name} · 左栏「已归档」里可以恢复`);
        }
        await load({ quiet: true });
      } catch (error) {
        toast(`归档失败：${error.message}`, "bad");
      }
    },
  });
}

async function restoreArchived(name) {
  try {
    const result = await post(`${state.base}v1/extensions/restore`, { name });
    if (result.pushed === false) toast(`已恢复，但推送失败：${result.pushError || "未知原因"}`, "bad");
    else toast(`${name} 已从归档恢复`);
    await load({ quiet: true });
  } catch (error) {
    toast(`恢复失败：${error.message}`, "bad");
  }
}

async function purgeArchived(name) {
  confirmName({
    title: `彻底删除 ${name}？`,
    hint: "连仓库里的备份一起删掉，并提交 + 推送。这一步之后只能靠 git 历史找回来。",
    name,
    label: "彻底删除",
    onConfirm: async () => {
      try {
        const result = await post(`${state.base}v1/extensions/purge`, { name });
        if (result.pushed === false) toast(`已提交，但推送失败：${result.pushError || "未知原因"}`, "bad");
        else toast(`${name} 已彻底删除`);
        await load({ quiet: true });
      } catch (error) {
        toast(`彻底删除失败：${error.message}`, "bad");
      }
    },
  });
}

// ── 商品卡 ─────────────────────────────────────────────────────────────────

// 商品卡：只放身份（色砖 / 名字 / 版本 / 状态 / 说明 / 三个数）。
// 类别、变体、动作全搬走了：点一下进详情，右键（或 ⋯）出动作菜单。
// 状态角标一个都不许搬走 —— 扫一眼看谁没装、谁红了，是这页存在的理由。
function productCard(ext) {
  const status = statusOf(ext);
  const ghost = status === "ghost";
  const installed = status === "installed";
  const failed = status === "failed";
  const unpublished = status === "unpublished"; // 只在本地，HEAD 里没有
  const card = el(
    "article",
    "product" + (status === "installed" ? "" : status === "failed" ? " load-failed" : ` ${status}`),
  );

  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.dataset.name = ext.name;
  const openMenuAt = (mx, my) => openCardMenu(mx, my, ext);

  card.onclick = () => openDetail(ext.name);
  card.oncontextmenu = (event) => {
    event.preventDefault();
    openMenuAt(event.clientX, event.clientY);
  };
  card.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      openDetail(ext.name);
    } else if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
      event.preventDefault();
      const rect = card.getBoundingClientRect();
      openMenuAt(rect.left + 8, rect.bottom - 6);
    }
  };
  attachMenuGesture(card, openMenuAt);

  const visual = el("div", "visual");
  visual.style.setProperty("--hue", String(hueOf(ext.name)));
  visual.append(el("span", "visual-mono", initialsOf(ext.name)));
  if (ghost) visual.append(el("span", "badge-float gone", "仅在仓库"));
  else if (failed) visual.append(el("span", "badge-float fail", "装载失败"));
  else if (unpublished) visual.append(el("span", "badge-float unpub", "未发布"));
  else if (!installed) visual.append(el("span", "badge-float bad", "未装入"));
  else if (state.data.defaults?.includes(ext.name)) visual.append(el("span", "badge-float good", "默认"));
  card.append(visual);

  const body = el("div", "product-body");
  const title = el("div", "product-title");
  title.append(el("span", "product-name", ext.name));
  title.append(el("span", "product-ver", "v" + ext.version));

  // 卡上唯一可见的菜单入口：桌面 hover 出现、触屏常驻。
  // 右键 / 长按只是快捷方式 —— 只藏右键里，第一次进来的人永远找不到。
  const menuBtn = el("button", "product-menu", "⋯");
  menuBtn.type = "button";
  menuBtn.title = `${ext.name} 的菜单：代码 / 类别 / 卸载 / 归档`;
  menuBtn.setAttribute("aria-label", `${ext.name} 的菜单`);
  menuBtn.onclick = (event) => {
    event.stopPropagation();
    const rect = menuBtn.getBoundingClientRect();
    openMenuAt(rect.left, rect.bottom + 4);
  };
  menuBtn.onkeydown = (event) => {
    if (event.key === "Enter" || event.key === " ") event.stopPropagation();
  };
  title.append(menuBtn);
  body.append(title);

  if (ext.summary) body.append(el("p", "product-sum", ext.summary));

  // 装载失败的错误摘要在详情里有全文；卡上留两行，是因为「坏了」和「为什么坏」不是一个问题。
  const load = state.data.load?.[ext.name];
  if (failed && load?.error) {
    const error = el("p", "product-err", load.error);
    error.title = load.error;
    body.append(error);
  }

  // 清单对账：只有「装上了」才比 —— 装载失败那一档已经在上面报了，别再叠一层。
  const drift = ghost || failed ? null : sectionDrift(ext);
  if (drift) {
    const warn = el("p", "product-drift", drift.text);
    body.append(warn);
    card.classList.add("drifted");
  }
  card.append(body);

  card.append(productFoot(ext));
  return card;
}

// ── 变体：一张卡里的多个可装载名字 ─────────────────────────────────────────
//
// 进程里变体是**另一个注册名**（`nous@review`），界面上它是本体**详情里**的一行 ——
// 卡片按本体分组，变体不另开卡。行上必须一眼看出代码从哪儿来，因为两类来源语义完全不同：
//   · 参数（repo）：代码就是本体那份，只换提示词 / 工具开关，跟着仓库走；
//   · 目录 / 分支（local）：指向本机另一棵 checkout，只在这台存在，别人 pull 不到。
const VARIANT_ID_RE = /^[a-z0-9][a-z0-9._-]*$/i;

function variantStrip(ext, variants) {
  const wrap = el("div", "variants");
  wrap.append(el("span", "variants-label", "变体"));
  for (const item of variants) {
    const row = el("button", "variant");
    row.type = "button";
    row.dataset.tone = item.load?.state === "failed" ? "bad" : item.installed ? "ok" : "off";
    row.append(el("span", "variant-id", item.id));
    const params = Object.keys(item.params ?? {});
    row.append(
      el("span", "variant-src", item.source === "repo" ? (params.length ? "参数" : "同代码") : item.branch ? "分支" : "目录"),
    );
    row.title =
      item.source === "repo"
        ? `${ext.name}@${item.id} · 参数变体（写在 nous.json 的 variants 里）`
        : `${ext.name}@${item.id}\n${item.dir ?? ""}${item.branch ? `\n分支 ${item.branch}` : ""}`;
    row.onclick = () => openVariantDialog(ext, item);
    wrap.append(row);
  }
  const add = el("button", "variant-add", "+");
  add.type = "button";
  add.title = `给 ${ext.name} 加一个变体（另一个可装载名字）`;
  add.onclick = () => openVariantDialog(ext, null);
  wrap.append(add);
  return wrap;
}

// 名字**跟着当前扩展和变体 id 走**，因为是算出来的。别加 placeholder：写死的示例
// 一旦跟当前扩展对不上，比什么都不写更糟（它看着像真的）。
// 路径和分支名都是**算**出来的，跟着当前扩展和变体 id 走。
function variantTargets() {
  const ext = state.variant?.ext?.name ?? "扩展";
  const id = $("variant-id").value.trim() || "变体";
  const repo = state.data?.repo?.path ?? "/opt/Nous-extend";
  const parent = repo.replace(/\/+$/, "").replace(/\/[^/]+$/, "");
  return { ext, id, branch: `ext/${ext}-${id}`, worktree: `${parent}/nous-ext-${ext}-${id}` };
}

function renderVariantHints() {
  const t = variantTargets();
  $("variant-where").textContent = `树 ${t.worktree} · 分支 ${t.branch}`;
  $("variant-where").title = `${t.worktree}\n分支 ${t.branch}`;
}

function openVariantDialog(ext, item) {
  state.variant = { ext, item };
  $("variant").reset();
  $("variant-err").hidden = true;
  $("variant-title").textContent = item ? `${ext.name}@${item.id}` : `给 ${ext.name} 加变体`;
  $("variant-fields").hidden = Boolean(item);
  $("variant-code").hidden = true;
  $("variant-where").textContent = "";
  $("variant-info").hidden = !item;
  $("variant-remove").hidden = !item || item.source !== "local";
  $("variant-ok").hidden = Boolean(item);
  if (item) {
    // 参数变体只能改清单：删掉那段 JSON 再发布，别在面板里再造一个编辑面。
    $("variant-info").textContent = [
      item.source === "repo"
        ? "参数变体：代码就是本体那份，写在 nous.json 的 variants 里 —— 要改就开「代码 ›」抽屉，删掉那段 JSON 再发布。"
        : "本机绑定：代码在这台上的某个目录里，只在这台存在。",
      Object.keys(item.params ?? {}).length ? `参数：${JSON.stringify(item.params)}` : null,
      item.dir ? `目录：${item.dir}` : null,
      item.branch ? `分支：${item.branch}` : null,
      item.load?.state === "failed" ? `装载失败：${item.load.error}` : item.installed ? "进程里：已装载" : "进程里：没装上",
      item.source === "local" ? "这个变体的代码不在主树里 —— 点「改代码 ›」改的就是上面那个目录。" : null,
    ]
      .filter(Boolean)
      .join("\n");
    $("variant-remove").textContent = item.branch ? "拆树并解绑" : "解绑";
    $("variant-code").hidden = item.source !== "local";
  } else {
    renderVariantHints();
    setTimeout(() => $("variant-id").focus(), 0);
  }
  $("variant-scrim").hidden = false;
  $("variant").hidden = false;
}

function closeVariant() {
  $("variant-scrim").hidden = true;
  $("variant").hidden = true;
  state.variant = null;
}

async function submitVariant(event) {
  event.preventDefault();
  const ctx = state.variant;
  if (!ctx?.ext) return;
  const err = $("variant-err");
  err.hidden = true;
  const id = $("variant-id").value.trim();
  if (!VARIANT_ID_RE.test(id) || id.includes("@")) {
    err.textContent = "变体 id 只能用字母、数字和 . _ -（不带 @）";
    err.hidden = false;
    return;
  }
  const body = { name: ctx.ext.name, id, action: "branch", branch: $("variant-branch").value.trim() };
  const ok = $("variant-ok");
  ok.disabled = true;
  ok.textContent = "开树…";
  try {
    const result = await post(`${state.base}v1/extensions/variant`, body);
    closeVariant();
    await load({ quiet: true });
    const key = `${ctx.ext.name}@${id}`;
    const failed = result.load?.[key]?.state === "failed";
    toast(failed ? `${key} 绑上了，但装载失败（卡片上是红的）` : `已加变体 ${key}`, failed ? "bad" : "");
  } catch (error) {
    err.textContent = String(error?.message ?? error);
    err.hidden = false;
  } finally {
    ok.disabled = false;
    ok.textContent = "开树并绑定";
  }
}

async function removeVariant() {
  const ctx = state.variant;
  const item = ctx?.item;
  if (!item) return;
  const err = $("variant-err");
  err.hidden = true;

  const run = async () => {
    const btn = $("variant-remove");
    btn.disabled = true;
    const pruning = item.source === "local" && Boolean(item.branch);
    try {
      const result = await post(`${state.base}v1/extensions/variant`, {
        name: ctx.ext.name,
        id: item.id,
        // 有分支才有树可拆；老的「登记型」变体（没有分支）只解绑。
        action: pruning ? "prune" : "unbind",
      });
      closeVariant();
      await load({ quiet: true });
      if (!pruning) return toast(`已解绑 ${item.key}`);
      const w = result?.worktree ?? {};
      if (w.remoteError) {
        toast(`树和本地分支已删，但 GitHub 那条没删掉：\n${w.remoteError}`, "bad");
      } else if (w.branchDeleted) {
        toast(`已拆树并删掉分支 ${w.branch}${w.remoteDeleted ? "（本地 + GitHub）" : "（本地；GitHub 上本来没有）"}`);
      } else {
        toast(`已拆树并解绑 ${item.key}（分支 ${w.branch} 没删掉，去命令行看看）`, "bad");
      }
    } catch (error) {
      err.textContent = String(error?.message ?? error);
      err.hidden = false;
    } finally {
      btn.disabled = false;
    }
  };

  // 分支上有没并进 main 的提交 = 真会丢东西 → 手输分支名再删。
  // 空壳 / 已合并的分支没什么可惜的，直接拆。
  if (item.branch && item.unmerged > 0) {
    // 先关掉变体弹层再开确认框：两层叠着时，确认框的按钮点不到（被上层的 form 吞掉）。
    closeVariant();
    const remoteNote = item.remoteBranch ? "，GitHub 上那条也一起删" : "";
    confirmName({
      title: `拆树并解绑 ${item.key}？`,
      hint: `分支 ${item.branch} 上有 ${item.unmerged} 个提交没并进 main${remoteNote}。删了这些代码就只能从 reflog 里捞。`,
      name: item.branch,
      label: "删掉",
      onConfirm: run,
    });
    return;
  }
  await run();
}

$("variant-close").onclick = () => closeVariant();
$("variant-scrim").onclick = () => closeVariant();
$("variant-remove").onclick = () => void removeVariant();
$("variant-code").onclick = () => {
  const ctx = state.variant;
  closeVariant();
  if (ctx?.ext) void openExtensionEditor(ctx.ext, ctx.item.id);
};
$("variant-id").addEventListener("input", renderVariantHints);
$("variant").addEventListener("submit", submitVariant);

// 卡底三个数：扫一眼就得知道这条扩展有多大。动作不在这儿了 —— 卡片右键菜单里。
function productFoot(ext) {
  const foot = el("div", "product-foot");
  const meta = (label, value, title) => {
    const span = el("span", "pmeta");
    span.append(el("span", "pmeta-k", label));
    span.append(el("span", "pmeta-v", value));
    if (title) span.title = title;
    return span;
  };

  const sections = ext.sections ?? [];
  foot.append(meta("提示词", sections.length ? `${sections.length} 段` : "无", sections.join("、") || "不塞提示词"));

  const needs = Object.keys(ext.needs ?? {});
  foot.append(meta("依赖", needs.length ? `${needs.length} 项` : "无", needs.join("、") || "不需要宿主喂东西"));

  const users = state.data.usage?.[ext.name] ?? [];
  foot.append(
    meta(
      "在用",
      `${users.length} 条`,
      users.map((id) => state.names.get(id) ?? `#${id}`).join("、") || "还没有会话选它",
    ),
  );

  return foot;
}

// 清单（nous.json 的 sections）跟进程里真装的段落对不上 = 页面在照抄一个谎。
// 「提示词 N 段」数是手写的，改代码时最容易忘的就是回头改它。
function sectionDrift(ext) {
  const live = state.data?.live?.[ext.name];
  // 没装 / 装载失败：没有真值可比，别在这儿重复报（那两种有自己的一档）。
  if (!live) return null;
  const said = (Array.isArray(ext.sections) ? ext.sections : []).filter(Boolean);
  const missing = said.filter((key) => !live.includes(key));
  const extra = live.filter((key) => !said.includes(key));
  if (!missing.length && !extra.length) return null;
  const parts = [];
  if (missing.length) parts.push(`代码里没有「${missing.join("、")}」`);
  if (extra.length) parts.push(`代码里多了「${extra.join("、")}」`);
  return { said: said.length, real: live.length, text: `清单写 ${said.length} 段、实际装 ${live.length} 段 —— ${parts.join("；")}` };
}

// ── 渲染 ───────────────────────────────────────────────────────────────────

// 顶部筛选栏：系统视图（全部 / 内置类别 / 未分类 / 已归档）+ 状态视图。
// 这些不是「类别」—— 是观察整个列表的几种方式，所以不占左栏。
function renderFilters(ours) {
  const box = $("filters");
  box.textContent = "";
  const { counts, none } = categoryCounts(ours);
  const archived = state.data.archived ?? [];

  const chip = (label, value, count, { tone = "", title = "", dot = "" } = {}) => {
    const button = el("button", "filter-chip" + (state.filter === value ? " on" : "") + (tone ? ` ${tone}` : ""));
    button.type = "button";
    if (dot) {
      const mark = el("span", "filter-dot");
      mark.dataset.tone = dot;
      button.append(mark);
    }
    button.append(el("span", "filter-label", label));
    button.append(el("span", "filter-n", String(count)));
    if (title) button.title = title;
    button.onclick = () => {
      state.filter = state.filter === value ? "" : value;
      state.detail = null; // 换筛选就是回列表
      state.editing = null;
      render();
    };
    return button;
  };

  box.append(chip("全部", "", ours.length));

  // 选中的是自己建的类别时，把「父级含子级」摆在结果旁边，而不是写进左栏说明书。
  const names = allCategoryNames(ours);
  if (
    state.filter &&
    state.filter !== "__none__" &&
    state.filter !== "__archived__" &&
    !isStatusFilter(state.filter) &&
    !protectedNames().includes(state.filter)
  ) {
    const kids = names.filter((name) => name !== state.filter && isUnder(name, state.filter)).length;
    const scope = el("button", "filter-chip on");
    scope.type = "button";
    scope.append(el("span", "filter-label", kids ? `${leafOf(state.filter)} · 含 ${kids} 个子级` : leafOf(state.filter)));
    scope.append(el("span", "filter-n", "×"));
    scope.title = `只看 ${state.filter}${kids ? "（含子级）" : ""} —— 点一下取消`;
    scope.onclick = () => {
      state.filter = "";
      render();
    };
    box.append(scope);
  }

  for (const name of protectedNames()) {
    box.append(
      chip(leafOf(name), name, counts.get(name) ?? 0, {
        tone: "locked",
        title: `${name} · 内置类别，删不掉改不了名`,
      }),
    );
  }
  if (none) box.append(chip("未分类", "__none__", none, { title: "一个类别都没挂的扩展" }));

  // 状态图例就是筛选：非零才出现，色点只是辅助，文字才是本体。
  const byStatus = new Map();
  for (const ext of ours) {
    const key = statusOf(ext);
    byStatus.set(key, (byStatus.get(key) ?? 0) + 1);
  }
  const views = STATUS_VIEWS.filter((view) => (byStatus.get(view.key) ?? 0) > 0);
  if (views.length) {
    box.append(el("span", "filter-sep"));
    for (const view of views) {
      box.append(chip(view.label, view.value, byStatus.get(view.key), { dot: view.tone, title: view.title }));
    }
  }

  if (archived.length) {
    box.append(
      chip("已归档", "__archived__", archived.length, {
        tone: "archived",
        title: "从仓库归档的扩展：可以恢复，也可以彻底删除",
      }),
    );
  }
}

function renderFacets(ours) {
  const { counts, none } = categoryCounts(ours);
  const names = allCategoryNames(ours);
  // 左栏只放**人自己建的**类别。全部 / 未分类 / 已归档 / 内置类别（Basics）都是系统视图，
  // 在顶部筛选栏 —— 栏里混着系统项，「我自己建了什么」就看不出来了。
  const installed = new Set(protectedNames());
  const mine = names.filter((name) => !installed.has(name));

  if (state.filter && state.filter !== "__none__" && state.filter !== "__archived__" && !isStatusFilter(state.filter) && !names.includes(state.filter)) state.filter = "";
  if (state.filter === "__none__" && !none) state.filter = "";
  if (state.filter === "__archived__" && !(state.data.archived ?? []).length) state.filter = "";

  const facets = $("facets");
  facets.textContent = "";

  // 谁有后代谁就是父级（allCategoryNames 已经把祖先补进 names 了）。
  const parents = new Set();
  for (const name of names) for (const parent of ancestorsOf(name)) parents.add(parent);

  const row = (label, value, count, path = "") => {
    const item = el("div", "facet" + (state.filter === value ? " on" : "") + (parents.has(path) ? " parent" : ""));

    // 改名就在原地改：一行输入框，回车落盘，Esc 反悔。填的是全路径，所以改前缀 = 搬家。
    if (state.renaming === value) {
      const input = el("input", "facet-input");
      input.type = "text";
      input.value = path;
      input.setAttribute("aria-label", `重命名类别 ${path}`);
      input.style.marginLeft = `${Math.min(depthOf(path), 3) * 11}px`;
      let done = false;
      const finish = (keep) => {
        if (done) return;
        done = true;
        const next = input.value.trim();
        state.renaming = null;
        render();
        if (keep && next && next !== path) void renameCategory(path, next);
      };
      input.onkeydown = (event) => {
        if (event.key === "Enter") {
          event.preventDefault();
          finish(true);
        } else if (event.key === "Escape") {
          event.preventDefault();
          finish(false);
        }
      };
      input.onblur = () => finish(true);
      item.append(input);
      return item;
    }

    const main = el("button", "facet-main");
    main.type = "button";
    main.append(el("span", "facet-label", label));
    main.append(el("span", "facet-n", String(count)));
    if (path) {
      // 栏里只显示末段，缩进已经说明它在谁底下；全路径放 title。
      main.title = isProtected(path) ? `${path} · 内置类别` : path;
      main.style.paddingLeft = `${8 + Math.min(depthOf(path), 3) * 11}px`;
    }
    main.onclick = () => {
      state.filter = state.filter === value ? "" : value;
      state.detail = null; // 换筛选就是回列表
      state.editing = null;
      render();
    };
    item.append(main);

    // 「全部 / 未分类 / 已归档」不是仓库里的类别，没菜单。
    if (value && value !== "__none__" && value !== "__archived__") {
      item.dataset.category = path;

      // 可见的菜单入口：桌面 hover 出现，触屏常驻。右键 / 长按只是快捷方式。
      const menuBtn = el("button", "facet-menu", "⋯");
      menuBtn.type = "button";
      menuBtn.title = isProtected(path)
        ? `${path} 的菜单：新建子类别`
        : `${path} 的菜单：新建子类别 / 改名 / 删除`;
      menuBtn.setAttribute("aria-label", `${path} 的类别菜单`);
      menuBtn.onclick = (event) => {
        event.stopPropagation();
        const rect = menuBtn.getBoundingClientRect();
        openCategoryMenu(rect.left, rect.bottom + 4, path);
      };
      item.append(menuBtn);

      item.oncontextmenu = (event) => {
        event.preventDefault();
        openCategoryMenu(event.clientX, event.clientY, path);
      };
      main.onkeydown = (event) => {
        if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
          event.preventDefault();
          const rect = main.getBoundingClientRect();
          openCategoryMenu(rect.left + 8, rect.bottom - 4, path);
        }
      };
      attachMenuGesture(main, (mx, my) => openCategoryMenu(mx, my, path));
    }
    return item;
  };

  for (const name of mine) facets.append(row(leafOf(name), name, counts.get(name) ?? 0, name));

  // 整串挂上去之后再聚焦：行在 append 之前是「失联」的，focus 会落空。
  const renaming = facets.querySelector(".facet-input");
  if (renaming) {
    renaming.focus();
    renaming.select();
  }

  $("cat-count").textContent = String(mine.length);
}

function renderRailNew() {
  const host = $("rail-new");
  host.textContent = "";
  if (state.creating) {
    const input = el("input", "rail-input");
    input.type = "text";
    input.placeholder = "新类别名，如 工具/搜索";
    input.value = state.createPrefix;
    let done = false;
    const commit = () => {
      if (done) return;
      done = true;
      const value = input.value;
      state.creating = false;
      state.createPrefix = "";
      render();
      if (value.trim()) void addCategory(value);
    };
    input.onkeydown = (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
      } else if (event.key === "Escape") {
        event.preventDefault();
        done = true;
        state.creating = false;
        state.createPrefix = "";
        render();
      }
    };
    input.onblur = commit;

    // 说明只在「正在输入」这一刻出现：/ 是层级，回车落盘，写回仓库。
    const hint = el("p", "rail-hint");
    const updateHint = () => {
      const parts = input.value.split(CAT_SEP).filter(Boolean);
      hint.textContent = parts.length > 1 ? `${parts.join(" › ")} · 回车建` : "回车建 · 会写回仓库（8 秒内同步）";
    };
    input.oninput = updateHint;
    updateHint();
    host.append(input, hint);
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  } else {
    const add = el("button", "rail-add", "+ 新建类别");
    add.type = "button";
    add.onclick = () => {
      state.creating = true;
      render();
    };
    host.append(add);
  }
}

function renderSuggestions(ours) {
  const list = $("tag-suggestions");
  list.textContent = "";
  for (const name of allCategoryNames(ours)) {
    const option = el("option");
    option.value = name;
    list.append(option);
  }
}

// 归档区：仓库里 `.archive/*`。与主网格不同 —— 这些不是「装着/未装」的扩展，是备份。
function renderArchivedGrid() {
  const grid = $("grid");
  grid.textContent = "";
  const items = state.data.archived ?? [];
  const list = el("div", "arch-list");
  for (const item of items) {
    const row = el("div", "arch-row");
    const main = el("div", "arch-main");
    const head = el("div", "arch-head");
    head.append(el("span", "arch-name", item.name));
    if (item.version) head.append(el("span", "arch-ver", "v" + item.version));
    head.append(
      el(
        "span",
        "arch-when dim",
        item.archivedAt
          ? new Date(item.archivedAt).toLocaleString("zh-CN", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" })
          : "未提交",
      ),
    );
    main.append(head);
    if (item.summary) main.append(el("p", "arch-sum", item.summary));
    row.append(main);

    const acts = el("div", "arch-acts");
    const back = el("button", "btn btn-small", "恢复");
    back.type = "button";
    back.title = `把 .archive/${item.name} 搬回仓库根目录，并 commit + push`;
    back.onclick = () => void restoreArchived(item.name);
    acts.append(back);
    const gone = el("button", "btn btn-small btn-bad", "彻底删除");
    gone.type = "button";
    gone.title = `连备份一起删掉：git rm -r .archive/${item.name}`;
    gone.onclick = () => void purgeArchived(item.name);
    acts.append(gone);
    row.append(acts);
    list.append(row);
  }
  grid.append(list);
  if (!items.length) grid.append(el("p", "empty", "回收站是空的。"));
}

// ── 详情：点卡进来 ─────────────────────────────────────────────────────────
//
// 电商那套：列表 → 商品详情。详情占的是**主区**，不是又一个右侧浮层 ——
// 右边那条边已经归代码抽屉了，两个浮层抢同一条边迟早互相踩。
//
// 分工定死：详情只做「读 + 类别」。类别的勾选是它**唯一**的编辑面，动作
// （代码 / 卸载 / 归档）全在卡片右键菜单里。同一份 category[] 开两个写入
// 路径，迟早漂移。

function detailSection(title, hint) {
  const node = el("section", "detail-sec");
  const head = el("div", "detail-sec-head");
  head.append(el("h3", "detail-sec-title", title));
  if (hint) head.append(el("span", "detail-sec-hint", hint));
  node.append(head);
  const body = el("div", "detail-sec-body");
  node.append(body);
  return { node, body };
}

function detailRow(label, value, title = "") {
  const row = el("div", "detail-row");
  row.append(el("span", "detail-row-k", label));
  const text = el("span", "detail-row-v", value);
  if (title) text.title = title;
  row.append(text);
  return row;
}

function alertLine(tone, label, text) {
  const line = el("p", `detail-alert ${tone}`);
  line.append(el("span", "detail-alert-k", label));
  line.append(el("span", "detail-alert-v", text));
  return line;
}

function openDetail(name, { editTags = false } = {}) {
  state.detail = name;
  // 「类别」那一项是跳到详情里的勾选区，顺手把输入框打开 —— 菜单不做第二个编辑面。
  state.editing = editTags ? name : null;
  render();
  $sel(".results")?.scrollTo({ top: 0, behavior: "smooth" });
}

function closeDetail() {
  if (!state.detail) return;
  state.detail = null;
  state.editing = null;
  render();
  $sel(".results")?.scrollTo({ top: 0, behavior: "smooth" });
}

function renderDetail() {
  const host = $("detail");
  const ext = oursNow().find((item) => item.name === state.detail);
  // 详情开着的时候这个扩展被归档 / 改名走了：退回列表，别留一面空墙。
  if (!ext) {
    state.detail = null;
    state.editing = null;
    host.hidden = true;
    $("grid").hidden = false;
    renderGrid();
    return;
  }

  host.textContent = "";
  const cats = categoriesOf(ext);
  // 顶栏那一格在列表里是计数，在详情里就是这条的类别 —— 别写一句「扩展详情」凑数。
  $("result-count").textContent = cats.length ? cats.join(" · ") : "未分类";

  const status = statusOf(ext);
  const ghost = status === "ghost";
  const failed = status === "failed";
  const view = STATUS_VIEWS.find((item) => item.key === status);

  const back = el("button", "detail-back", "‹ 返回");
  back.type = "button";
  back.title = "回到列表（Esc）";
  back.onclick = closeDetail;
  host.append(back);

  // ── 头：色砖 + 名字 + 状态 + 说明 + 两个入口 ──────────────────────────
  const head = el("header", "detail-head");
  const tile = el("div", "detail-tile");
  tile.style.setProperty("--hue", String(hueOf(ext.name)));
  tile.append(el("span", "detail-mono", initialsOf(ext.name)));
  head.append(tile);

  const id = el("div", "detail-id");
  const nameRow = el("div", "detail-name-row");
  nameRow.append(el("h2", "detail-name", ext.name));
  nameRow.append(el("span", "detail-ver", `v${ext.version}`));
  const badge = el("span", "detail-badge", ghost ? "仅在仓库" : (view?.label ?? "已装"));
  badge.dataset.tone = ghost
    ? "gone"
    : failed
      ? "fail"
      : status === "unpublished"
        ? "unpub"
        : status === "installed"
          ? "ok"
          : "bad";
  nameRow.append(badge);
  if (status === "installed" && state.data.defaults?.includes(ext.name)) {
    const fallback = el("span", "detail-badge", "默认");
    fallback.dataset.tone = "good";
    nameRow.append(fallback);
  }
  id.append(nameRow);
  id.append(el("p", "detail-sum", ext.summary || "没写说明。"));

  const acts = el("div", "detail-acts");
  if (ghost) {
    const restore = el("button", "btn btn-small", "装回本机");
    restore.type = "button";
    restore.title = `把 ${ext.name} 装回这台；仓库里的文件一直没动`;
    restore.onclick = () => void installExtension(ext.name);
    acts.append(restore);
  } else {
    const code = el("button", "btn btn-small", "代码 ›");
    code.type = "button";
    code.title = `看 / 改 ${ext.name} 的源码`;
    code.onclick = () => void openExtensionEditor(ext);
    acts.append(code);
  }
  if (failed) {
    const retry = el("button", "btn btn-small", "重试装载");
    retry.type = "button";
    retry.title = "按磁盘现状立刻重新 import 一次，不用等 30 秒轮询";
    retry.onclick = () => void retryLoad(ext.name);
    acts.append(retry);
  }
  id.append(acts);
  head.append(id);
  host.append(head);

  // ── 真话说在最前面：坏了 / 清单对不上 / 已从本机卸载 ────────────────
  const load = state.data.load?.[ext.name];
  if (failed && load?.error) host.append(alertLine("bad", "装载失败", load.error));
  const drift = ghost || failed ? null : sectionDrift(ext);
  if (drift) host.append(alertLine("warn", "清单对不上", drift.text));
  if (ghost) {
    const at = state.data.local.deleted[ext.name]?.at;
    host.append(
      alertLine(
        "plain",
        "已从本机卸载",
        at
          ? `${new Date(at).toLocaleString("zh-CN")} · 源码在本机垃圾站，仓库和 GitHub 都没动`
          : "源码已移到本机垃圾站，仓库和 GitHub 都没动",
      ),
    );
  }

  // ── 两列：类别 / 变体，清单 / 仓库 ────────────────────────────────────
  const grid = el("div", "detail-grid");

  const cat = detailSection("类别", "写回仓库 · 8 秒内同步");
  cat.body.append(tagRow(ext, { readOnly: ghost, className: "detail-tags" }));
  grid.append(cat.node);

  if (!ghost) {
    const variants = state.data?.variants?.[ext.name] ?? [];
    const sec = detailSection("变体", variants.length ? `${variants.length} 个可装载名字` : "另一个可装载名字");
    sec.body.append(variantStrip(ext, variants));
    grid.append(sec.node);
  }

  const list = detailSection("清单", "nous.json 写的");
  const sections = (ext.sections ?? []).filter(Boolean);
  list.body.append(detailRow("文件", ext.file ?? "index.js"));
  list.body.append(
    detailRow("提示词", sections.length ? sections.map((key) => `「${key}」`).join(" · ") : "不塞提示词"),
  );
  const needs = Object.keys(ext.needs ?? {});
  list.body.append(detailRow("依赖", needs.length ? needs.join(" · ") : "不需要宿主喂东西"));
  const users = (state.data.usage?.[ext.name] ?? []).map((cid) => state.names.get(cid) ?? `#${cid}`);
  list.body.append(
    detailRow("在用", users.length ? `${users.length} 条会话 · ${users.join(" · ")}` : "还没会话选它"),
  );
  grid.append(list.node);

  const where = detailSection("仓库", "HEAD 里有没有 · 这台装没装");
  where.body.append(detailRow("发布", ext.tracked === false ? "未发布 · HEAD 里没有" : "已进 HEAD"));
  where.body.append(detailRow("本机", ghost ? "已卸载" : "装着"));
  grid.append(where.node);

  host.append(grid);

  // 输入框必须等整串挂进文档之后再 focus —— 行还在内存里时聚焦会落空（左栏踩过）。
  const input = host.querySelector(".tag-input");
  if (input) {
    input.focus();
    input.setSelectionRange(input.value.length, input.value.length);
  }
}

function renderGrid() {
  if (state.filter === "__archived__") return renderArchivedGrid();
  const ours = oursNow();
  const query = state.query.trim().toLowerCase();
  let shown = ours;
  if (state.filter === "__none__") shown = shown.filter((ext) => !categoriesOf(ext).length);
  else if (isStatusFilter(state.filter)) shown = shown.filter((ext) => statusValueOf(ext) === state.filter);
  else if (state.filter) shown = shown.filter((ext) => categoriesOf(ext).some((tag) => isUnder(tag, state.filter)));
  if (query) {
    shown = shown.filter((ext) =>
      [ext.name, ext.summary, ...categoriesOf(ext)].join(" ").toLowerCase().includes(query),
    );
  }

  // 灰卡沉到最后：一眼看出「这些没装」，也别让它们插在正常卡中间挡视线。
  shown = [...shown].sort((a, b) => Number(localDeleted(a.name)) - Number(localDeleted(b.name)));

  const grid = $("grid");
  grid.textContent = "";
  for (const ext of shown) grid.append(productCard(ext));
  if (!shown.length) {
    const view = STATUS_VIEWS.find((item) => item.value === state.filter);
    const why = !state.data
      ? "读取中…"
      : query
        ? "没有匹配的扩展。"
        : state.filter === "__none__"
          ? "没有未分类的扩展。"
          : view
            ? `没有「${view.label}」的扩展。`
            : state.filter
              ? `没有标「${state.filter}」的扩展。`
              : "仓库的 index.json 里一条自研扩展都没有。";
    grid.append(el("div", "empty", why));
  }

  $("result-count").textContent =
    shown.length === ours.length ? `${ours.length} 个扩展` : `${shown.length} / ${ours.length} 个扩展`;
}

function render() {
  const data = state.data;
  if (!data) return;
  closeMenu();

  const repo = data.repo ?? {};
  const drift = Boolean(repo.drift);
  $("repo-path").textContent = repo.path ?? "—";
  const headLine = `lock ${short(repo.lock)} · HEAD ${short(repo.head)}`;
  $("repo").textContent = drift ? `${headLine} · ⚠ 不一致` : headLine;
  $("repo").dataset.drift = drift ? "1" : "0";
  $("repo").title = drift
    ? "仓库 HEAD 跟 extensions.lock.json 钉的不是同一个提交 —— 改完没重启，现在跑的是旧代码"
    : "跑的就是钉的那一版";

  const ours = oursNow();
  renderFilters(ours);
  renderFacets(ours);
  renderRailNew();
  renderSuggestions(ours);
  // 详情占主区：网格让位，而不是被挤窄。
  $("grid").hidden = Boolean(state.detail);
  $("detail").hidden = !state.detail;
  if (state.detail) renderDetail();
  else renderGrid();
  $("count").textContent = `${ours.length} 个自研扩展`;
  renderSync();
}

// ── 取数 ───────────────────────────────────────────────────────────────────

async function load({ quiet = false } = {}) {
  const dot = $sel("#refresh .dot");
  if (!quiet) dot.dataset.state = "wait";
  try {
    const ctx = await get(SELF + "__nous/ctx");
    const tag =
      ctx.blocks?.find((block) => block.id === "qrd3wh") ??
      ctx.blocks?.find((block) => block.kind === "group" && block.name === "session");
    if (!tag) throw new Error("这一盘里没有 qrd3wh（session 引擎）—— 扩展装在它进程里，它不在就没得问");

    const base = tag.url.endsWith("/") ? tag.url : tag.url + "/";
    state.base = base;

    const [data, tree, sync] = await Promise.all([
      get(base + "v1/extensions?usage=1"),
      get(base + "v1/sessions"),
      get(base + "v1/extensions/sync").catch(() => null),
    ]);

    state.data = data;
    state.names = new Map((tree.items ?? []).map((item) => [item.id, item.name]));
    if (sync) {
      state.ahead = sync.ahead ?? 0;
      state.behind = sync.behind ?? 0;
      // dirty 也得跟着刷新 —— 不刷的话，别处（编辑器保存）置上的「待同步」
      // 会一直挂到刷新页面为止，而仓库早就是干净的。
      // 但别踩掉正在 debounce 的类别写入（timer 在跑 = 那笔还没推）。
      if (!syncTimer) {
        state.dirty = sync.dirty ? 1 : 0;
        state.codeDirty = Boolean(sync.dirty);
      }
    }
    if (state.sync !== "pushing") {
      state.sync = "idle";
      state.syncError = "";
    }
    render();
    dot.dataset.state = "ok";
  } catch (error) {
    dot.dataset.state = "bad";
    $("count").textContent = "拿不到";
    $("result-count").textContent = "";
    $("grid").textContent = "";
    $("grid").append(el("div", "empty", String(error?.message ?? error)));
    if (!quiet) toast(String(error?.message ?? error), "bad");
  }
}

// ── 代码抽屉 ────────────────────────────────────────────────────────────────
//
// 保存和发布是两个动作，别合并。
//   · 保存 = PUT /v1/extensions/file：立刻落盘，**不提交** —— 敲到一半可以停。
//   · 发布 = POST /v1/extensions/publish：语法闸 + index.mjs + commit + push。
// 合起来的话，半截文件会被推上去；而扩展里一个语法错误，下一次进程启动会在
// 顶层 import 那一行直接炸掉 —— 不是这一块坏，是整个会话引擎起不来。

const bytes = (n) => (n < 1024 ? `${n} B` : `${(n / 1024).toFixed(1)} KB`);
const editorDirty = () => Boolean(state.editor) && $("ed-text").value !== state.editor.text;

async function openExtensionEditor(ext, variant = null) {
  // 本地删掉的扩展没有文件可读：别让抽屉卡在 404 上。
  if (localDeleted(ext.name)) {
    toast(`${ext.name} 已从本机卸载，先点「装回本机」恢复`);
    return;
  }
  closeMenu();
  state.editor = {
    name: ext.name,
    // 变体的代码在它自己的 checkout 里（可能还挂在自己的分支上）：读、写、发布全得跟着走。
    variant: variant ?? null,
    version: ext.version,
    file: ext.file ?? "index.js", // 清单里的代码路径：会话提示词拿它，不信「现在开着哪个文件」
    files: [],
    filesLoading: true,
    path: null,
    kind: null,
    text: "",
    base: null,
    pending: new Set(),
    saving: false,
    publishing: false,
    error: "",
    notice: "",
    // ── 差异视图 ──────────────────────────────────────────────────────────
    mode: "edit", // edit | diff
    baseText: null, // HEAD 里那一版；null = 这个文件还没进仓库
    baseMissing: false,
    diff: null, // buildDiff() 的结果
    expanded: new Set(), // 手动展开过的折叠块 `${from}-${to}`
    lineCount: 1,
    diffTimer: null,
    delta: null,
  };
  $("ed-text").value = "";
  $("ed-text").disabled = true;
  $("ed-path").textContent = "—";
  $("ed-scrim").hidden = false;
  $("editor").hidden = false;
  $("ed-name").textContent = variant ? `${ext.name}@${variant}` : ext.name;
  $("ed-meta").textContent = variant
    ? `${ext.version} · 变体 · 另一棵树 / 另一个分支`
    : `v${ext.version} · ${ext.file ?? "index.js"}`;
  renderEditorFiles();
  renderEditorState();

  try {
    const q = variant ? `&variant=${encodeURIComponent(variant)}` : "";
    const list = await get(`${state.base}v1/extensions/files?name=${encodeURIComponent(ext.name)}${q}`);
    state.editor.files = list.files ?? [];
    state.editor.filesLoading = false;
    renderEditorFiles();
    // 先开 nous.json（清单最常改），没有就 index.js，再没有就第一个。
    const first =
      state.editor.files.find((f) => f.path === "nous.json") ??
      state.editor.files.find((f) => f.path === (ext.file ?? "index.js")) ??
      state.editor.files[0];
    if (first) await openEditorFile(first.path, { force: true });
    else $("ed-path").textContent = "这个扩展目录里没有可编辑的文件";
  } catch (error) {
    state.editor.filesLoading = false;
    state.editor.error = `读文件列表失败：${error.message}`;
    renderEditorFiles();
    renderEditorState();
  }
}

function closeEditor({ force = false } = {}) {
  if (!state.editor) return;
  if (!force && editorDirty() && !confirm("这个文件有没保存的改动，关掉就丢了。继续？")) return;
  state.editor = null;
  $("editor").hidden = true;
  $("ed-scrim").hidden = true;
}

function renderEditorFiles() {
  const ed = state.editor;
  if (!ed) return;
  const nav = $("ed-files");
  nav.textContent = "";
  if (ed.filesLoading) {
    nav.append(el("div", "ed-files-empty", "读文件列表…"));
    return;
  }
  if (!ed.files.length) {
    nav.append(el("div", "ed-files-empty", "没有 .js / .json / .md 文件"));
    return;
  }
  for (const file of ed.files) {
    const btn = el("button", "ed-file");
    btn.type = "button";
    btn.dataset.kind = file.kind;
    btn.setAttribute("aria-current", String(file.path === ed.path));
    btn.append(el("span", "ed-file-name", file.path));
    btn.append(el("span", "ed-file-size", bytes(file.size)));
    btn.title = `${file.path} · ${bytes(file.size)}`;
    btn.onclick = () => void openEditorFile(file.path);
    nav.append(btn);
  }
}

function renderEditorState() {
  const ed = state.editor;
  if (!ed) return;
  const chip = $("ed-state");
  let tone = "ok";
  let text = "已同步";
  if (ed.error) {
    tone = "bad";
    text = "出错";
  } else if (ed.saving) {
    tone = "pending";
    text = "保存中…";
  } else if (ed.publishing) {
    tone = "pending";
    text = "发布中…";
  } else if (editorDirty()) {
    tone = "dirty";
    text = "未保存";
  } else if (ed.pending?.size) {
    tone = "pending";
    text = `已保存 · 待发布 ${ed.pending.size}`;
  }
  chip.dataset.tone = tone;
  chip.textContent = text;

  const busy = ed.saving || ed.publishing || !ed.path;
  $("ed-save").disabled = busy;
  $("ed-publish").disabled = busy;
  $("ed-dev").disabled = busy;

  const msg = $("ed-msg");
  if (ed.error) {
    msg.hidden = false;
    msg.dataset.tone = "bad";
    msg.textContent = ed.error;
  } else if (ed.notice) {
    msg.hidden = false;
    msg.dataset.tone = "ok";
    msg.textContent = ed.notice;
  } else {
    msg.hidden = true;
    msg.textContent = "";
  }
}

async function openEditorFile(path, { force = false } = {}) {
  const ed = state.editor;
  if (!ed) return;
  if (!force && editorDirty() && !confirm(`「${ed.path}」有没保存的改动，切过去会丢掉。继续？`)) return;
  try {
    const file = await get(
      `${state.base}v1/extensions/file?name=${encodeURIComponent(ed.name)}&path=${encodeURIComponent(path)}` +
        (ed.variant ? `&variant=${encodeURIComponent(ed.variant)}` : ""),
    );
    ed.path = file.path;
    ed.kind = file.kind;
    ed.text = file.text;
    ed.base = file.etag;
    ed.baseText = file.base ?? null;
    ed.baseMissing = file.base == null;
    ed.expanded.clear();
    ed.error = "";
    ed.notice = "";
    const area = $("ed-text");
    area.disabled = false;
    area.value = file.text;
    $("ed-path").textContent = `${ed.name}/${file.path}`;
    ed.lineCount = file.text.split("\n").length;
    recomputeDiff();
    renderEditorFiles();
    renderEditorState();
  } catch (error) {
    ed.error = `读文件失败：${error.message}`;
    renderEditorState();
  }
}

async function saveEditorFile({ quiet = false } = {}) {
  const ed = state.editor;
  if (!ed?.path || ed.saving) return false;
  const text = $("ed-text").value;
  ed.saving = true;
  ed.error = "";
  ed.notice = "";
  renderEditorState();
  try {
    const saved = await fetch(`${state.base}v1/extensions/file`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      // base 是「我改的是哪一版」：别人先改了，服务端 409，别把对方的字盖掉。
      body: JSON.stringify({ name: ed.name, path: ed.path, text, base: ed.base, variant: ed.variant }),
    }).then(readJson);
    ed.text = text;
    ed.base = saved.etag;
    ed.pending.add(ed.path);
    if (!quiet) ed.notice = `已保存到磁盘：${ed.name}/${ed.path}（还没提交）`;
    // 顶栏那个 chip 必须跟着变：仓库脏了就不是「已同步」，
    // 否则页面在撒谎（本地改了、远端没动，最坏的那种假象）。
    state.codeDirty = true;
    void get(`${state.base}v1/extensions/sync`)
      .then((next) => {
        state.ahead = next.ahead ?? 0;
        state.behind = next.behind ?? 0;
        state.codeDirty = Boolean(next.dirty);
        renderSync();
      })
      .catch(() => {});
    return true;
  } catch (error) {
    ed.error = `保存失败：${error.message}`;
    return false;
  } finally {
    ed.saving = false;
    renderEditorState();
  }
}

async function publishEditor() {
  const ed = state.editor;
  if (!ed || ed.publishing) return;
  if (editorDirty() && !(await saveEditorFile({ quiet: true }))) return;
  ed.publishing = true;
  ed.error = "";
  ed.notice = "";
  renderEditorState();
  try {
    // 不传 message：说明由服务端按**真正改了的文件**生。
    // 客户端只知道「现在开着哪个文件」，发布推的却是整个工作区 —— 两者很容易不是一回事。
    const result = ed.variant
      ? await post(`${state.base}v1/extensions/publish`, { name: ed.name, variant: ed.variant })
      : await post(`${state.base}v1/extensions/publish`, {});
    ed.pending.clear();
    state.codeDirty = false;
    // 发布成功 = HEAD 变了。重读这个文件拿新基线，否则差异视图还在跟旧 HEAD 比。
    if (ed.path) await openEditorFile(ed.path, { force: true }).catch(() => {});
    if (result.pushed === false) {
      // 提交在本地、远端没动 —— 这是最坏的一种「假成功」，必须红着说出来。
      ed.error = `本地已提交，但推送失败：\n${result.pushError || "未知原因"}\n点「发布」再试一次。`;
    } else if (result.committed) {
      // 发布顺手重装过了：别再说「最多 30 秒后」—— 那是没装的说法。
      const reload = result.reload;
      const where = result.branch ? `分支 ${result.branch}` : "origin";
      const head = `已发布 · ${short(result.head)}${result.pushed ? ` · 已推到 ${where}` : ""}`;
      if (reload?.ok === false) {
        ed.error = `代码已进仓库，但重装失败：\n${reload.error ?? "未知原因"}\n卡片上会红着报，点「重试装载」。`;
      } else {
        const failed = Object.entries(reload?.load ?? {}).filter(([name, state]) => name !== "__reload__" && state?.state === "failed");
        ed.notice = failed.length
          ? `${head}\n已重装，但有 ${failed.length} 个扩展装载失败：${failed.map(([name]) => name).join("、")}`
          : `${head}\n已重装，进程里跑的就是这一版。`;
      }
    } else {
      ed.notice = ed.variant ? "这条分支上没有待提交的改动。" : "工作区没有待提交的改动。";
    }
    await load({ quiet: true });
  } catch (error) {
    ed.error = `发布失败：${error.message}`;
  } finally {
    ed.publishing = false;
    renderEditorState();
  }
}

/** 在 qrd3wh 里开一个会话，让它去改这个扩展 —— 比在 textarea 里敲 400 行划算。 */
async function openDevSession() {
  const ed = state.editor;
  if (!ed) return;
  if (editorDirty() && !(await saveEditorFile({ quiet: true }))) return;
  ed.error = "";
  ed.notice = "";
  $("ed-dev").disabled = true;
  try {
    const device = await get(`${state.base}v1/device`);
    const created = await post(`${state.base}v1/sessions`, { name: `开发 ${ed.name}`, model: device.model });
    const prompt = [
      `仓库在 /opt/Nous-extend，我要改扩展「${ed.name}」。`,
      `先读 /opt/Nous-extend/AGENTS.md 和 index.mjs，再读 ${ed.name}/nous.json 和 ${ed.name}/${ed.file}。`,
      `用三句话讲清它现在做什么、你觉得哪里值得改；我确认之后你再动代码。`,
      `改完跑 node index.mjs，然后 git commit + git push —— 宿主每 30 秒 pull 一次并热重载。`,
      `别碰别的扩展。`,
    ].join("\n");
    await post(`${state.base}v1/sessions/${created.id}/messages`, { content: prompt });
    await post(`${state.base}v1/selection`, { id: created.id });
    window.open(state.base, "_blank", "noopener");
    ed.notice = `已开一个会话（#${created.id}）—— 新标签页里继续说；它改完 push 回仓库，这页会自己跟上。`;
  } catch (error) {
    ed.error = `开会话失败：${error.message}`;
  } finally {
    renderEditorState();
  }
}

// ── 绑定 ───────────────────────────────────────────────────────────────────

$("search").oninput = (event) => {
  state.query = event.target.value;
  state.detail = null;
  state.editing = null;
  $("grid").hidden = false;
  $("detail").hidden = true;
  renderGrid();
};
$("refresh").onclick = () => void load();
// 点它是**看**，不是推。以前这里直接 flushSync(true)，等于一次点击把整个工作区
// （包括你正在改的半截代码）commit + push 上 GitHub。
$("sync").onclick = () => void openPending();
$("pending-cancel").onclick = () => closePending();
$("pending-scrim").onclick = () => closePending();
$("pending-ok").onclick = () => void submitPending();
$("pending-all").onclick = () => {
  const p = state.pending;
  if (!p) return;
  p.repush = false;
  pendingToggle(p.units.flatMap((unit) => unit.files.filter((file) => !file.deleted).map((file) => file.path)), true);
};
$("pending-none").onclick = () => {
  const p = state.pending;
  if (!p) return;
  p.sel = new Set();
  renderPending();
};

// ── 抽屉：行号槽 / 差异视图 / 改动概览 ─────────────────────────────────────
//
// 一份 diff（ed.diff）两个视图共用：编辑态拿它画左侧槽的变更标记和右侧概览条，
// 差异态拿它渲染整页。打字时 160ms 防抖重算 —— 槽上的颜色跟着手走，才有编辑器的感觉。

const ED_LINE = () => parseFloat(getComputedStyle($("ed-text")).lineHeight) || 20;

function recomputeDiff() {
  const ed = state.editor;
  if (!ed?.path) return;
  const text = $("ed-text").value;
  ed.lineCount = text.split("\n").length;
  ed.diff = buildDiff(ed.baseText ?? "", text, { context: 3 });
  renderGutter();
  renderDelta();
  renderOverview();
  if (ed.mode === "diff") renderDiffView();
}

function scheduleDiff() {
  const ed = state.editor;
  if (!ed) return;
  clearTimeout(ed.diffTimer);
  ed.diffTimer = setTimeout(recomputeDiff, 160);
}

function renderDelta() {
  const ed = state.editor;
  const chip = $("ed-delta");
  if (!ed?.diff) {
    chip.hidden = true;
    return;
  }
  const { adds, dels } = ed.diff.stats;
  chip.hidden = adds === 0 && dels === 0;
  $("ed-plus").textContent = `+${adds}`;
  $("ed-minus").textContent = `−${dels}`;
}

/** 左侧行号槽。行高必须和 textarea 完全一致，否则越滚越偏。 */
function renderGutter() {
  const ed = state.editor;
  const gutter = $("ed-gutter");
  const area = $("ed-text");
  if (!ed || !gutter) return;
  const total = Math.max(1, ed.lineCount);
  gutter.style.width = `${Math.max(2, String(total).length) * 7.5 + 26}px`;
  const frag = document.createDocumentFragment();
  for (let n = 1; n <= total; n++) {
    const mark = ed.diff?.marks?.get(n);
    const row = el("div", "gut" + (mark ? ` ${mark}` : ""));
    const gap = ed.diff?.gaps?.get(n);
    if (gap) {
      row.dataset.gap = String(gap);
      row.title = `这上面删掉了 ${gap} 行`;
    }
    row.append(el("span", "gut-n", String(n)));
    frag.append(row);
  }
  const tail = ed.diff?.gaps?.get(total + 1);
  if (tail) {
    const row = el("div", "gut gut-tail");
    row.dataset.gap = String(tail);
    row.title = `文件末尾删掉了 ${tail} 行`;
    frag.append(row);
  }
  gutter.textContent = "";
  gutter.append(frag);
  gutter.scrollTop = area.scrollTop;
}

function syncGutter() {
  const gutter = $("ed-gutter");
  const area = $("ed-text");
  if (gutter && area) gutter.scrollTop = area.scrollTop;
}

function foldRowEl(row) {
  const button = el("button", "dl-fold");
  button.type = "button";
  button.append(el("span", "dl-fold-icon", "⋯"));
  button.append(el("span", "", `展开 ${row.count} 行未改动`));
  button.onclick = () => {
    state.editor.expanded.add(`${row.from}-${row.to}`);
    renderDiffView();
  };
  return button;
}

/** 一行 → 元素数组：修改行拆成「旧行 + 新行」两行，词级碎片各自高亮 —— 这才读得懂。 */
function diffRowEls(row) {
  const line = (kind, oldNo, newNo, content) => {
    const node = el("div", `dl ${kind}`);
    if (newNo != null) node.dataset.new = String(newNo);
    const num = (value) => {
      const cell = el("span", "dl-n");
      cell.textContent = value == null ? "" : String(value);
      return cell;
    };
    node.append(num(oldNo));
    node.append(num(newNo));
    const code = el("span", "dl-code");
    if (typeof content === "string") code.textContent = content;
    else for (const seg of content) code.append(seg);
    if (!code.childNodes.length) code.append(" ");
    node.append(code);
    return node;
  };

  if (row.kind === "mod" && row.segs) {
    const side = (wanted) => {
      const parts = [];
      for (const seg of row.segs) {
        if (seg.type === "same") parts.push(seg.text);
        else if (seg.type === wanted) parts.push(el("span", `seg-${wanted}`, seg.text));
      }
      return parts;
    };
    return [line("mod-del", row.old, null, side("del")), line("mod-add", null, row.new, side("add"))];
  }
  return [line(row.kind, row.old, row.new, row.text ?? "")];
}

function renderDiffView() {
  const ed = state.editor;
  const box = $("ed-diff");
  if (!ed?.diff || !box) return;
  box.textContent = "";
  if (ed.diff.identical) {
    box.append(
      el("p", "ed-diff-empty", ed.baseMissing ? "这个文件还没进仓库（HEAD 里没有），发布之后才有基线。" : "与 HEAD 一致 —— 这个文件没有改动。"),
    );
    return;
  }
  const meta = el("div", "ed-diff-head");
  meta.append(el("span", "dh-path", ed.path));
  meta.append(el("span", "dh-note", ed.diff.coarse ? "文件较大，只做行级对比" : "与 HEAD 比"));
  meta.append(el("span", "dh-stat", `+${ed.diff.stats.adds} −${ed.diff.stats.dels}`));
  box.append(meta);

  const all = ed.diff.allRows ?? [];
  for (const row of ed.diff.rows) {
    if (row.kind !== "fold") {
      for (const node of diffRowEls(row)) box.append(node);
      continue;
    }
    const key = `${row.from}-${row.to}`;
    if (ed.expanded.has(key)) for (const inner of all.slice(row.from, row.to)) for (const node of diffRowEls(inner)) box.append(node);
    else box.append(foldRowEl(row));
  }
}

/** 右侧那根细条：改动在文件里的位置。点哪儿跳哪儿。 */
function renderOverview() {
  const ed = state.editor;
  const map = $("ed-map");
  if (!map) return;
  const all = ed?.diff?.allRows ?? [];
  if (!all.length || ed.diff.identical) {
    map.hidden = true;
    map.textContent = "";
    return;
  }
  map.hidden = false;
  map.textContent = "";
  const total = Math.max(1, all.length);
  for (let i = 0; i < all.length; ) {
    const kind = all[i].kind;
    if (kind === "ctx") {
      i++;
      continue;
    }
    let j = i;
    while (j < all.length && all[j].kind === kind) j++;
    const flag = el("div", `map-block ${kind}`);
    flag.style.top = `${(i / total) * 100}%`;
    flag.style.height = `${Math.max(1.6, ((j - i) / total) * 100)}%`;
    map.append(flag);
    i = j;
  }
}

function setEditorMode(mode) {
  const ed = state.editor;
  if (!ed) return;
  ed.mode = mode;
  const diff = mode === "diff";
  $("ed-code").hidden = diff;
  $("ed-diff").hidden = !diff;
  $("ed-tab-edit").classList.toggle("on", !diff);
  $("ed-tab-diff").classList.toggle("on", diff);
  $("ed-tab-edit").setAttribute("aria-selected", String(!diff));
  $("ed-tab-diff").setAttribute("aria-selected", String(diff));
  if (diff) {
    recomputeDiff();
    renderDiffView();
    $("ed-diff").focus();
  } else {
    renderGutter();
    $("ed-text").focus();
  }
}

/** Alt+↑/↓：跳到上一处 / 下一处改动。两个视图都给。 */
function gotoChange(dir) {
  const ed = state.editor;
  if (!ed?.diff) return;
  const lines = [...ed.diff.marks.keys()].sort((a, b) => a - b);
  if (!lines.length) return;

  if (ed.mode === "diff") {
    const box = $("ed-diff");
    const nodes = [...box.querySelectorAll(".dl.add, .dl.mod-add")];
    if (!nodes.length) return;
    const middle = box.scrollTop + box.clientHeight / 2;
    const after = nodes.find((node) => node.offsetTop > middle);
    const before = [...nodes].reverse().find((node) => node.offsetTop < middle);
    const target = dir > 0 ? after ?? nodes[0] : before ?? nodes[nodes.length - 1];
    target.scrollIntoView({ block: "center", behavior: "smooth" });
    return;
  }

  const area = $("ed-text");
  const caret = area.value.slice(0, area.selectionStart).split("\n").length;
  const line = dir > 0 ? lines.find((n) => n > caret) ?? lines[0] : [...lines].reverse().find((n) => n < caret) ?? lines[lines.length - 1];
  const textLines = area.value.split("\n");
  const start = textLines.slice(0, line - 1).join("\n").length + (line > 1 ? 1 : 0);
  area.focus();
  area.setSelectionRange(start, start + (textLines[line - 1]?.length ?? 0));
  area.scrollTop = Math.max(0, (line - 1) * ED_LINE() - area.clientHeight / 2);
  syncGutter();
}

// ── 新建扩展 ────────────────────────────────────────────────────────────────
//
// 只问三样：名字、一句话说明、类别。其余全给默认（0.1.0 / index.js / default / [] / {}）。
// 名字是唯一不可逆的决定 —— 会话里存的就是这个名字，改它等于让所有老会话找不到自己，
// 所以校验必须贴着输入框实时跑，而不是等服务端 409 回来才告诉你。
//
// 建完**不发布**：新扩展的第一个正确状态是「未发布」（untracked），用户看了满意再点发布。

const NAME_RE = /^[a-z0-9][a-z0-9._-]*$/i;
const CORE_NAMES = ["nous", "skills"];

/** 名字能不能用 + 还缺什么。tone：ok 绿 / warn 只是还没写完 / bad 挡住。 */
function validateCreate() {
  const name = $("create-name").value.trim();
  const summary = $("create-summary").value.trim();
  const set = (tone, msg) => ({ ok: tone === "ok", tone, msg });
  const slash = state.data?.repo?.path ?? "扩展仓库";

  if (!name) return set("bad", `先起个名字（小写字母开头最好）。会建在 ${slash}/&lt;名字&gt;/`);
  if (CORE_NAMES.includes(name)) return set("bad", "nous / skills 是核心扩展（server.js 顶层写死了 import），名字占了");
  if (!NAME_RE.test(name)) return set("bad", "只能字母数字和 . _ -，而且得字母数字开头");
  if ((state.data?.catalog ?? []).some((ext) => ext.name === name)) return set("bad", `已经有一个叫 ${name} 的了`);
  if ((state.data?.archived ?? []).some((item) => item.name === name))
    return set("bad", `.archive/${name} 还留着 —— 去「已归档」里点恢复，别建第二个`);
  if (!summary) return set("warn", `名字可用。还差一句说明 —— 卡片上就靠它认人`);
  return set("ok", `会建在 ${slash}/<b>${name}</b>/`);
}

function renderCreate() {
  const verdict = validateCreate();
  const note = $("create-name-note");
  note.className = `create-note ${verdict.tone}`;
  note.innerHTML = verdict.msg;
  $("create-ok").disabled = !verdict.ok || state.creating;
  // 页脚只报"会生成哪两个文件"，路径已经在名字下面那句里说了，别重复。
  $("create-where").textContent = "nous.json + index.js · v0.1.0";
  $("create-name").classList.toggle("bad", verdict.tone === "bad" && $("create-name").value.trim() !== "");
}

function openCreate() {
  closeMenu();
  state.creating = false;
  $("create").reset();
  $("create-err").hidden = true;
  // 当前正筛着某个类别，新建时就默认挂上去 —— 少点一次。
  const filter = state.filter && !state.filter.startsWith("__") ? state.filter : "";
  $("create-cat").value = filter;
  const core = state.data?.protected ?? [];
  const cats = [...new Set((state.data?.categories ?? []).filter((name) => !core.includes(name)))];
  $("create-cats").replaceChildren(...cats.map((name) => { const o = el("option"); o.value = name; return o; }));
  $("create-scrim").hidden = false;
  $("create").hidden = false;
  renderCreate();
  $("create-name").focus();
}

function closeCreate() {
  $("create-scrim").hidden = true;
  $("create").hidden = true;
  state.creating = false;
}

async function submitCreate(event) {
  event.preventDefault();
  const verdict = validateCreate();
  if (!verdict.ok) return renderCreate();
  const name = $("create-name").value.trim();
  const summary = $("create-summary").value.trim();
  const category = $("create-cat").value.trim();
  state.creating = true;
  renderCreate();
  $("create-err").hidden = true;
  $("create-ok").textContent = "建…";
  try {
    const result = await post(`${state.base}v1/extensions/create`, {
      name,
      summary,
      category: category ? [category] : [],
    });
    closeCreate();
    await load({ quiet: true });
    const ext = (state.data?.catalog ?? []).find((item) => item.name === name) ?? result.manifest;
    // 装载失败也照样打开：目录在，红卡 + 错误行就在卡上，抽屉里可以立刻改。
    toast(`已建 ${name} · 未发布（只有这台有）`, result.load?.ok === false ? "bad" : "");
    if (state.editor) closeEditor();
    await openExtensionEditor(ext);
  } catch (error) {
    $("create-err").textContent = String(error?.message ?? error);
    $("create-err").hidden = false;
  } finally {
    state.creating = false;
    $("create-ok").textContent = "建";
    if (!$("create").hidden) renderCreate();
  }
}

$("new-ext").onclick = () => openCreate();
$("create-cancel").onclick = () => closeCreate();
$("create-scrim").onclick = () => closeCreate();
$("create").addEventListener("submit", submitCreate);
for (const id of ["create-name", "create-summary", "create-cat"]) {
  const node = $(id);
  node.addEventListener("input", renderCreate);
  node.addEventListener("change", renderCreate);
}
// 回车一格一格往下走：名字 → 说明 → 类别 → 建。别在第一格就提交。
$("create-name").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  $("create-summary").focus();
});
$("create-summary").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  $("create-cat").focus();
});

// 抽屉的交互：Esc 关、Ctrl/⌘+S 保存、Tab 缩进两格（不跳出编辑框）。
$("ed-close").onclick = () => closeEditor();
$("ed-scrim").onclick = () => closeEditor();
$("ed-save").onclick = () => void saveEditorFile();
$("ed-publish").onclick = () => void publishEditor();
$("ed-dev").onclick = () => void openDevSession();
$("ed-text").addEventListener("input", renderEditorState);
$("ed-text").addEventListener("input", scheduleDiff);
$("ed-text").addEventListener("scroll", syncGutter);
$("ed-tab-edit").onclick = () => setEditorMode("edit");
$("ed-tab-diff").onclick = () => setEditorMode("diff");
$("ed-map").onclick = (event) => {
  const ed = state.editor;
  if (!ed?.diff) return;
  const rect = $("ed-map").getBoundingClientRect();
  const at = Math.min(1, Math.max(0, (event.clientY - rect.top) / rect.height));
  const box = ed.mode === "diff" ? $("ed-diff") : $("ed-text");
  box.scrollTop = at * (box.scrollHeight - box.clientHeight);
  syncGutter();
};
$("ed-text").addEventListener("keydown", (event) => {
  if (event.key === "Tab" && !event.metaKey && !event.ctrlKey && !event.altKey) {
    event.preventDefault();
    const area = event.target;
    const { selectionStart: start, selectionEnd: end } = area;
    area.value = area.value.slice(0, start) + "  " + area.value.slice(end);
    area.selectionStart = area.selectionEnd = start + 2;
    renderEditorState();
    return;
  }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
    event.preventDefault();
    void saveEditorFile();
  }
});
addEventListener("keydown", (event) => {
  // 菜单盖在所有层之上（它是最后一次交互）：Esc 先关它。
  if (event.key === "Escape" && menuOpen) {
    closeMenu();
    return;
  }
  if (event.key === "Escape" && !$("dup").hidden) {
    closeDuplicate();
    return;
  }
  // 未同步弹窗是最外一层（顶栏开的），Esc 先关它。
  if (event.key === "Escape" && !$("pending").hidden) {
    closePending();
    return;
  }
  // 新建层盖在最上面：Esc 先关它，别顺手把背后的抽屉也关了。
  // 变体弹层是从卡片上开的，比新建层更靠外：Esc 先关它。
  if (event.key === "Escape" && !$("variant").hidden) {
    closeVariant();
    return;
  }
  if (event.key === "Escape" && !$("create").hidden) {
    closeCreate();
    return;
  }
  if (!state.editor) {
    // 详情是主区、不是浮层：Esc 退回列表。
    if (event.key === "Escape" && state.detail) closeDetail();
    return;
  }
  if (event.altKey && event.key.toLowerCase() === "d") {
    event.preventDefault();
    setEditorMode(state.editor.mode === "diff" ? "edit" : "diff");
  } else if (event.altKey && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
    event.preventDefault();
    gotoChange(event.key === "ArrowDown" ? 1 : -1);
  } else if (event.key === "Escape") {
    if (state.editor.mode === "diff") setEditorMode("edit");
    else closeEditor();
  }
});
// 有没保存的字就别让浏览器静默关掉这一页。
addEventListener("beforeunload", (event) => {
  if (!editorDirty()) return;
  event.preventDefault();
  event.returnValue = "";
});

load();
