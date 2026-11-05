/**
 * skill 编目 · 店面。
 *
 * 设计照抄 nw-panel SourcePane 的 #205 店面裁定（那一份被测试钉着，是原图）：
 *   - 网格主体 = 源（仓库）卡：一仓一卡 —— owner 视觉（这里只走字母砖，#214 的教训：
 *     默认头像=给 github.com 送浏览记录；warehouse 的 /api/avatar 代理在 j1900 上今天
 *     也是 504，等它通了再谈 opt-in）+ 物种角标 + 名字 / 类别 / 仓库 / N 条目 / 版本 / 标签色点。
 *   - 抽屉 = 源详情：字段 + 条目列表（>4 条给内搜）+ **条目级标签**（ref 两段 id
 *     `仓库/条目`，整段 percent-encode —— #20/#184 的形状）。
 *   - facets = 物种 / 类别树（根+叶，前缀语义）/ 标签；**计数对全量算**（facet 是导航，
 *     不是结果统计）；类别与标签多选。
 *   - 对账（recheck）是独立报告视图，把店面整个让出来；400 + 漂移照样渲染报告。
 *
 * 写路径与 nw-panel 同一批 REST：register 两段式 / category 单步直写 / rm 两步确认 /
 * tag 增摘。语义闸全在上游，本面板不复刻 kind 值域与类别词法。
 */

// ── 小工具 ─────────────────────────────────────────────────────────────

const el = (tag, props = {}, ...kids) => {
  const n = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v == null || v === false) continue;
    if (k === "class") n.className = v;
    else if (k === "text") n.textContent = v;
    else if (k.startsWith("on") && typeof v === "function") n.addEventListener(k.slice(2).toLowerCase(), v);
    else if (k === "style") Object.assign(n.style, v);
    else if (k in n) n[k] = v;
    else n.setAttribute(k, v);
  }
  for (const kid of kids.flat(9)) {
    if (kid == null || kid === false) continue;
    n.append(kid.nodeType ? kid : document.createTextNode(String(kid)));
  }
  return n;
};

const $ = (sel) => document.querySelector(sel);

const enc = (s) => encodeURIComponent(s);

/**
 * 块可能挂在任何前缀下：Nous 里是 `/p/<board>/<id>/`，`cp -r` 出去自己跑就是 `/`。
 * 请求一律相对这块自己的根 —— 绝对路径 `/api/...` 会越出前缀打到宿主，
 * 拿回来的是宿主那张 200 的 index.html（Nous 的 SPA 回退）。
 */
const ROOT = (() => {
  const p = location.pathname;
  return p.endsWith("/") ? p : p + "/";
})();
const api = (path) => ROOT + "api" + path;

/** append() / replaceChildren() 会把 null 折成字符串 "null" —— 这里挡一道。 */
const mount = (parent, ...kids) => {
  parent.replaceChildren(
    ...kids.flat(9).filter((k) => k != null && k !== false).map((k) => (k.nodeType ? k : document.createTextNode(String(k)))),
  );
  return parent;
};

const repoURL = (repo) => {
  const r = String(repo || "").trim();
  if (/^https?:\/\//i.test(r)) return r.replace(/\.git$/i, "");
  if (/^git@github\.com:/i.test(r)) return "https://github.com/" + r.slice("git@github.com:".length).replace(/\.git$/i, "");
  if (/^git@[^:]+:/i.test(r)) return "";
  if (/^[\w.-]+\/[\w.-]+$/.test(r)) return "https://github.com/" + r;
  return "";
};

const shortRepo = (repo) => {
  const u = repoURL(repo);
  return u ? u.replace(/^https?:\/\/(www\.)?github\.com\//i, "").replace(/\/$/, "") : String(repo || "");
};

const entryURL = (repo, p) => {
  const u = repoURL(repo);
  if (!u || !p) return "";
  return u + "/tree/HEAD/" + String(p).split("/").map(enc).join("/");
};

const parseCategory = (s) =>
  String(s || "")
    .split(/[\/,，、]+/)
    .map((x) => x.trim())
    .filter(Boolean);

/** 名字哈希色：同一名字永远同一色。nw-panel visual.ts 的 10 色盘。 */
const PALETTE = ["#4f6df5", "#0ea5a4", "#e0663c", "#8b5cf6", "#d946ef", "#f59e0b", "#10b981", "#3b82f6", "#ef4444", "#64748b"];
const hue = (name) => {
  let h = 0;
  for (const c of String(name)) h = (h * 31 + c.codePointAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length];
};

const monogram = (name) => (String(name || "?").replace(/^[\W_]+/, "").slice(0, 2) || "?").toUpperCase();

/** 标签色点：标签名 → 稳定颜色。 */
const dot = (t) => el("span", { class: "dot", style: { background: hue(t) }, title: t });

// ── 状态 ───────────────────────────────────────────────────────────────

const S = {
  config: null,
  nwVersion: "",
  sources: [],
  tags: [],
  q: "",
  selCats: new Set(),
  selTags: new Set(),
  sort: "name", // name | entries
  drawer: null, // 源名
  entryQ: "", // 抽屉里的条目内搜索
  lastReport: {}, // 源名 -> 最近一次单源对账 envelope
  upstream: null, // { at, ttl, repos: { <repo>: { latest, tags, semverTags, checkedAt, error } } }
  upstreamBusy: false,
  mode: "store", // store | report
  report: null, // { data, scope }
  busy: "",
  loading: true,
  loaded: false,
  loadError: "",
  lastRefresh: null,
};

const notify = (msg, kind = "") => {
  const t = el("div", { class: "toast" + (kind ? " toast-" + kind : "") }, msg);
  $("#toasts").append(t);
  setTimeout(() => t.remove(), kind === "bad" ? 12000 : 6000);
};

const errLine = (env) =>
  (env?.errors || [])
    .map((e) => (e?.message || String(e)) + (e?.hint ? "\n→ " + e.hint : ""))
    .join("\n");

// ── 上游调用 ───────────────────────────────────────────────────────────

async function nw(path, { method = "GET", body } = {}) {
  let res;
  try {
    res = await fetch(api("/nw" + path), {
      method,
      headers: body === undefined ? undefined : { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch (e) {
    return { status: 0, env: { ok: false, errors: [{ message: "本地面板进程没回话：" + e.message }], data: {} } };
  }
  const text = await res.text();
  let env;
  try {
    env = JSON.parse(text);
  } catch {
    const looksHTML = /<(!doctype\s+html|html[\s>])/i.test(text.slice(0, 400));
    env = {
      ok: false,
      errors: [
        {
          message: looksHTML
            ? `请求没到 warehouse，拿回来的是宿主的 HTML（${res.status}）。这说明 /api 路径没带块的前缀 —— 打到了 Nous 自己身上。`
            : `上游回了非 JSON（${res.status}）：${text.slice(0, 200)}`,
        },
      ],
      data: {},
    };
  }
  return { status: res.status, ms: res.headers.get("x-nw-ms"), env };
}

async function cfgGet() {
  const r = await fetch(api("/config"));
  return r.json();
}

async function cfgPut(body) {
  const r = await fetch(api("/config"), { method: "PUT", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
  const text = await r.text();
  let env;
  try {
    env = JSON.parse(text);
  } catch {
    env = { ok: false, errors: [{ message: text.slice(0, 200) }] };
  }
  return { status: r.status, env };
}

async function runOp(label, fn) {
  S.busy = label;
  renderTopbar();
  try {
    return await fn();
  } finally {
    S.busy = "";
    render();
  }
}

async function loadSources() {
  const [{ env }, tags] = await Promise.all([nw("/source"), nw("/tags")]);
  if (!env.ok) throw new Error(errLine(env) || "读源编目失败");
  S.nwVersion = env.nw || "";
  S.sources = (env.data?.sources || []).filter((s) => s.kind === "skill");
  S.tags = tags.env?.ok ? tags.env.data?.tags || [] : [];
  S.lastRefresh = new Date().toTimeString().slice(0, 8);
}

async function refresh() {
  try {
    await loadSources();
    S.loadError = "";
  } catch (e) {
    S.loadError = String(e.message || e);
  }
  S.loading = false;
  S.loaded = true;
  // 上游那一列跟编目无关：自己查自己的，从来没查过的顺手补一次（不阻塞界面）。
  await upstreamLoad();
  void upstreamCheck(reposOf(S.sources), { quiet: true });
}

/* ── 上游版本：两个数字，两套口径，绝不合并成一个「版本」 ──────────────────
 *
 *   账上 = nw 编目里那条 `version`，注册那一刻抓的 latest **release** 的 tag，会过期。
 *   上游 = 本块进程跑 `git ls-remote --tags` 查到的最新 **semver tag**（面板的 /api/upstream）。
 *
 * 为什么不由 nw 给：nw 的 source recheck 也报版本漂移，但它在 j1900 上今天是红的（真 clone
 * 上游，那台机器出网坏）。本块跑在 Nous 这台机器上，出网正常，一条仓库 ls-remote 一次 ~1 秒，
 * 而且 git 协议不占 api.github.com 那 60 次/小时（整台机器共享，页面开几次就穿）。
 */

const vtuple = (v) => {
  const m = /^v?(\d+)\.(\d+)\.(\d+)/.exec(String(v || "").trim());
  return m ? [Number(m[1]), Number(m[2]), Number(m[3])] : null;
};

/** 两个版本号比大小。任一边不是 semver 就返回 null —— 不比，也就不许说「落后」。 */
const vcmp = (a, b) => {
  const x = vtuple(a);
  const y = vtuple(b);
  if (!x || !y) return null;
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i] ? 1 : -1;
  return 0;
};

const upstreamOf = (repo) => S.upstream?.repos?.[repo] || null;

/** 一张卡 / 一行抽屉上的「账上 → 上游」版本对。 */
function versionPair(s) {
  const up = upstreamOf(s.repo);
  const book = s.version || "";
  const lines = [book ? `账上 ${book}（nw 登记那一刻的 latest release）` : "账上没记（没抓到 release / 存量记录）"];

  if (!up) return el("span", { class: "chip", title: lines.join("\n") + "\n上游：还没查过", text: book || "无版本" });

  const age = up.checkedAt ? `查于 ${new Date(up.checkedAt).toLocaleString("zh-CN", { hour12: false })}` : "";
  if (up.error) {
    lines.push(`上游：查不到 —— ${up.error}`);
    return el("span", { class: "chip", title: lines.join("\n") + "\n" + age, text: `${book || "?"} → ?` });
  }
  if (!up.latest) {
    lines.push(`上游：没有 semver 的 tag（共 ${up.tags} 个 tag）`);
    return el("span", { class: "chip", title: lines.join("\n") + "\n" + age, text: `${book || "?"} → 未打 tag` });
  }
  lines.push(`上游 ${up.latest}（最新 semver tag，共 ${up.tags} 个 tag）`);

  const cmp = vcmp(book, up.latest);
  // 顺序要紧：账上压根没记 ≠ 口径对不上，那是两种病（一个去重新登记，一个去查上游怎么打的 tag）。
  if (!book) {
    lines.push("账上没记，上游有 —— 该重新登记了");
    return el("span", { class: "chip chip-warn", title: lines.join("\n") + "\n" + age, text: `未记 → ${up.latest}` });
  }
  if (cmp === 0) return el("span", { class: "chip chip-ok", title: lines.join("\n") + "\n" + age, text: `${book} ✓` });
  if (cmp === null) {
    // 一边不是 semver：摆着看，不判落后。
    lines.push("口径对不上（有一边不是 semver），不比");
    return el("span", { class: "chip", title: lines.join("\n") + "\n" + age, text: `${book} · 上游 ${up.latest}` });
  }
  return el("span", { class: "chip chip-warn", title: lines.join("\n") + "\n" + age, text: `${book} → ${up.latest}` });
}

/** 抽屉里那一行：上游最新 tag + 状态。落后才给「重新登记」—— 写账的闸还在 nw 那儿。 */
function versionField(s) {
  const up = upstreamOf(s.repo);
  const book = s.version || "";
  const head = el("span", { class: "row row-tight" });
  if (!up) {
    head.append(el("span", { class: "muted", text: "还没查过" }));
    return field("上游最新", head);
  }
  if (up.error) {
    head.append(
      el("span", { class: "chip", title: up.error, text: "查不到" }),
      el("span", { class: "muted", text: up.error.slice(0, 60) }),
    );
    return field("上游最新", head);
  }
  if (!up.latest) {
    head.append(el("span", { class: "muted", text: `没有 semver 的 tag（共 ${up.tags} 个）` }));
    return field("上游最新", head);
  }

  const cmp = vcmp(book, up.latest);
  const chip =
    !book
      ? el("span", { class: "chip chip-warn", text: "账上没记" })
      : cmp === 0
        ? el("span", { class: "chip chip-ok", text: "一致" })
        : cmp === null
          ? el("span", { class: "chip", text: "口径不同，不比" })
          : cmp < 0
            ? el("span", { class: "chip chip-warn", text: "账上落后" })
            : el("span", { class: "chip chip-accent", text: "账上比上游新" });

  head.append(
    el("span", { class: "mono", text: up.latest }),
    chip,
    el("span", { class: "hint", text: `共 ${up.tags} 个 tag` }),
    el("div", { class: "spacer" }),
    cmp !== 0 ? reregisterBtn(s.name) : null,
  );
  return field("上游最新", head);
}

async function upstreamLoad() {
  try {
    const res = await fetch(api("/upstream"));
    S.upstream = await res.json();
  } catch {
    // 查不到上游不影响编目本身：页面照常摆账上的那一列。
  }
}

/** 按仓库去重：`matt` 和 `mattpocock` 是同一条仓库的两份账，别查两遍。 */
const reposOf = (sources) => [...new Set((sources || []).map((s) => s.repo).filter(Boolean))];

async function upstreamCheck(repos, { force = false, quiet = false } = {}) {
  const wanted = [...new Set((repos || []).filter(Boolean))];
  if (!wanted.length) return;
  // 不 force 的时候只补「从来没查过的」：TTL 过期由人点按钮，别每次开页面就打 GitHub。
  const todo = force ? wanted : wanted.filter((r) => !upstreamOf(r));
  if (!todo.length) return;
  S.upstreamBusy = true;
  render();
  try {
    const res = await fetch(api("/upstream"), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ repos: todo, force }),
    });
    S.upstream = await res.json();
    const bad = todo.filter((r) => upstreamOf(r)?.error);
    if (!quiet) {
      if (bad.length) notify(`上游查了 ${todo.length} 条仓库，${bad.length} 条查不到（卡片上是 → ?）`, "warn");
      else notify(`上游 tag 查完：${todo.length} 条仓库`, "ok");
    }
  } catch (e) {
    if (!quiet) notify("查上游失败：" + e.message, "bad");
  } finally {
    S.upstreamBusy = false;
    render();
  }
}

const sourceByName = (name) => S.sources.find((s) => s.name === name) || null;

// 标签 join：ref kind=skill，id 一段（源级）或两段（条目级 `仓库/条目`）。
const tagsOf = (id) => {
  const hit = new Map();
  for (const t of S.tags) {
    const mine = (t.refs || []).some((r) => r.kind === "skill" && (r.id === id || String(r.id) === `skill/${id}`));
    if (mine && !hit.has(t.name)) hit.set(t.name, t);
  }
  return [...hit.keys()];
};

const allTagsOfSource = (s) => {
  const seen = new Set(tagsOf(s.name));
  for (const e of s.entries || []) for (const t of tagsOf(`${s.name}/${e.name}`)) seen.add(t);
  return [...seen];
};

// ── facets（计数对全量算 —— facet 是导航，不是结果统计）───────────────

const catPath = (s) => ((s.category || []).length ? s.category.join("/") : "未分类");

function facetRow(label, n, on, onPick, child = false) {
  return el(
    "button",
    { class: "facet" + (on ? " on" : "") + (child ? " child" : ""), type: "button", "aria-pressed": String(on), onclick: onPick },
    el("span", { class: "facet-label", text: label }),
    el("span", { class: "facet-n mono", text: String(n) }),
  );
}

function renderRail() {
  const rail = $("#rail");
  const rows = S.sources;

  // 类别树：根（第一级）→ 叶（第二级起合并成一段）。根 n = 自身 + 全部叶子。
  const roots = new Map();
  for (const s of rows) {
    const parts = catPath(s).split("/");
    const root = parts[0];
    const node = roots.get(root) || { self: 0, leaves: new Map() };
    if (parts.length === 1) node.self++;
    else {
      const leaf = parts.slice(1).join("/");
      node.leaves.set(leaf, (node.leaves.get(leaf) || 0) + 1);
    }
    roots.set(root, node);
  }

  // 标签计数：带此标签的源数（一源多标签只计一次；源级 + 条目级同账并提）。
  const tagCounts = new Map();
  for (const s of rows) for (const t of allTagsOfSource(s)) tagCounts.set(t, (tagCounts.get(t) || 0) + 1);

  mount(
    rail,
    el("h3", { text: "类别" }),
    roots.size
      ? el(
          "div",
          { class: "facet-row-group" },
          ...[...roots.entries()].sort((a, b) => a[0].localeCompare(b[0])).flatMap(([root, node]) => {
            const rootN = node.self + [...node.leaves.values()].reduce((a, b) => a + b, 0);
            return [
              facetRow(root, rootN, S.selCats.has(root), () => {
                S.selCats.has(root) ? S.selCats.delete(root) : S.selCats.add(root);
                render();
              }),
              ...[...node.leaves.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([leaf, n]) =>
                facetRow(leaf, n, S.selCats.has(`${root}/${leaf}`), () => {
                  const full = `${root}/${leaf}`;
                  S.selCats.has(full) ? S.selCats.delete(full) : S.selCats.add(full);
                  render();
                }, true),
              ),
            ];
          }),
        )
      : el("div", { class: "hint", style: { padding: "0 8px" } }, "（没有类别）"),
    el("h3", { text: "标签" }),
    tagCounts.size
      ? el(
          "div",
          { class: "facet-row-group" },
          ...[...tagCounts.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).map(([t, n]) =>
            facetRow(t, n, S.selTags.has(t), () => {
              S.selTags.has(t) ? S.selTags.delete(t) : S.selTags.add(t);
              render();
            }),
          ),
        )
      : el("div", { class: "hint", style: { padding: "0 8px" } }, "（还没有标签）"),
    S.selCats.size || S.selTags.size || S.q
      ? el("button", { class: "btn btn-sm btn-ghost clear-facets", type: "button", text: "清除全部筛选", onclick: () => { S.selCats.clear(); S.selTags.clear(); S.q = ""; $("#q").value = ""; render(); } })
      : null,
    el("div", { class: "side-foot hint", text: S.lastRefresh ? `已刷新 ${S.lastRefresh}` : "未刷新" }),
  );
}

// ── 店面（网格 = 源卡）────────────────────────────────────────────────

function matchSource(s) {
  const q = S.q.trim().toLowerCase();
  if (S.selCats.size > 0 && ![...S.selCats].some((c) => catPath(s) === c || catPath(s).startsWith(c + "/"))) return false;
  if (S.selTags.size > 0) {
    const tags = new Set(allTagsOfSource(s));
    if (![...S.selTags].some((t) => tags.has(t))) return false;
  }
  if (q) {
    const hay = `${s.name} ${s.repo || ""} ${(s.entries || []).map((e) => e.name).join(" ")}`.toLowerCase();
    if (!hay.includes(q)) return false;
  }
  return true;
}

function sortRows(rows) {
  const out = [...rows];
  if (S.sort === "entries") out.sort((a, b) => (b.entries || []).length - (a.entries || []).length || a.name.localeCompare(b.name));
  else out.sort((a, b) => a.name.localeCompare(b.name));
  return out;
}

/** 字母砖封面（#214 的裁定：默认不外呼；这里的 owner 头像等 warehouse 代理通了再说）。 */
function visual(s, cls = "visual") {
  return el(
    "div",
    { class: cls, style: { background: hue(s.name) } },
    el("span", { class: "visual-mono", text: monogram(s.name) }),
  );
}

function sourceCard(s) {
  const tags = allTagsOfSource(s);
  const drift = S.recheckDriftCount?.(s.name) || 0;
  return el(
    "article",
    {
      class: "card-item" + (S.drawer === s.name ? " selected" : ""),
      role: "button",
      tabindex: "0",
      onclick: () => {
        S.drawer = s.name;
        S.entryQ = "";
        render();
      },
      onkeydown: (ev) => {
        if (ev.key === "Enter" || ev.key === " ") {
          ev.preventDefault();
          S.drawer = s.name;
          S.entryQ = "";
          render();
        }
      },
    },
    el(
      "div",
      { class: "visual-wrap" },
      visual(s),
      el("span", { class: "badge skill", text: s.kind }),
    ),
    el(
      "div",
      { class: "card-body" },
      el("div", { class: "card-name", title: s.name, text: s.name }),
      el("div", { class: "card-cat muted", text: catPath(s) }),
      el("div", { class: "card-repo mono", title: s.repo || "", text: shortRepo(s.repo) }),
    ),
    el(
      "div",
      { class: "card-foot" },
      el("span", { class: "entries-n" }, el("b", { text: String((s.entries || []).length) }), " 条目"),
      versionPair(s),
      drift ? el("span", { class: "chip chip-bad", title: "上次对账的漂移数", text: `${drift} 漂移` }) : null,
      tags.length ? el("span", { class: "dots" }, ...tags.map(dot)) : null,
    ),
  );
}

function renderStore(host) {
  const rows = sortRows(S.sources.filter(matchSource));
  const entryTotal = rows.reduce((a, s) => a + (s.entries || []).length, 0);

  host.append(
    el(
      "div",
      { class: "store-main" },
      el(
        "div",
        { class: "store-top" },
        el(
          "div",
          { class: "store-count muted" },
          el("b", { text: String(rows.length) }),
          ` / ${S.sources.length} 源 · ${entryTotal} 条目`,
          S.loaded ? "" : " · 加载中…",
        ),
        el("div", { class: "spacer" }),
        el("button", {
          class: "btn btn-sm",
          type: "button",
          disabled: S.upstreamBusy,
          title: "git ls-remote --tags 查每条源的上游最新 semver tag（只查不写编目）",
          text: S.upstreamBusy ? "查上游…" : "检查上游",
          onclick: () => upstreamCheck(reposOf(S.sources), { force: true }),
        }),
        el(
          "label",
          { class: "sort" },
          el("span", { class: "hint", text: "排序" }),
          el(
            "select",
            {
              onchange: (ev) => {
                S.sort = ev.target.value;
                render();
              },
            },
            el("option", { value: "name", selected: S.sort === "name", text: "名字 A→Z" }),
            el("option", { value: "entries", selected: S.sort === "entries", text: "条目数多→少" }),
          ),
        ),
      ),
      S.loading
        ? el("div", { class: "empty" }, el("span", { class: "spinner" }), " 读编目中…")
        : S.loadError
          ? el("div", { class: "card drift" }, el("strong", { text: "读不到 warehouse" }), el("div", { style: { marginTop: "8px", whiteSpace: "pre-wrap" } }, S.loadError), el("div", { class: "hint", style: { marginTop: "8px" } }, "点右上角 ⚙ 改 base，或确认 tailnet 通着。"))
          : rows.length
            ? el("div", { class: "grid" }, ...rows.map(sourceCard))
            : el(
                "div",
                { class: "empty", style: { padding: "60px 0", textAlign: "center" } },
                el("div", { text: S.sources.length === 0 ? "还没有登记过的源。" : "没有命中的源 —— 试试放宽筛选。" }),
                S.sources.length === 0
                  ? el("button", { class: "btn btn-primary", type: "button", text: "＋ 登记第一个源", style: { marginTop: "10px" }, onclick: openRegister })
                  : el("button", { class: "btn", type: "button", text: "清除全部筛选", style: { marginTop: "10px" }, onclick: () => { S.selCats.clear(); S.selTags.clear(); S.q = ""; $("#q").value = ""; render(); } }),
              ),
    ),
  );
}

// ── 对账报告（独立视图，店面让位）────────────────────────────────────

/** 版本漂移那一行的「重新登记」入口：打开同一个预览弹窗，把账面那一份预填进去。
 *  写路径不变 —— 还是 register 两段式，闸还是那道闸，只是不用手抄。 */
function reregisterBtn(sourceName) {
  const s = sourceByName(sourceName);
  if (!s) return null;
  return el(
    "button",
    {
      class: "btn btn-sm btn-accent",
      type: "button",
      text: "重新登记",
      title: "预填仓库 / 名字 / 类别，走 register 覆盖写刷新条目与版本",
      onclick: (ev) => {
        ev.stopPropagation();
        openRegister({ name: s.name, repo: s.repo, category: (s.category || []).filter(Boolean) });
      },
    },
  );
}

function statusChip(status) {
  const map = {
    pass: ["chip-ok", "通过"],
    fail: ["chip-bad", "失败"],
    skip: ["chip", "跳过"],
    added: ["chip-warn", "仓库有 · 编目无"],
    removed: ["chip-bad", "编目有 · 仓库无"],
    renamed: ["chip-warn", "改名了"],
    path_changed: ["chip-warn", "搬家了"],
    version: ["chip-accent", "版本落后"],
  };
  const [cls, label] = map[status] || ["chip", status];
  return el("span", { class: cls, text: label });
}

function renderReport(host) {
  const d = S.report?.data || {};
  const drift = d.drift || [];
  const checked = d.checked || [];
  const scope = S.report?.scope;
  host.append(
    el(
      "div",
      { class: "store-main" },
      el(
        "div",
        { class: "report-head" },
        el("strong", { text: drift.length ? `对账报告：检出 ${drift.length} 条漂移` : "对账报告：账与上游一致" }),
        el("span", { class: "hint", text: scope ? `范围：${scope}` : "范围：全部已登记源" }),
        el("div", { class: "spacer" }),
        el("button", { class: "btn btn-sm", type: "button", disabled: Boolean(S.busy), text: "再跑一次", onclick: () => recheckRun(scope ?? undefined) }),
        el("button", { class: "btn btn-sm btn-ghost", type: "button", text: "回到店面", onclick: () => { S.report = null; S.mode = "store"; render(); } }),
      ),
      el(
        "div",
        { class: "card" },
        el("div", { class: "row" }, ...checked.map((c) => el("span", { class: "chip", text: [c.kind, c.name, `${c.entries} 条目`, c.commit ? String(c.commit).slice(0, 7) : ""].filter(Boolean).join(" · ") }))),
        !checked.length && !drift.length ? el("div", { class: "muted", text: "没有扫到任何源（编目是空的，或取回全部失败）。" }) : null,
      ),
      drift.length
        ? el(
            "div",
            { class: "card drift" },
            el(
              "ul",
              {},
              ...drift.map((x) =>
                el(
                  "li",
                  { class: x.kind === "version" ? "has-act" : "" },
                  statusChip(x.kind),
                  " ",
                  el("span", { class: "mono", text: x.entry || x.source || "" }),
                  x.detail ? el("span", { class: "muted", text: " · " + x.detail }) : null,
                  x.path && !x.detail ? el("span", { class: "dim mono", text: " @" + x.path }) : null,
                  el("span", { class: "spacer" }),
                  x.kind === "version" ? reregisterBtn(x.source) : null,
                ),
              ),
            ),
            el("div", { class: "hint", style: { marginTop: "8px" } }, "修法只有一个：重跑 register --yes 覆盖写（分类和删源都替不了它版本落后那条右侧有现成入口。"),
          )
        : null,
      // ok:false 且没有漂移 = 真事故（取回失败 / 编目坏）—— 不拿「无漂移」掩盖它。
      S.report?.env?.ok === false && !drift.length
        ? el("div", { class: "card drift", style: { whiteSpace: "pre-wrap" } }, errLine(S.report.env))
        : null,
    ),
  );
}

// ── 抽屉：源详情 ───────────────────────────────────────────────────────

function drawerBody(s) {
  const wrap = el("div", { class: "drawer-sections" });
  const repo = repoURL(s.repo);

  wrap.append(
    el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "row" },
        visual(s, "visual visual-sm"),
        el(
          "div",
          { style: { minWidth: 0 } },
          el("div", { class: "card-name", text: s.name }),
          repo ? el("a", { href: repo, target: "_blank", rel: "noreferrer", text: shortRepo(s.repo), style: { fontSize: "12px", wordBreak: "break-all" } }) : el("div", { class: "muted", text: s.repo || "—" }),
        ),
      ),
    ),
  );

  // 字段
  wrap.append(
    el(
      "div",
      { class: "card" },
      field("kind", el("span", { class: "mono", text: s.kind })),
      field(
        "类别",
        el(
          "span",
          { class: "row row-tight" },
          (s.category || []).filter(Boolean).length
            ? (s.category || []).filter(Boolean).map((c) => el("span", { class: "chip chip-accent", text: c }))
            : el("span", { class: "muted", text: "（未分类）" }),
          el("button", { class: "btn btn-sm", type: "button", disabled: Boolean(S.busy), text: "改类别", onclick: (ev) => categoryEditor(s, ev.target.closest(".card")) }),
        ),
      ),
      field(
        "账上版本",
        s.version
          ? el("span", { class: "mono", title: "nw 注册那一刻上游最新 release 的 tag（账面情报，不是装机版本 —— 装机真相源是锁定集）", text: s.version })
          : el("span", { class: "muted", text: "（无：没抓到 release / 存量记录 / 降级过 —— 不参与版本比对）" }),
      ),
      versionField(s),
      field(
        "检查于",
        el(
          "span",
          { class: "row row-tight" },
          el("span", { class: "muted", text: upstreamOf(s.repo)?.checkedAt ? new Date(upstreamOf(s.repo).checkedAt).toLocaleString("zh-CN", { hour12: false }) : "还没查过" }),
          el("div", { class: "spacer" }),
          el("button", {
            class: "btn btn-sm",
            type: "button",
            disabled: Boolean(S.busy) || S.upstreamBusy,
            text: S.upstreamBusy ? "查中…" : "再查一次",
            title: "git ls-remote --tags 一次，不写编目",
            onclick: () => upstreamCheck([s.repo], { force: true }),
          }),
        ),
      ),
      field("源标签", tagEditor(tagsOf(s.name), (next) => tagDiff(tagsOf(s.name), next, s.name))),
    ),
  );

  // 对账
  wrap.append(
    el("div", { class: "section-head", text: "上游对账" }),
    S.lastReport?.[s.name]
      ? reportCard(s.name, S.lastReport[s.name])
      : el("div", { class: "card row" }, el("span", { class: "muted", text: "还没扫过。" }), el("span", { class: "hint", text: "recheck 真 clone 上游、只报告不写账。" })),
    el("button", { class: "btn", type: "button", disabled: Boolean(S.busy), style: { marginTop: "8px" }, text: S.busy === "recheck:" + s.name ? "重扫中…" : "对账此源", onclick: () => recheckRun(s.name) }),
  );

  // 条目
  wrap.append(
    el("div", { class: "section-head", text: `条目（${(s.entries || []).length}）` }),
    el("div", { class: "card" }, entriesBlock(s)),
  );

  // 危险区
  wrap.append(
    el("div", { class: "section-head", text: "危险区" }),
    el(
      "div",
      { class: "card" },
      el(
        "div",
        { class: "row" },
        el("span", { class: "hint", text: `删掉整条源记录（${(s.entries || []).length} 个条目一起从读侧消失；其下标签引用悬空，等下一次 tag 写操作清理）。` }),
        el("div", { class: "spacer" }),
        el("button", { class: "btn btn-danger btn-sm", type: "button", disabled: Boolean(S.busy), text: "删除此源", onclick: () => confirmDelete(s) }),
      ),
    ),
  );

  return wrap;
}

const field = (k, v) =>
  el(
    "div",
    { class: "field" },
    el("span", { class: "k", text: k }),
    el("span", { class: "v", style: { minWidth: 0 } }, v),
  );

function categoryEditor(s, card) {
  const current = (s.category || []).filter(Boolean);
  const input = el("input", {
    type: "text",
    value: current.join(" / "),
    placeholder: "多级用 / 分隔；清空 = 未分类",
    style: { flex: "1 1 160px" },
  });
  const row = el(
    "div",
    { class: "row row-tight", style: { marginTop: "8px" } },
    input,
    el("button", {
      class: "btn btn-sm",
      type: "button",
      text: "保存",
      onclick: () => saveCategory(s.name, parseCategory(input.value)),
    }),
    el("button", { class: "btn btn-sm btn-ghost", type: "button", text: "取消", onclick: () => render() }),
  );
  card.querySelector(".v").append(row);
  input.focus();
}

/** TagInput 的差量语义照抄 nw-panel：全量列表进出，这里翻成一次 add / rm 调用。 */
function tagEditor(tags, onDiff) {
  const input = el("input", {
    type: "text",
    placeholder: "＋ 标签，回车",
    style: { width: "130px" },
    onkeydown: (ev) => {
      if (ev.key !== "Enter") return;
      const v = ev.target.value.trim();
      if (v && !tags.includes(v)) onDiff([...tags, v]);
    },
  });
  return el(
    "span",
    { class: "row row-tight" },
    tags.length ? tags.map((t) => el("span", { class: "chip chip-accent" }, t, el("button", { type: "button", title: "摘掉", text: "×", onclick: () => onDiff(tags.filter((x) => x !== t)) }))) : el("span", { class: "muted", text: "（无）" }),
    input,
  );
}

function tagDiff(before, after, id) {
  const added = after.find((t) => !before.includes(t));
  if (added) return addTag(id, added);
  const removed = before.find((t) => !after.includes(t));
  if (removed) return removeTag(id, removed);
}

function entriesBlock(s) {
  const q = (S.entryQ || "").trim().toLowerCase();
  const entries = (s.entries || []).filter((e) => !q || `${e.name} ${e.path}`.toLowerCase().includes(q));

  const block = el("div", {});
  if ((s.entries || []).length > 4) {
    block.append(
      el("input", {
        type: "text",
        placeholder: "在条目里搜…",
        value: S.entryQ || "",
        style: { width: "100%", marginBottom: "8px" },
        oninput: (ev) => {
          S.entryQ = ev.target.value;
          // 只重画条目块，别把整个抽屉重画 —— 输入框焦点会丢。
          const fresh = entriesBlock(s);
          ev.target.closest(".card").replaceWith(fresh);
          fresh.querySelector("input")?.focus();
          fresh.querySelector("input").setSelectionRange(fresh.querySelector("input").value.length, fresh.querySelector("input").value.length);
        },
      }),
    );
  }

  if (!(s.entries || []).length) {
    block.append(el("div", { class: "muted", text: "（无条目）" }));
    return block;
  }
  if (!entries.length) {
    block.append(el("div", { class: "muted", text: "没有命中的条目" }));
    return block;
  }

  block.append(
    el(
      "ul",
      { class: "source-entries" },
      ...entries.map((e) =>
        el(
          "li",
          {},
          el(
            "div",
            { class: "entry-line" },
            entryURL(s.repo, e.path)
              ? el("a", { href: entryURL(s.repo, e.path), target: "_blank", rel: "noreferrer", text: e.name })
              : el("span", { class: "name", text: e.name }),
            el("code", { class: "ref mono", text: e.path || "" }),
          ),
          el("div", { class: "tag-row", style: { marginTop: "4px" } }, tagEditor(tagsOf(`${s.name}/${e.name}`), (next) => tagDiff(tagsOf(`${s.name}/${e.name}`), next, `${s.name}/${e.name}`))),
        ),
      ),
    ),
  );
  return block;
}

function reportCard(name, env) {
  const drift = env?.data?.drift || [];
  const checked = env?.data?.checked || [];
  const box = el("div", { class: "card drift" + (env?.ok && !drift.length ? " drift-ok" : "") });
  mount(
    box,
    el(
      "div",
      { class: "row" },
      el("strong", { text: env?.ok && !drift.length ? "对账：没有漂移" : `对账：检出 ${drift.length} 条漂移` }),
      el("div", { class: "spacer" }),
      ...checked.map((c) => el("span", { class: "chip", text: [c.kind, c.name, `${c.entries} 条目`, c.commit ? String(c.commit).slice(0, 7) : ""].filter(Boolean).join(" · ") })),
    ),
    env?.ok === false && !drift.length ? el("div", { class: "hint", style: { marginTop: "6px", whiteSpace: "pre-wrap" } }, errLine(env)) : null,
    drift.length
      ? el(
          "ul",
          {},
          ...drift.map((x) =>
            el(
              "li",
              { class: x.kind === "version" ? "has-act" : "" },
              statusChip(x.kind),
              " ",
              el("span", { class: "mono", text: x.entry || x.source || "" }),
              x.detail ? el("span", { class: "muted", text: " · " + x.detail }) : null,
              x.path && !x.detail ? el("span", { class: "dim mono", text: " @" + x.path }) : null,
              el("span", { class: "spacer" }),
              x.kind === "version" ? reregisterBtn(x.source) : null,
            ),
          ),
        )
      : null,
  );
  return box;
}

function renderDrawer() {
  const root = $("#drawer-root");
  const s = S.drawer ? sourceByName(S.drawer) : null;
  if (!s) {
    root.hidden = true;
    root.replaceChildren();
    return;
  }
  root.hidden = false;
  mount(
    root,
    el("div", { class: "drawer-backdrop", onclick: closeDrawer }),
    el(
      "aside",
      { class: "drawer", role: "dialog", "aria-modal": "true" },
      el(
        "div",
        { class: "drawer-head" },
        el("span", { class: "drawer-title", text: s.name }),
        el("span", { class: "badge skill", text: s.kind }),
        el("div", { class: "spacer" }),
        el("button", { class: "btn btn-icon btn-ghost", type: "button", text: "✕", onclick: closeDrawer, title: "关闭" }),
      ),
      el("div", { class: "drawer-body" }, drawerBody(s)),
    ),
  );
}

function closeDrawer() {
  S.drawer = null;
  $("#drawer-root").hidden = true;
  $("#drawer-root").replaceChildren();
  render();
}

// ── 写路径 ─────────────────────────────────────────────────────────────

async function saveCategory(name, category) {
  await runOp("category", async () => {
    const { env } = await nw(`/source/skill/${enc(name)}/category`, { method: "PUT", body: { category } });
    if (!env.ok) return notify("改类别失败\n" + errLine(env), "bad");
    const s = sourceByName(name);
    if (s) s.category = category;
    notify(`已把「${name}」归到 ${category.length ? category.join(" / ") : "（未分类）"}`, "ok");
  });
}

async function addTag(id, tag) {
  await runOp("tag", async () => {
    // id 可能是两段（仓库/条目）—— 整段 percent-encode，照 nw-panel 的形状。
    const { env } = await nw(`/source/skill/${enc(id)}/tags`, { method: "POST", body: { name: tag } });
    if (!env.ok) return notify("加标签失败\n" + errLine(env), "bad");
    await refresh();
    notify(`已给 ${id} 打上 ${tag}`, "ok");
  });
}

async function removeTag(id, tag) {
  await runOp("tag", async () => {
    const { env } = await nw(`/source/skill/${enc(id)}/tags/${enc(tag)}`, { method: "DELETE" });
    if (!env.ok) return notify("摘标签失败\n" + errLine(env), "bad");
    await refresh();
    notify(`已从 ${id} 摘掉 ${tag}`, "ok");
  });
}

async function recheckRun(name) {
  await runOp(name ? "recheck:" + name : "recheck-all", async () => {
    notify(name ? `对账「${name}」：真 clone 上游，可能要几十秒…` : "全量对账：逐个源 clone 上游，慢是正常的…");
    const { env, status } = await nw("/source/recheck", { method: "POST", body: name ? { name } : {} });
    if (name) {
      S.lastReport = S.lastReport || {};
      S.lastReport[name] = env;
    }
    const n = env?.data?.drift?.length || 0;
    if (env.ok) {
      notify(name ? `「${name}」没有漂移` : "全量对账：账与上游一致", "ok");
      return;
    }
    if (n) {
      // 漂移 = 报告，不是错误：接住 400 照样渲染（不落错误页）。
      if (!name) {
        S.report = { data: env.data, scope: null, env };
        S.mode = "report";
        notify(`全量对账：${n} 条漂移`, "warn");
      } else {
        notify(`「${name}」检出 ${n} 条漂移`, "warn");
      }
    } else {
      notify(`对账没结论（HTTP ${status}）\n${errLine(env)}`, "bad");
    }
  });
}

function confirmDelete(s) {
  openModal({
    title: `删掉源「${s.name}」？`,
    body: el(
      "div",
      { style: { display: "grid", gap: "10px" } },
      el("div", {}, `这会从 skills.json 里删掉这条记录和它下面的 ${(s.entries || []).length} 个条目路径。`),
      el("div", { class: "hint" }, "上游一个字节都不动。要加回来就重跑一次 register —— 但手工分类不会复活（条目清单永远取上游最新）。"),
      el("div", { class: "hint" }, "删的是编目记录，不是仓库；挂在这条上的标签引用会悬空，等下一次 tag 写操作清理。"),
    ),
    actions: [
      { text: "取消" },
      {
        text: "确认删源",
        kind: "danger",
        run: async () => {
          const { env } = await nw(`/source/skill/${enc(s.name)}`, { method: "DELETE" });
          if (!env.ok) return notify("删源失败\n" + errLine(env), "bad");
          notify(`已删除 ${s.name}`, "ok");
          if (S.drawer === s.name) S.drawer = null;
          await refresh();
          render();
        },
      },
    ],
  });
}

// ── 登记源（两段式）──────────────────────────────────────────────────

function openRegister(seed) {
  // 只认「长得像一条源记录」的 seed —— 事件对象、字符串都不算（误传时当没预填）。
  seed = seed && typeof seed === "object" && seed.name ? seed : null;
  const state = { result: null, body: null };
  const form = {
    repo: seed?.repo || "",
    name: seed?.name || "",
    category: (seed?.category || []).join(" / "),
  };
  const resultBox = el("div", {});

  const collect = () => ({
    repo: form.repo.trim(),
    kind: "skill",
    name: form.name.trim(),
    category: parseCategory(form.category),
    pull: false,
  });

  const preview = async () => {
    const body = collect();
    if (!body.repo) return notify("repo 必填：owner/repo 或 https 链接", "bad");
    state.body = body;
    previewBtn.disabled = true;
    previewBtn.textContent = "克隆 + 扫描 + 验收中…";
    const { env, status } = await nw("/source", { method: "POST", body });
    previewBtn.disabled = false;
    previewBtn.textContent = "重新预览";
    state.result = env;
    syncCommit();
    renderResult();
    if (!env.ok && (env.data?.acceptance || []).length === 0) {
      notify(`预览失败（HTTP ${status}）\n${errLine(env)}`, "bad");
    }
  };

  const commit = async () => {
    const body = state.body || collect();
    if (!state.result?.ok) return notify("先预览一次，验收过了才落账", "warn");
    if (JSON.stringify(collect()) !== JSON.stringify(body)) {
      return notify("表单改过了 —— 重新预览一次，预览和落账必须是同一份意图", "warn");
    }
    const name = state.result?.data?.name || body.name;
    if (!name) return notify("上游没算出登记名，手动填一个", "bad");
    commitBtn.disabled = true;
    commitBtn.textContent = "落账中…";
    const { env } = await nw(`/source/${enc(name)}`, { method: "PUT", body: { ...body, name } });
    if (!env.ok) {
      commitBtn.disabled = false;
      commitBtn.textContent = "落账 --yes";
      return notify("落账失败（编目一个字节都没落）\n" + errLine(env), "bad");
    }
    notify(`已${env.data?.updated ? "覆盖登记" : "登记"} ${env.data?.name || name}，${env.data?.entries?.length ?? 0} 个条目`, "ok");
    closeModal();
    await refresh();
    S.mode = "store";
    S.report = null;
    S.drawer = env.data?.name || name;
    render();
  };

  function renderResult() {
    const env = state.result;
    const d = env?.data || {};
    const rows = d.acceptance || [];
    resultBox.replaceChildren();
    if (!env) return;
    if (env.ok === false && !rows.length) {
      resultBox.append(el("div", { class: "card drift" }, el("div", { style: { whiteSpace: "pre-wrap" } }, errLine(env))));
      return;
    }
    resultBox.append(
      el(
        "div",
        { class: "card" },
        el(
          "div",
          { class: "row" },
          el("span", { class: "chip chip-accent", text: d.name || "?" }),
          el("span", { class: "chip", text: `${(d.entries || []).length} 条目` }),
          d.version ? el("span", { class: "chip chip-ok", text: d.version }) : el("span", { class: "chip chip-warn", text: "无版本" }),
          el("div", { class: "spacer" }),
          el("span", { class: "hint mono", text: String(d.commit || "").slice(0, 7) }),
          el("span", { class: "chip" + (d.written ? " chip-ok" : " chip-warn"), text: d.written ? "已落账" : "未落账（预览）" }),
        ),
      ),
      rows.length
        ? el(
            "table",
            { class: "entries", style: { marginTop: "10px" } },
            el("thead", {}, el("tr", {}, el("th", { text: "验收" }), el("th", { text: "结论" }), el("th", { text: "说明" }))),
            el("tbody", {}, ...rows.map((a) => el("tr", {}, el("td", { class: "mono", text: a.id }), el("td", {}, statusChip(a.status)), el("td", { class: "muted", text: a.message || "" })))),
          )
        : null,
    );
    if (!env.ok) resultBox.append(el("div", { class: "card drift", style: { marginTop: "10px", whiteSpace: "pre-wrap" } }, errLine(env)));
  }

  const previewBtn = el("button", { class: "btn", type: "button", text: "预览（不写账）", onclick: preview });
  const commitBtn = el("button", { class: "btn btn-primary", type: "button", text: "落账 --yes", disabled: true, onclick: commit });

  /** 落账只在「预览通过 + 表单与预览同一份」时可点 —— 跟 SourcePane 的验收闸同款：
   *  红的那一格不让确认，不靠点了再弹 toast 教育人。 */
  function syncCommit() {
    const ok = Boolean(state.result?.ok) && JSON.stringify(collect()) === JSON.stringify(state.body);
    commitBtn.disabled = !ok;
    commitBtn.title = ok ? "写编目（覆盖写会刷新条目与版本）" : "先预览，且别再改表单";
  }

  openModal({
    title: seed ? `重新登记「${seed.name}」` : "登记 skill 源",
    body: el(
      "div",
      { style: { display: "grid", gap: "10px" } },
      seed
        ? el(
            "div",
            { class: "card drift" },
            "这是覆盖写：整条记录按下面这份表单",
            el("b", { text: "重建" }),
            " —— 条目清单重扫、版本重抓；类别一栏空着就会把现有分类清掉（现在预填的是账面那份：" + ((seed.category || []).join(" / ") || "未分类") + "）。",
            el("div", { class: "hint", style: { marginTop: "6px" } }, "写路径和确认闸跟第一次登记一字不差 —— 预览 → 验收 6 行 → 落账 --yes。"),
          )
        : null,
      el(
        "div",
        { class: "field" },
        el("label", { text: "仓库（--repo）" }),
        el("input", { type: "text", value: form.repo, placeholder: "owner/repo 或 https://github.com/owner/repo", oninput: (ev) => { form.repo = ev.target.value; syncCommit(); } }),
        el("div", { class: "hint" }, "必须是 GitHub：register 只认 GitHub 链接；取回是 git clone --depth 1。"),
      ),
      el(
        "div",
        { class: "row", style: { gap: "10px", alignItems: "flex-start" } },
        el(
          "div",
          { class: "field", style: { flex: "1 1 200px" } },
          el("label", { text: "登记名（--name，可空）" }),
          el("input", { type: "text", value: form.name, placeholder: "缺省 = 仓库 basename 小写归一", oninput: (ev) => { form.name = ev.target.value; syncCommit(); } }),
          el("div", { class: "hint" }, "同仓同名 = 覆盖写；撞名换仓 = 拒。"),
        ),
        el(
          "div",
          { class: "field", style: { flex: "1 1 200px" } },
          el("label", { text: "类别（--category，可空）" }),
          el("input", { type: "text", value: form.category, placeholder: "工具 / cli", oninput: (ev) => { form.category = ev.target.value; syncCommit(); } }),
          el("div", { class: "hint" }, "可多级，用 / 分隔；空 = category: []（未分类）。"),
        ),
      ),
      el("div", { class: "row", style: { marginTop: "2px" } }, previewBtn, el("span", { class: "hint", text: "预览会真取回 + 扫 + 跑恒 6 行验收，但编目一个字节不落。" })),
      resultBox,
    ),
  });
  $(".modal-foot").replaceChildren(
    el("button", { class: "btn", type: "button", text: "取消", onclick: closeModal }),
    commitBtn,
  );
}

// ── 设置 ───────────────────────────────────────────────────────────────

function openSettings() {
  const cfg = S.config || {};
  const baseInput = el("input", { type: "text", value: cfg.base || "", placeholder: "https://j1900.tail8d0018.ts.net:8443" });
  const tokenInput = el("input", { type: "password", value: "", placeholder: cfg.hasToken ? `已有 token（${cfg.tokenHint}），留空 = 不改` : "可选：备用入口的 Bearer token" });
  const status = el("div", { class: "hint" });

  openModal({
    title: "warehouse 连接",
    body: el(
      "div",
      { style: { display: "grid", gap: "10px" } },
      el(
        "div",
        { class: "field" },
        el("label", { text: "base" }),
        baseInput,
        el("div", { class: "hint" }, "首选 tailnet 的 8443（TLS 之后无认证）。token 只存在这个块的 data/config.json，浏览器永远看不到它。"),
      ),
      el("div", { class: "field" }, el("label", { text: "token（可选）" }), tokenInput),
      status,
    ),
    actions: [
      { text: "取消" },
      {
        text: "存下并测试",
        kind: "primary",
        run: async () => {
          const body = { base: baseInput.value.trim() };
          if (tokenInput.value.trim()) body.token = tokenInput.value.trim();
          const { env } = await cfgPut(body);
          if (!env.ok) return notify("存配置失败\n" + errLine(env), "bad");
          const health = await nw("/health");
          status.textContent = health.env?.ok ? `通了：${JSON.stringify(health.env)}` : "存下了，但 health 没回 ok\n" + errLine(health.env);
          S.loading = true;
          S.loadError = "";
          render();
          await refresh();
          render();
          closeModal();
          notify(health.env?.ok ? "连接正常" : "配置已存，连接还没通", health.env?.ok ? "ok" : "warn");
        },
      },
    ],
  });
}

// ── 弹层 ───────────────────────────────────────────────────────────────

let modalCleanup = null;

function closeModal() {
  $("#modal-root").hidden = true;
  $("#modal-root").replaceChildren();
  modalCleanup?.();
  modalCleanup = null;
}

function openModal({ title, body, actions = [] }) {
  const root = $("#modal-root");
  root.hidden = false;
  root.replaceChildren(
    el(
      "div",
      { class: "modal", role: "dialog", "aria-modal": "true" },
      el("div", { class: "modal-head" }, el("span", { class: "modal-title", text: title }), el("div", { class: "spacer" }), el("button", { class: "btn btn-icon btn-ghost", type: "button", text: "✕", onclick: closeModal })),
      el("div", { class: "modal-body" }, body),
      el(
        "div",
        { class: "modal-foot" },
        ...actions.map((a) =>
          el("button", {
            class: "btn" + (a.kind === "primary" ? " btn-primary" : a.kind === "danger" ? " btn-danger" : ""),
            type: "button",
            text: a.text,
            onclick: async (ev) => {
              if (!a.run) return closeModal();
              const btn = ev.currentTarget;
              const label = btn.textContent;
              btn.disabled = true;
              try {
                await a.run();
              } finally {
                btn.disabled = false;
                btn.textContent = label;
              }
            },
          }),
        ),
      ),
    ),
  );
  root.onclick = (ev) => {
    if (ev.target === root) closeModal();
  };
  const onKey = (ev) => {
    if (ev.key === "Escape") closeModal();
  };
  window.addEventListener("keydown", onKey);
  modalCleanup = () => window.removeEventListener("keydown", onKey);
}

// ── 渲染循环 ───────────────────────────────────────────────────────────

function renderTopbar() {
  const pill = $("#health");
  pill.className = "pill";
  if (S.loading) {
    pill.classList.add("pill-idle");
    pill.textContent = "连接中…";
  } else if (S.loadError) {
    pill.classList.add("pill-bad");
    pill.textContent = "离线";
    pill.title = S.loadError;
  } else {
    pill.classList.add("pill-ok");
    pill.textContent = `已连接 · nw ${S.nwVersion || "?"}`;
    pill.title = S.config?.base || "";
  }
  $("#btn-recheck-all").disabled = Boolean(S.busy);
  $("#btn-new").disabled = Boolean(S.busy);
  $("#btn-recheck-all").textContent = S.busy === "recheck-all" ? "对账中…" : "对账";
}

function render() {
  renderTopbar();
  renderRail();
  const host = $("#results");
  mount(host);
  if (S.loading) {
    // renderStore 自己会画加载态
  }
  if (S.mode === "report" && S.report) renderReport(host);
  else renderStore(host);
  renderDrawer();
}

async function bootLoad() {
  try {
    S.config = await cfgGet();
  } catch {
    S.config = { base: "?" };
  }
  S.loading = true;
  S.loadError = "";
  render();
  await refresh();
  render();
}

// ── 绑定 ───────────────────────────────────────────────────────────────

$("#q").addEventListener("input", (ev) => {
  S.q = ev.target.value;
  renderRail();
  const host = $("#results");
  mount(host);
  if (S.mode === "report" && S.report) renderReport(host);
  else renderStore(host);
});

window.addEventListener("keydown", (ev) => {
  if (ev.key === "/" && document.activeElement?.tagName !== "INPUT" && document.activeElement?.tagName !== "SELECT") {
    ev.preventDefault();
    $("#q").focus();
  }
  if (ev.key === "Escape" && S.drawer && $("#modal-root").hidden) closeDrawer();
});

$("#btn-recheck-all").addEventListener("click", () => recheckRun());
$("#btn-new").addEventListener("click", () => openRegister()); // 不能直接递函数：事件对象会被当成 seed
$("#btn-settings").addEventListener("click", openSettings);

bootLoad();
