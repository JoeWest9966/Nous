// extend · Nous 扩展清单。全部数据来自 qrd3wh 的 `GET /v1/extensions`。
//
// 块之间只走 HTTP，地址只从宿主的 `__nous/ctx` 来 —— 这里一个 `/p/…` 都不拼。
// 页面挂在哪一层（本地直开 / 宿主反代 / 换个 board）都无所谓。

const SELF = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";
const $ = (id) => document.getElementById(id);
// 带空格的选取得用 querySelector —— $ 只认 id，别指望它理解 "#refresh .dot"。
const $sel = (sel) => document.querySelector(sel);

const get = (url) =>
  fetch(url).then(async (res) => {
    const body = await res.json().catch(() => null);
    if (!res.ok) throw new Error(body?.error ?? `HTTP ${res.status}`);
    return body ?? {};
  });

let toastTimer;
function toast(msg, kind = "") {
  const el = $("toast");
  el.textContent = msg;
  el.dataset.kind = kind;
  el.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (el.hidden = true), 3200);
}

const el = (tag, className, text) => {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
};

const short = (sha) => (sha ? sha.slice(0, 7) : "?");

// ── 一行一个扩展 ───────────────────────────────────────────────────────────

function extRow(ext, ctx) {
  const { data, names } = ctx;
  const installed = data.installed?.includes(ext.name);

  const row = el("li", "ext" + (installed ? "" : " absent"));

  const head = el("div", "ext-head");
  head.append(el("span", "ext-name", ext.name));
  head.append(el("span", "ext-ver", "v" + ext.version));
  if (data.defaults?.includes(ext.name)) head.append(el("span", "pill", "新会话默认选"));
  // 菜谱上有、厨房里没有：这才是这一页存在的理由 —— 扫目录永远看不出这种漂移。
  if (!installed) head.append(el("span", "pill bad", "仓库里有，进程里没装"));
  row.append(head);

  if (ext.summary) row.append(el("p", "ext-sum", ext.summary));

  const facts = el("div", "ext-facts");
  const add = (label, value, mono) => {
    if (!value) return;
    const span = el("span", "fact");
    span.append(el("span", "fact-k", label));
    span.append(el("span", mono ? "fact-v mono" : "fact-v", value));
    facts.append(span);
  };
  add("提示词段", ext.sections?.length ? ext.sections.join("、") : "不塞提示词");

  const needs = Object.keys(ext.needs ?? {});
  if (needs.length) {
    const span = el("span", "fact");
    span.append(el("span", "fact-k", "要宿主喂"));
    for (const key of needs) {
      const chip = el("span", "chip", key);
      chip.title = String(ext.needs[key] ?? "");
      span.append(chip);
    }
    facts.append(span);
  }

  // 谁在用：id 换成名字（名字是 qrd3wh 的 session 文档，不是我们这儿存的）
  const users = data.usage?.[ext.name] ?? [];
  const span = el("span", "fact");
  span.append(el("span", "fact-k", "谁在用"));
  if (users.length) {
    span.append(el("span", "fact-v", users.map((id) => names.get(id) ?? `#${id}`).join("、" )));
  } else {
    span.append(el("span", "fact-v dim", "还没有会话选它"));
  }
  facts.append(span);

  row.append(facts);
  return row;
}

// ── 渲染 ───────────────────────────────────────────────────────────────────

function render(data, names) {
  const repo = data.repo ?? {};
  const drift = Boolean(repo.drift);

  $("repo-path").textContent = repo.path ?? "—";
  const head = `lock ${short(repo.lock)} · HEAD ${short(repo.head)}`;
  $("repo").textContent = drift ? `${head} · ⚠ 不一致` : head;
  $("repo").dataset.drift = drift ? "1" : "0";
  $("repo").title = drift
    ? "仓库 HEAD 跟 extensions.lock.json 钉的不是同一个提交 —— 改完没重启，现在跑的是旧代码"
    : "跑的就是钉的那一版";

  const catalog = data.catalog ?? [];
  const builtin = new Set(data.builtin ?? []);
  const plates = data.nameplates;

  const ours = catalog.filter((ext) => !builtin.has(ext.name));
  $("ours-list").textContent = "";
  for (const ext of ours) $("ours-list").append(extRow(ext, { data, names }));
  if (!ours.length) $("ours-list").append(el("li", "ext dim", "仓库的 index.json 里一条自研扩展都没有。"));

  $("others-list").textContent = "";
  for (const name of data.builtin ?? []) {
    const row = el("li", "ext flat");
    const head = el("div", "ext-head");
    head.append(el("span", "ext-name", name));
    head.append(el("span", "pill", "pi-durable 自带"));
    row.append(head);
    row.append(el("p", "ext-sum", "不是我们开发的（谁写的代码谁进账），不在仓库里，也不该有装卸按钮。"));
    $("others-list").append(row);
  }
  if (plates?.count) {
    const row = el("li", "ext flat");
    const head = el("div", "ext-head");
    head.append(el("span", "ext-name", "skill:<名字>"));
    head.append(el("span", "pill", `${plates.count} 个`));
    row.append(head);
    row.append(
      el(
        "p",
        "ext-sum",
        `${(plates.names ?? []).slice(0, 12).join("、")}${plates.count > 12 ? "…" : ""}` +
          " —— skills 扩展运行时按技能生成的空名牌（能力全空，只为让会话「会」这条技能），不是独立扩展。",
      ),
    );
    $("others-list").append(row);
  }
  if (!$("others-list").children.length) $("others-list").append(el("li", "ext dim", "没有。"));

  const total = ours.length + (data.builtin?.length ?? 0) + (plates?.count ?? 0);
  $("count").textContent = `${ours.length} 个自研扩展 · 共 ${total} 项`;
}

// ── 取数 ───────────────────────────────────────────────────────────────────

async function load() {
  const dot = $sel("#refresh .dot");
  dot.dataset.state = "wait";
  try {
    const ctx = await get(SELF + "__nous/ctx");
    const tag =
      ctx.blocks?.find((block) => block.id === "qrd3wh") ??
      ctx.blocks?.find((block) => block.kind === "group" && block.name === "session");
    if (!tag) throw new Error("这一盘里没有 qrd3wh（session 引擎）—— 扩展装在它进程里，它不在就没得问");

    const base = tag.url.endsWith("/") ? tag.url : tag.url + "/";
    // usage=1 要点代价（每条会话读一次 agent），所以只在真的画这页的时候要。
    const [data, tree] = await Promise.all([get(base + "v1/extensions?usage=1"), get(base + "v1/sessions")]);
    const names = new Map((tree.items ?? []).map((item) => [item.id, item.name]));
    render(data, names);
    dot.dataset.state = "ok";
  } catch (error) {
    dot.dataset.state = "bad";
    $("count").textContent = "拿不到";
    $("ours-list").textContent = "";
    $("ours-list").append(el("li", "ext dim", String(error?.message ?? error)));
    toast(String(error?.message ?? error), "bad");
  }
}

$("refresh").onclick = load;
load();
