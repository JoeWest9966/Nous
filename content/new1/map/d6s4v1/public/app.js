// 浏览器这一侧：只做三件事 —— 取 JSON、画表、把 Mermaid 源码交给 pele。
//
// 所有数字都来自 /api/*，没有一个是前端算出来的。前端的活儿：
//   1. 记住你在哪一屏、看哪条会话、回放到哪一条（写进 hash，后退键就还能用）
//   2. 把「字节 %」这种一眼能比的东西画成条
//   3. 图挂了要说清是图挂了（pele 的报错原样贴出来），不是留一片空白

const state = {
  tab: "overview",
  db: null,
  conv: null,
  upTo: null,
  targets: [],
  detail: null,
};

const $ = (selector, root = document) => root.querySelector(selector);

const esc = (value) =>
  String(value ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

const bytes = (n) => {
  const value = Number(n) || 0;
  if (value < 1024) return `${value} B`;
  if (value < 1024 ** 2) return `${(value / 1024).toFixed(value < 10_240 ? 1 : 0)} KB`;
  return `${(value / 1024 ** 2).toFixed(1)} MB`;
};

const num = (n) => (Number(n) || 0).toLocaleString("en-US");
const pct = (part, total) => (total > 0 ? `${((part / total) * 100).toFixed(part / total < 0.1 ? 1 : 0)}%` : "0%");
const pad = (n) => String(n).padStart(2, "0");
const clock = (ms) => {
  if (!ms) return "—";
  const d = new Date(ms);
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
};
const duration = (ms) => {
  const value = Number(ms) || 0;
  if (value < 1000) return `${value} ms`;
  if (value < 60_000) return `${(value / 1000).toFixed(1)} s`;
  if (value < 3_600_000) return `${Math.round(value / 60_000)} 分钟`;
  return `${(value / 3_600_000).toFixed(1)} 小时`;
};

const kpi = (label, value, note = "") =>
  `<div class="kpi"><label>${esc(label)}</label><b>${esc(value)}</b>${note ? `<small>${esc(note)}</small>` : ""}</div>`;

const notice = (text, kind = "") => `<div class="notice ${kind}">${esc(text)}</div>`;

/** 一张图的容器：出错就把 pele 的原话贴在这儿，别吞。 */
function figure(container, source, caption) {
  const fig = document.createElement("figure");
  fig.className = "diagram";
  if (!source) {
    fig.classList.add("empty");
    fig.textContent = caption ? `${caption}：库里没这个数据` : "库里没这个数据";
    container.append(fig);
    return;
  }
  fig.innerHTML = `<figcaption>${esc(caption ?? "")}</figcaption><div class="mount"></div>`;
  container.append(fig);
  const mount = $(".mount", fig);
  try {
    pele.mount(mount, source, { zoom: "auto" });
  } catch (error) {
    mount.innerHTML = `<p class="notice bad">这张图没画出来：${esc(error?.message ?? error)}</p>`;
  }
}

/**
 * 地址一律相对：宿主把页面反代在 `/p/<board>/<id>/` 底下，写绝对路径就逃出前缀了。
 * 所以这里从 `location.href` 起算 —— 在宿主里和在块自己的端口上都是对的。
 */
const api = async (path, params = {}) => {
  const url = new URL(`api/${path}`, location.href);
  if (state.db) url.searchParams.set("db", state.db);
  for (const [key, value] of Object.entries(params)) {
    if (value != null) url.searchParams.set(key, value);
  }
  const response = await fetch(url, { headers: { accept: "application/json" } });
  const body = await response.json();
  if (!response.ok) throw new Error(body?.error ?? `HTTP ${response.status}`);
  return body;
};

const panel = (name) => $(`#panel-${name}`);

// ── ① 解剖 ───────────────────────────────────────────────────────────────────

async function renderOverview() {
  const root = panel("overview");
  root.innerHTML = notice("读库中…", "info");
  const data = await api("overview");
  const { bytes: sizes, pragmas, file } = data;
  const share = (part) => pct(part, sizes.db);
  root.innerHTML = `
    <div class="kpis">
      ${kpi("库文件", bytes(sizes.db), file.mtime ? clock(file.mtime) : "")}
      ${kpi("WAL", bytes(sizes.wal), sizes.wal ? "还没 checkpoint" : "空的")}
      ${kpi("空闲页", bytes(sizes.free), `${pragmas.freelist} 页 · VACUUM 能拿回`)}
      ${kpi("表 / 行", `${data.tables.length}`, `${num(Object.values(data.counts).reduce((a, b) => a + (b ?? 0), 0))} 行`)}
      ${kpi("entries", num(data.counts.entries ?? 0), `${bytes(data.entriesByKind.reduce((s, k) => s + k.bytes, 0))} 的 JSON`)}
      ${kpi("schema", `v${pragmas.schemaVersion}`, `page ${pragmas.pageSize}B · ${pragmas.journalMode}`)}
    </div>
    ${data.attention.map((line) => notice(line, line.includes("旁路") ? "info" : "")).join("")}
    ${
      data.integrity.checked
        ? data.integrity.ok
          ? notice("integrity_check 通过", "good")
          : notice(`integrity_check 没过：${data.integrity.messages.join(" / ")}`, "bad")
        : ""
    }
  `;

  const treePair = document.createElement("div");
  treePair.className = "grid2";
  root.append(treePair);
  figure(treePair, data.diagrams.treemap, "字节解剖 · 按页（dbstat 真页数，格子里的数字是 KB）");
  figure(treePair, data.diagrams.treemapKinds, "同一份数据换个尺子 · entries 里 JSON 字节的分布（还是 KB）");

  const table = document.createElement("div");
  const rows = data.tables
    .map(
      (row) => `
      <tr>
        <td class="mono">${esc(row.name)}</td>
        <td class="num">${row.rows == null ? "—" : num(row.rows)}</td>
        <td class="num">${bytes(row.bytes)}</td>
        <td>
          <div class="bar"><i style="width:${Math.max(1.5, (row.bytes / Math.max(1, sizes.db)) * 100)}%"></i>
          <span class="muted mono">${share(row.bytes)}</span></div>
        </td>
      </tr>`,
    )
    .join("");
  table.innerHTML = `
    <h2>表 · ${data.tables.length} 张，${bytes(data.tables.reduce((s, r) => s + r.bytes, 0))}</h2>
    <table><thead><tr><th>表</th><th class="num">行</th><th class="num">字节</th><th>占库</th></tr></thead>
    <tbody>${rows}</tbody></table>`;
  root.append(table);

  if (data.entriesByKind.length) {
    const kindRows = data.entriesByKind
      .map(
        (row) => `<tr><td class="mono">${esc(row.kind)}</td><td class="num">${num(row.count)}</td>
        <td class="num">${bytes(row.bytes)}</td>
        <td class="num muted">${num(row.count ? Math.round(row.bytes / row.count) : 0)} B / 条</td></tr>`,
      )
      .join("");
    const box = document.createElement("div");
    box.innerHTML = `<h2>entries 里到底装的是什么</h2>
      <table><thead><tr><th>kind</th><th class="num">条</th><th class="num">总字节</th><th class="num">平均</th></tr></thead>
      <tbody>${kindRows}</tbody></table>`;
    root.append(box);
  }

  figure(root, data.diagrams.er, "pi-durable 自己那张 ER 图 —— 这一页认得的就是这些表");
}

// ── ② 会话树 ─────────────────────────────────────────────────────────────────

async function renderConversations() {
  const root = panel("conversations");
  root.innerHTML = notice("读会话树…", "info");
  const data = await api("conversations");
  root.innerHTML = `
    <div class="kpis">
      ${kpi("会话", num(data.count))}
      ${kpi("entries", num(data.totals.entries))}
      ${kpi("tokens", num(data.totals.tokens), "所有 assistant 自己报的 totalTokens 之和")}
      ${kpi("花费", `$${data.totals.cost.toFixed(4)}`, "provider 没给价就是 0")}
    </div>`;
  figure(root, data.diagrams.git, "会话树 · owner_conversation_id / owner_task_id（分叉才长得出来）");
  figure(root, data.diagrams.timeline, "什么时候跟它说过话 · 每条会话一个点，标的是最后一次发言");

  const box = document.createElement("div");
  const rows = data.list
    .map(
      (row) => `
      <tr class="clickable" data-conv="${row.id}">
        <td class="mono">#${row.id}</td>
        <td class="trunc">${esc(row.title)}</td>
        <td class="num">${num(row.entries)}</td>
        <td class="num">${num(row.tokens)}</td>
        <td class="num">${row.toolCalls}</td>
        <td class="mono muted">${esc(row.models.join(" ") || "—")}</td>
        <td class="mono muted">${row.ownerConv ? `分叉自 #${row.ownerConv}` : ""}${row.ownerTask ? ` 任务 #${row.ownerTask}` : ""}</td>
        <td class="mono muted">${clock(row.lastAt)}</td>
      </tr>`,
    )
    .join("");
  box.innerHTML = `
    <h2>会话 · 点一行进回放</h2>
    <table><thead><tr><th>id</th><th>第一句</th><th class="num">entries</th><th class="num">tokens</th>
    <th class="num">工具</th><th>模型</th><th>来路</th><th>最后一条</th></tr></thead>
    <tbody>${rows}</tbody></table>`;
  root.append(box);
  root.querySelectorAll("[data-conv]").forEach((tr) =>
    tr.addEventListener("click", () => {
      state.conv = Number(tr.dataset.conv);
      state.upTo = null;
      switchTab("replay");
    }),
  );
}

// ── ③ 回放 ───────────────────────────────────────────────────────────────────

async function renderReplay() {
  const root = panel("replay");
  const conversations = await api("conversations");
  if (!state.conv || !conversations.list.some((row) => row.id === state.conv)) {
    state.conv = conversations.list[0]?.id ?? null;
  }
  if (!state.conv) {
    root.innerHTML = notice("这个库里还没有会话。先去那个块里说一句话，再回来。", "info");
    return;
  }
  const detail = await api("conversation", { id: state.conv, upTo: state.upTo });
  state.detail = detail;

  const options = conversations.list
    .map((row) => `<option value="${row.id}" ${row.id === state.conv ? "selected" : ""}>#${row.id} · ${esc(row.title.slice(0, 40))}</option>`)
    .join("");

  root.innerHTML = `
    <div class="kpis">
      ${kpi("会话", `#${detail.id}`, detail.ownerConv ? `分叉自 #${detail.ownerConv}` : "")}
      ${kpi("entries", num(detail.entries.length))}
      ${kpi("任务", num(detail.tasks.length), detail.tasks.filter((t) => t.status !== "terminal").map((t) => t.status).join(" ") || "全部 terminal")}
      ${kpi("文档", num(detail.documents.length), `${bytes(detail.documents.reduce((s, d) => s + d.bytes, 0))} · ${detail.documents.reduce((s, d) => s + d.revisions, 0)} 个 revision`)}
      ${kpi("输入", num(detail.submissions.length), `${detail.submissions.filter((s) => s.status !== "done").length} 条没答完`)}
    </div>
    <div style="display:flex;gap:10px;align-items:center;margin-bottom:12px">
      <label class="muted" for="conv" style="font-size:12px">换一条</label>
      <select id="conv">${options}</select>
      <button id="reset-upTo" ${state.upTo ? "" : "hidden"}>回到最新</button>
    </div>`;

  figure(root, detail.diagrams.context, "上下文是怎么涨上去的 · 每次调用真正喂进去的 input + cacheRead");

  const split = document.createElement("div");
  split.className = "split";
  split.innerHTML = `
    <div>
      <h2>entries · 点一行，看那一刻模型看到了什么</h2>
      <table><thead><tr><th>id</th><th>kind</th><th class="num">B</th><th class="num">tok</th><th>内容 / 工具</th></tr></thead>
      <tbody>${detail.entries
        .map(
          (entry) => `
        <tr class="clickable" data-entry="${entry.id}" ${state.upTo === entry.id ? 'aria-selected="true"' : ""}>
          <td class="mono">${entry.id}</td>
          <td><span class="pill ${esc(entry.role || entry.kind.replace("pi.", ""))}">${esc(entry.kind.replace("pi.", ""))}</span></td>
          <td class="num">${entry.bytes}</td>
          <td class="num">${entry.tokens || ""}</td>
          <td class="trunc">${
            entry.toolCalls.length
              ? `<span class="mono">→ ${esc(entry.toolCalls.map((c) => c.tool).join(", "))}</span>`
              : entry.tool
                ? `<span class="mono muted">${esc(entry.tool)}${entry.ms != null ? ` · ${entry.ms}ms` : ""}${entry.isError ? " · 失败" : ""}</span>`
                : esc(entry.text.slice(0, 200))
          }</td>
        </tr>`,
        )
        .join("")}</tbody></table>
      <details><summary class="muted" style="cursor:pointer">这条会话说了什么（sequence）</summary></details>
    </div>
    <aside class="prompt" id="prompt"></aside>`;
  root.append(split);
  figure($("details", split), detail.diagrams.sequence, "对话顺序 · 参与者是从记录里现拼的");
  split.querySelector("details").open = false;

  renderPrompt(detail.prompt);

  $("#conv", root).addEventListener("change", (event) => {
    state.conv = Number(event.target.value);
    state.upTo = null;
    renderReplay();
  });
  $("#reset-upTo", root)?.addEventListener("click", () => {
    state.upTo = null;
    renderReplay();
  });
  root.querySelectorAll("[data-entry]").forEach((tr) =>
    tr.addEventListener("click", () => {
      state.upTo = Number(tr.dataset.entry);
      renderReplay();
    }),
  );
}

function renderPrompt(prompt) {
  const box = $("#prompt");
  if (!box) return;
  const totals = prompt.totals;
  box.innerHTML = `
    <h3>此刻模型看到了什么</h3>
    <div class="meta">
      ${prompt.upTo ? `回放到 entry #${prompt.upTo}` : "整条会话"} ·
      ${totals.messages} 条消息 · ${num(totals.chars)} 字符 ≈ ${num(totals.approxTokens)} tok
      ${totals.realInputTokens ? ` · 真值 ${num(totals.realInputTokens)} input tok` : ""}
      ${totals.toolChars ? `<br />其中工具描述 ${num(totals.toolChars)} 字符（${pct(totals.toolChars, totals.chars)}）` : ""}
    </div>
    ${prompt.messages
      .map(
        (message) => `
      <div class="msg ${esc(message.kind.replace("pi.", ""))}">
        <div class="head"><b>#${message.id}</b><span>${esc(message.label)}</span>
        <span class="muted">${message.bytes} B${message.tokens ? ` · ${num(message.tokens)} tok` : ""}${message.head ? " · 压缩点" : ""}</span></div>
        <p>${esc(message.text || "(空)")}</p>
      </div>`,
      )
      .join("")}
    <p class="muted" style="font-size:11px;margin-top:10px">
      这不是模拟：条目就是库里存的那一条条 record。真实 prompt 还会插上 provider 自己的模板与工具 schema，
      所以「真值」比「≈」更可信。
    </p>`;
}

// ── ④ 账单 ───────────────────────────────────────────────────────────────────

async function renderCost() {
  const root = panel("cost");
  const data = await api("cost");
  const t = data.totals;
  root.innerHTML = `
    <div class="kpis">
      ${kpi("总 tokens", num(t.tokens))}
      ${kpi("输入", num(t.input), "每次调用都重发一遍历史")}
      ${kpi("输出", num(t.output))}
      ${kpi("缓存读", num(t.cacheRead), t.input ? `命中率 ≈ ${pct(t.cacheRead, t.input + t.cacheRead)}` : "")}
      ${kpi("推理", num(t.reasoning))}
      ${kpi("花费", `$${t.cost.toFixed(4)}`, `${num(t.entries)} 次生成`)}
    </div>
    ${t.cost === 0 ? notice(data.note, "info") : ""}`;
  figure(root, data.diagrams.xyTokens, `tokens · ${data.byDay.length >= 2 ? "按天" : "按小时"}的分布`);
  const pair = document.createElement("div");
  pair.className = "grid2";
  root.append(pair);
  figure(pair, data.diagrams.pieModels, "tokens 占比 · 按模型");
  figure(pair, data.diagrams.sankey, "tokens 从哪儿来、到哪儿去");

  const box = document.createElement("div");
  const perCall = (row) => (row.calls ? num(Math.round(row.tokens / row.calls)) : "—");
  box.innerHTML = `
    <h2>按模型</h2>
    <table><thead><tr><th>模型</th><th class="num">调用</th><th class="num">tokens</th><th class="num">每次</th><th class="num">花费</th></tr></thead>
    <tbody>${data.byModel
      .map(
        (row) => `<tr><td class="mono">${esc(row.label)}</td><td class="num">${num(row.calls)}</td>
        <td class="num">${num(row.tokens)}</td><td class="num muted">${perCall(row)}</td>
        <td class="num">$${row.cost.toFixed(4)}</td></tr>`,
      )
      .join("")}</tbody></table>
    <h2>按会话</h2>
    <table><thead><tr><th>会话</th><th class="num">调用</th><th class="num">tokens</th><th class="num">每次</th><th class="num">花费</th></tr></thead>
    <tbody>${data.byConv
      .map(
        (row) => `<tr class="clickable" data-conv="${row.label.replace("#", "")}"><td class="mono">${esc(row.label)}</td>
        <td class="num">${num(row.calls)}</td><td class="num">${num(row.tokens)}</td>
        <td class="num muted">${perCall(row)}</td><td class="num">$${row.cost.toFixed(4)}</td></tr>`,
      )
      .join("")}</tbody></table>
    <h2>按${data.byDay.length >= 2 ? "天" : "小时"}</h2>
    <table><thead><tr><th>时间</th><th class="num">调用</th><th class="num">tokens</th><th class="num">花费</th></tr></thead>
    <tbody>${(data.byDay.length >= 2 ? data.byDay : data.byHour)
      .map(
        (row) => `<tr><td class="mono">${esc(row.label)}</td><td class="num">${num(row.calls)}</td>
        <td class="num">${num(row.tokens)}</td><td class="num">$${row.cost.toFixed(4)}</td></tr>`,
      )
      .join("")}</tbody></table>`;
  root.append(box);
  box.querySelectorAll("[data-conv]").forEach((tr) =>
    tr.addEventListener("click", () => {
      state.conv = Number(tr.dataset.conv);
      switchTab("replay");
    }),
  );
}

// ── ⑤ 维修 ───────────────────────────────────────────────────────────────────

async function renderHealth() {
  const root = panel("health");
  const data = await api("health");
  const { tasks, submissions, documents } = data;
  root.innerHTML = `
    <div class="kpis">
      ${kpi("卡住", num(tasks.stuck.length), tasks.stuck.length ? "有活跑超了 10 分钟" : "没有活挂着")}
      ${kpi("未回答的输入", num(submissions.unanswered.length), `${submissions.total} 条 submission`)}
      ${kpi("任务", num(tasks.total), `平均 ${duration(tasks.avgMs)} · p95 ${duration(tasks.p95)}`)}
      ${kpi("base / delta", `${documents.base} / ${documents.delta}`, documents.delta ? `${pct(documents.delta, documents.revisions)} 是增量` : "一次增量都没写过")}
      ${kpi("WAL", bytes(data.walBytes), data.walBytes > 4 * 1024 * 1024 ? "该 checkpoint 了" : "正常")}
      ${kpi("空闲页", bytes(data.freelist.bytes), `${data.freelist.free} 页`)}
    </div>
    ${
      tasks.stuck.length
        ? notice(`${tasks.stuck.length} 个任务卡在 running/waiting 超过 10 分钟：${tasks.stuck.map((t) => `#${t.id}`).join(" ")}`, "bad")
        : ""
    }
    ${submissions.unanswered.length ? notice(`${submissions.unanswered.length} 条输入没被回答（unanswered）`, "") : ""}
    ${documents.delta === 0 && documents.revisions > 0 ? notice("所有 document revision 都是 base：压缩记录从没写过增量，每改一次就是整份重写。", "info") : ""}`;

  figure(root, data.diagrams.kanban, "任务按状态 · pi-durable 的六种 status");
  figure(root, data.diagrams.quadrant, "最慢的 14 次：什么时候开的（横） vs 跑了多久（纵） —— 右上角就是又晚又慢的那几次");
  const pies = document.createElement("div");
  pies.className = "grid2";
  root.append(pies);
  figure(pies, data.diagrams.pieKinds, "生成 vs 工具：钱花在想上还是翻文件上");
  figure(pies, data.diagrams.pieSubmissions, "输入的处理结果");

  if (tasks.slowest.length) {
    const box = document.createElement("div");
    box.innerHTML = `
      <h2>最慢的几次</h2>
      <table><thead><tr><th>任务</th><th>kind</th><th>会话</th><th class="num">耗时</th><th>结束于</th></tr></thead>
      <tbody>${tasks.slowest
        .map(
          (task) => `<tr><td class="mono">#${task.id}</td><td class="mono">${esc(task.kind)}</td>
          <td class="mono muted">#${task.conv}</td><td class="num">${duration(task.endedAt - task.startedAt)}</td>
          <td class="mono muted">${clock(task.endedAt)}</td></tr>`,
        )
        .join("")}</tbody></table>`;
    root.append(box);
  }

  const tickets = document.createElement("div");
  tickets.innerHTML = `<h2>维修工单 · 复制走自己跑，页面不动手</h2>`;
  for (const ticket of data.maintenance) {
    const node = document.createElement("div");
    node.className = "ticket";
    node.innerHTML = `
      <header><b>${esc(ticket.label)}</b>${ticket.danger ? '<span class="chip bad">会改动数据</span>' : ""}</header>
      <p>${esc(ticket.why)}</p>
      <div class="cmd"><code>${esc(ticket.cmd)}</code><button type="button">复制</button></div>`;
    $("button", node).addEventListener("click", async (event) => {
      try {
        await navigator.clipboard.writeText(ticket.cmd);
        event.target.textContent = "已复制";
        setTimeout(() => (event.target.textContent = "复制"), 1200);
      } catch {
        event.target.textContent = "复制失败";
      }
    });
    tickets.append(node);
  }
  root.append(tickets);
}

// ── 外壳 ─────────────────────────────────────────────────────────────────────

const RENDERERS = {
  overview: renderOverview,
  conversations: renderConversations,
  replay: renderReplay,
  cost: renderCost,
  health: renderHealth,
};

function switchTab(tab) {
  if (!RENDERERS[tab]) tab = "overview";
  state.tab = tab;
  document.querySelectorAll("nav.tabs button").forEach((button) =>
    button.setAttribute("aria-selected", String(button.dataset.tab === tab)),
  );
  for (const name of Object.keys(RENDERERS)) panel(name).hidden = name !== tab;
  history.replaceState(null, "", `#${tab}${state.db ? `/${state.db}` : ""}`);
  RENDERERS[tab]().catch((error) => {
    panel(tab).innerHTML = notice(`这一屏读挂了：${error.message}`, "bad");
  });
}

function paintTargets() {
  const select = $("#target");
  select.innerHTML = state.targets
    .map(
      (target) =>
        `<option value="${esc(target.id)}">${esc(target.id)} · ${target.entries}e · ${bytes(target.bytes)}${target.own ? " · 自己" : ""}</option>`,
    )
    .join("");
  select.value = state.db;
  const current = state.targets.find((target) => target.id === state.db);
  $("#badge").innerHTML = current
    ? current.own
      ? '<span class="chip good">本块自己的库</span>'
      : '<span class="chip warn">旁路读盘 · 属于别的块</span>'
    : "";
}

async function boot() {
  const hash = location.hash.replace(/^#/, "");
  const [tab, db] = hash.split("/");
  if (RENDERERS[tab]) state.tab = tab;
  const data = await api("targets");
  state.targets = data.targets;
  state.db = db && data.targets.some((t) => t.id === db) ? db : data.default;
  if (!state.db) {
    panel(state.tab).innerHTML = notice(
      "找不到任何 agent.sqlite。这块自己的 data/agent.sqlite 还没生出来 —— 先让某个块跑起来（模板 agent-block），再回来看。",
      "info",
    );
    return;
  }
  paintTargets();
  $("#target").addEventListener("change", (event) => {
    state.db = event.target.value;
    state.conv = null;
    state.upTo = null;
    paintTargets();
    switchTab(state.tab);
  });
  $("#refresh").addEventListener("click", () => switchTab(state.tab));
  document.querySelectorAll("nav.tabs button").forEach((button) =>
    button.addEventListener("click", () => switchTab(button.dataset.tab)),
  );
  switchTab(state.tab);
}

boot().catch((error) => {
  $("main").innerHTML = `<div class="notice bad">启动失败：${esc(error.message)}</div>`;
});
