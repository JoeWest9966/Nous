// 这块的前端：一张拓扑 + 一个抽屉。
//
// 拓扑照 Pele 的 architecture-beta 来：组框（按标签分）+ 服务块（机器）+ 连线（到面板的那条 SSH）。
// 连线上的数字是本机对 22 端口的实测握手耗时 —— 图里唯一一个会变的数，所以它值一条线。
//
// 纪律：所有来自编目的字符串都过 esc() 再进 innerHTML；凭证只在「登记」表单里出现，
// 提交后立刻从内存里清掉，不回显、不进 URL、不进 toast。

const ROOT = (() => {
  const u = new URL(location.href);
  if (!u.pathname.endsWith("/")) u.pathname = u.pathname.replace(/[^/]*$/, "");
  return u;
})();

const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];

const esc = (s) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);

const el = (id) => document.getElementById(id);

const S = {
  machines: [],
  tags: [],
  source: {},
  upstream: {},
  counts: {},
  domains: [],
  caveats: [],
  warnings: [],
  errors: [],
  selected: null,
  mode: "overview", // overview | detail | edit | create
  draft: null,
  busy: false,
  busyLabel: "",
  deleteArm: false,
  health: null,
  menu: null, // { items[], at:{x,y}, back: element }
  touched: false, // 长按已经开过菜单了，紧跟的那个 click 得吞掉
};

const NAME_RE = /^[a-z0-9-]+$/;
const TAG_RE = /^[a-z0-9-]+$/;

async function api(path, opts = {}) {
  const res = await fetch(new URL(path, ROOT), {
    ...opts,
    headers: opts.body ? { "content-type": "application/json" } : undefined,
  });
  const body = await res.json().catch(() => null);
  const errors = (body?.errors ?? []).map((e) => (typeof e === "string" ? e : [e.message, e.hint].filter(Boolean).join(" —— ")));
  return { status: res.status, ok: res.ok && body?.ok !== false, body, errors, data: body?.data ?? {} };
}

function toast(msg, kind = "ok") {
  const t = document.createElement("div");
  t.className = "toast";
  t.dataset.kind = kind;
  t.textContent = msg;
  el("toasts").append(t);
  setTimeout(() => t.remove(), kind === "err" ? 6500 : kind === "warn" ? 9000 : 3200);
}

function copy(text) {
  const done = () => toast("已复制");
  if (navigator.clipboard?.writeText) {
    navigator.clipboard.writeText(text).then(done, () => fallback());
  } else fallback();

  function fallback() {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.append(ta);
    ta.select();
    try {
      document.execCommand("copy");
      done();
    } catch {
      toast("复制失败，选中文案手动复制", "err");
    }
    ta.remove();
  }
}

// ── 拉数据 ────────────────────────────────────────────────────────────────

async function load(fresh = false) {
  const r = await api(`api/bootstrap${fresh ? "?fresh=1" : ""}`);
  S.errors = r.errors;
  S.warnings = (r.body?.warnings ?? []).filter(Boolean);
  if (!r.ok) {
    S.machines = [];
    notice("err");
    renderAll();
    return;
  }
  S.machines = r.data.machines ?? [];
  S.tags = r.data.tags ?? [];
  S.source = r.data.source ?? {};
  S.upstream = r.data.upstream ?? {};
  S.counts = r.data.counts ?? {};
  S.domains = r.data.domains ?? [];
  S.caveats = r.data.caveats ?? [];
  notice("warn");
  renderAll();
  probeAll();
}

async function loadHealth() {
  const chip = el("health").querySelector(".dot");
  chip.dataset.state = "wait";
  el("health-text").textContent = "问上游…";
  const r = await api("api/health");
  const u = r.data?.upstream ?? {};
  S.health = u;
  if (u.ok) {
    chip.dataset.state = "ok";
    el("health-text").textContent = `nw-panel ${u.ms}ms${u.cached ? " · 缓存" : ""}`;
  } else {
    chip.dataset.state = "bad";
    el("health-text").textContent = "nw-panel 不可达";
    if (u.error) toast(`上游：${u.error}`, "err");
  }
}

function notice(kind) {
  const box = el("notice");
  const lines = kind === "err" ? S.errors : S.warnings;
  if (!lines.length) {
    box.hidden = true;
    box.innerHTML = "";
    return;
  }
  box.hidden = false;
  box.dataset.kind = kind;
  box.innerHTML = `<ul>${lines.slice(0, 6).map((w) => `<li>${esc(w)}</li>`).join("")}</ul>`;
}

// ── 渲染：画布 ────────────────────────────────────────────────────────────

const reachChip = (m) => {
  const r = m.reach?.[0];
  if (!r) return { state: "wait", label: "…" };
  if (r.ok) return { state: "ok", label: `${r.ms}ms` };
  return { state: "bad", label: r.error?.includes("没连上") || r.error === "超时" ? "超时" : "不通" };
};

const iconSvg = `<svg viewBox="0 0 24 24" aria-hidden="true"><rect x="4" y="3" width="16" height="7" rx="1.5"/><rect x="4" y="14" width="16" height="7" rx="1.5"/><path d="M7 6.5h.01M7 17.5h.01M12 10v4"/></svg>`;

/** 卡片放不下整个 URL：`j1900 · 8443` 比 `j1900.tail8d0018.ts.net:8443` 好认，
 *  完整值在 title 里，agent 要的完整值在 /api/bootstrap 里。 */
function compactBase(base) {
  try {
    const u = new URL(base);
    return `${u.hostname.split(".")[0]} · ${u.port || (u.protocol === "https:" ? 443 : 80)}`;
  } catch {
    return base;
  }
}

function nodeHtml(m) {
  const chip = reachChip(m);
  const primary = m.ips[0] ? `${m.ips[0].addr}:${m.port}` : "没有地址";
  const extraTags = m.tags.slice(1).map((t) => `<span class="tag">${esc(t)}</span>`).join("");
  return `
    <button class="node" data-node="${esc(m.name)}" aria-pressed="${S.selected === m.name}">
      <span class="ico">${iconSvg}</span>
      <span class="body">
        <span class="nm">${esc(m.name)}</span>
        <span class="meta mono">${esc(primary)}${m.ips.length > 1 ? ` <span class="dim">+${m.ips.length - 1}</span>` : ""}</span>
        ${m.note ? `<span class="meta dim">${esc(m.note)}</span>` : ""}
        ${extraTags ? `<span class="tagline">${extraTags}</span>` : ""}
      </span>
      <span class="reach" data-state="${chip.state}" data-reach="${esc(m.name)}" title="${esc(chip.label)}"><span class="dot" data-state="${chip.state}"></span></span>
    </button>`;
}

function renderCanvas() {
  const groupsCol = el("groups");  const byTag = new Map();
  for (const m of S.machines) {
    const key = m.tags[0] ?? "";
    const list = byTag.get(key) ?? [];
    list.push(m);
    byTag.set(key, list);
  }

  if (S.machines.length === 0) {
    groupsCol.innerHTML = `
      <div class="empty">
        <h3>编目是空的</h3>
        <p>上游 /api/vps 回了 0 台。要么还没登记，要么 --root 指错了（那也回空，不报错）。</p>
        <button class="primary" data-act="add">登记第一台</button>
      </div>`;
  } else {
    // 有标签的组按标签编目里的顺序排，没标签的沉到最后一组 —— 页面上不看「未分类」的脸色行事。
    const known = S.tags.map((t) => t.name).filter((n) => byTag.has(n));
    const rest = [...byTag.keys()].filter((k) => k && !known.includes(k)).sort();
    const order = [...known, ...rest];
    if (byTag.has("")) order.push("");

    groupsCol.innerHTML = order
      .map((key) => {
        const list = byTag.get(key) ?? [];
        const title = key || "未分组";
        return `
          <section class="group" data-tag="${esc(key)}">
            <header class="g-head">
              <span class="g-name">${esc(title)}</span>
              <span class="g-meta mono">${list.length} 台</span>
            </header>
            <div class="nodes">${list.map(nodeHtml).join("")}</div>
          </section>`;
      })
      .join("");
  }

  el("panel-url").textContent = compactBase(S.upstream.base ?? "");
  el("panel-url").title = S.upstream.base ?? "";
  el("panel-catalog").textContent = (S.upstream.catalogPath ?? "").split("/").pop() || "";
  el("panel-catalog").title = S.upstream.catalogPath ?? "";
  el("panel-meta").textContent = S.upstream.at ? `${S.upstream.nw || "nw"} · ${new Date(S.upstream.at).toLocaleTimeString("zh-CN", { hour12: false })}` : "—";
  el("panel-reach").textContent = S.health?.ok ? `${S.health.ms}ms` : S.health ? "不通" : "…";
  el("panel-reach").dataset.state = S.health?.ok ? "ok" : S.health ? "bad" : "wait";

  el("count").textContent = `${S.machines.length} 台`;
  requestAnimationFrame(drawEdges);
}

// ── 渲染：连线 ────────────────────────────────────────────────────────────
//
// 手画折线，而不是引一张图库：三个人五个块不值得一个 bundle，而且这条线要挂实测延迟。
// 坐标从 DOM 量，尺寸变了重画 —— 不做布局算法，就不会有布局算法的那种坏掉。

function rel(elBox, canvas) {
  const r = elBox.getBoundingClientRect();
  const c = canvas.getBoundingClientRect();
  return {
    x: r.left - c.left - canvas.clientLeft + canvas.scrollLeft,
    y: r.top - c.top - canvas.clientTop + canvas.scrollTop,
    w: r.width,
    h: r.height,
  };
}

const R = 10;

function elbow(from, to) {
  const rightward = to.x > from.x + from.w + 8;

  if (rightward) {
    const fx = from.x + from.w;
    const fy = from.y + from.h / 2;
    const tx = to.x;
    const ty = to.y + to.h / 2;
    // 所有分支共用一条竖向干线，落在「面板右边缘 → 卡片左边缘」的中点上：
    // 一排卡片就是一根干 + 几根短支，和架构图里那种扇出一片的样子一致。
    const mx = fx + Math.max(22, (tx - fx) / 2);
    const dir = ty >= fy ? 1 : -1;
    const d =
      Math.abs(ty - fy) < R * 2
        ? `M ${fx} ${fy} H ${tx}`
        : `M ${fx} ${fy} H ${mx - R} Q ${mx} ${fy} ${mx} ${fy + dir * R} V ${ty - dir * R} Q ${mx} ${ty} ${mx + R} ${ty} H ${tx}`;
    const tip = `M ${tx} ${ty} l -7 -4 v 8 z`;
    // 标签贴在箭头前：贴着终点说「到这里几毫秒」，比飘在干线上好认。
    return { d, tip, labelX: tx - 11, labelY: ty - 7, anchor: "end" };
  }

  const fx = from.x + from.w / 2;
  const fy = from.y + from.h;
  const tx = to.x + to.w / 2;
  const ty = to.y;
  const my = fy + Math.max(18, (ty - fy) / 2);
  const dir = tx >= fx ? 1 : -1;
  const d =
    Math.abs(tx - fx) < R * 2
      ? `M ${fx} ${fy} V ${ty}`
      : `M ${fx} ${fy} V ${my - R} Q ${fx} ${my} ${fx + dir * R} ${my} H ${tx - dir * R} Q ${tx} ${my} ${tx} ${my + R} V ${ty}`;
  const tip = `M ${tx} ${ty} l -4 -7 h 8 z`;
  return { d, tip, labelX: tx + 9, labelY: ty - 6, anchor: "start" };
}

let edgeEls = new Map();

function drawEdges() {
  const canvas = el("canvas");
  const svg = el("edges");
  if (!canvas || !svg) return;
  svg.style.width = `${canvas.scrollWidth}px`;
  svg.style.height = `${canvas.scrollHeight}px`;
  svg.setAttribute("viewBox", `0 0 ${canvas.scrollWidth} ${canvas.scrollHeight}`);

  const hub = el("hub");
  if (!hub) return;
  const from = rel(hub, canvas);
  const parts = [];
  edgeEls = new Map();

  for (const m of S.machines) {
    const node = canvas.querySelector(`.node[data-node="${CSS.escape(m.name)}"]`);
    if (!node) continue;
    const to = rel(node, canvas);
    const { d, tip, labelX, labelY, anchor } = elbow(from, to);
    const chip = reachChip(m);
    parts.push(`
      <path class="link" data-state="${chip.state}" data-link="${esc(m.name)}" d="${d}" />
      <path class="tip" data-state="${chip.state}" data-link="${esc(m.name)}" d="${tip}" />
      <text data-state="${chip.state}" data-label="${esc(m.name)}" x="${labelX}" y="${labelY}" text-anchor="${anchor}">${esc(chip.label)}</text>`);
  }

  svg.innerHTML = parts.join("");
  for (const m of S.machines) {
    edgeEls.set(m.name, svg.querySelector(`.link[data-link="${CSS.escape(m.name)}"]`));
  }
}

function paintReach(m) {
  const chip = reachChip(m);
  const node = el("canvas").querySelector(`[data-reach="${CSS.escape(m.name)}"]`);
  if (node) {
    node.dataset.state = chip.state;
    node.title = chip.label;
    const dot = node.querySelector(".dot");
    if (dot) dot.dataset.state = chip.state;
  }
  const label = el("edges").querySelector(`[data-label="${CSS.escape(m.name)}"]`);
  if (label) {
    label.textContent = chip.label;
    label.dataset.state = chip.state;
  }
  const link = edgeEls.get(m.name);
  if (link) {
    link.dataset.state = chip.state;
    const tip = link.nextElementSibling;
    if (tip?.classList.contains("tip")) tip.dataset.state = chip.state;
  }
}

async function probeAll(force = false, only = S.machines) {
  const list = [...only];
  let i = 0;
  const workers = Array.from({ length: Math.min(4, list.length) }, async () => {
    while (i < list.length) {
      const m = list[i++];
      try {
        const r = await api(`api/probe?name=${encodeURIComponent(m.name)}${force ? "&fresh=1" : ""}`);
        m.reach = r.ok ? [r.data] : [{ ok: false, error: r.errors[0] ?? "探测失败" }];
      } catch (e) {
        m.reach = [{ ok: false, error: String(e.message ?? e) }];
      }
      paintReach(m);
      if (S.selected === m.name && S.mode === "detail") renderDrawer();
    }
  });
  await Promise.all(workers);
  drawEdges();
}

// ── 渲染：抽屉 ────────────────────────────────────────────────────────────

const machine = (name) => S.machines.find((m) => m.name === name) ?? null;

function renderDrawer() {
  const drawer = el("drawer");
  const main = el("main");
  const wide = window.matchMedia("(min-width: 861px)").matches;

  // 宽屏常驻一个检查器；窄屏只有选中 / 在填表时才是底部抽屉。
  const show = S.mode === "create" || S.mode === "edit" || S.selected ? true : wide;
  if (!show) {
    drawer.hidden = true;
    main.dataset.drawer = "closed";
    return;
  }

  drawer.hidden = false;
  main.dataset.drawer = "open";

  if (S.mode === "create" || S.mode === "edit") return renderForm();
  if (!S.selected) return renderOverview();

  const m = machine(S.selected);
  if (!m) return renderOverview();

  const reach = m.reach?.[0];
  const primary = m.ssh[0]?.cmd ?? "";
  const ips = m.ips
    .map(
      (i, idx) => `
      <div class="row">
        <span class="tag">${esc(i.label)}</span>
        <span class="grow mono">${esc(i.addr)}</span>
        ${m.ips.length > 1 ? `<button class="copy" data-copy="${esc(m.ssh[idx]?.cmd ?? i.addr)}">复制 ssh</button>` : ""}
      </div>`,
    )
    .join("");

  const reachRows = m.reach?.length
    ? m.reach
        .map(
          (r) => `
      <div class="row">
        <span class="dot" data-state="${r.ok ? "ok" : "bad"}"></span>
        <span class="grow mono">${esc(r.addr)}:${esc(r.port)} ${r.ok ? `· ${r.ms}ms` : `· ${esc(r.error ?? "不通")}`}</span>
      </div>
      ${
        r.ok
          ? `<dl class="kv">
               ${r.service ? `<dt>服务</dt><dd class="mono">${esc(r.service)}</dd>` : ""}
               ${r.os ? `<dt>系统</dt><dd>${esc(r.os)}</dd>` : ""}
               ${r.software ? `<dt>版本</dt><dd class="mono">${esc(r.software)}</dd>` : ""}
               <dt>量于</dt><dd class="mono dim">${esc(new Date(r.checkedAt ?? r.at).toLocaleTimeString("zh-CN", { hour12: false }))}</dd>
             </dl>
             `
          : ""
      }`,
        )
        .join("")
    : `<p class="dim" style="margin:0">还没探过。</p>`;

  const tagChips = m.tags.length
    ? m.tags.map((t) => `<span class="chip">${esc(t)}<button data-untag="${esc(t)}" title="摘掉这个标签">×</button></span>`).join("")
    : `<span class="dim">没有标签</span>`;

  const known = S.tags.map((t) => t.name).filter((t) => !m.tags.includes(t));

  drawer.innerHTML = `
    <div class="d-head">
      <div>
        <h2>${esc(m.name)}</h2>
        <p class="mono">${esc(m.user)}@${esc(m.ips[0]?.addr ?? "—")}:${esc(m.port)}</p>
      </div>
      <button class="d-close" data-act="close" aria-label="关闭">×</button>
    </div>

    <div class="d-body">
      <section class="d-sec">
        <h3>连接</h3>
        ${ips || `<p class="dim" style="margin:0">编目里没有地址。</p>`}
        ${primary ? `<div class="row" style="margin-top:6px"><code class="grow">${esc(primary)}</code><button class="copy" data-copy="${esc(primary)}">复制</button></div>` : ""}
      </section>

      <section class="d-sec">
        <h3>实测（本机 → 编目里的端口）</h3>
        ${reachRows}
        <div class="row" style="margin:8px 0 0">
          <button class="ghost" data-act="probe">重新探测</button>
        </div>
      </section>

      <section class="d-sec">
        <h3>标签</h3>
        <div class="chips">${tagChips}</div>
        ${
          known.length
            ? `<div class="row" style="margin-top:8px">
                 <select class="grow" data-tagpick>
                   <option value="">挂一个已存在的标签…</option>
                   ${known.map((t) => `<option value="${esc(t)}">${esc(t)}</option>`).join("")}
                 </select>
                 <button class="ghost" data-act="tag-add">挂上</button>
               </div>
               <p class="dim" style="margin:6px 0 0;font-size:11px">面板没有「建标签」的端点：新标签只能在登记 / 覆盖登记里顺带建。</p>`
            : ""
        }
      </section>

      <section class="d-sec">
        <h3>注解（这一块写的，不是上游）</h3>
        ${m.role ? `<p style="margin:0 0 8px">${esc(m.role)}</p>` : `<p class="dim" style="margin:0 0 8px">data/vps.json 里没写这台。加一段 role，agent 才知道它是干嘛的。</p>`}
        ${
          m.facts.length
            ? `<dl class="kv">${m.facts.map((f) => `<dt>${esc(f.k)}</dt><dd>${esc(f.v)}</dd>`).join("")}</dl>`
            : ""
        }
        ${m.caveats.length ? `<ul class="caveats" style="margin-top:8px">${m.caveats.map((c) => `<li>${esc(c)}</li>`).join("")}</ul>` : ""}
      </section>

      <section class="d-sec">
        <h3>凭据</h3>
        ${
          m.credential === "key"
            ? `<p class="dim" style="margin:0">密钥型：本体在数据仓的 <code>vps_key/${esc(m.name)}.key</code>，远端会话直接能用。</p>`
            : `<p style="margin:0 0 8px">密码型：能登进去，但**跑不了远端会话** —— ssh 的密码不走 stdin，自动化只剩 sshpass（密码进 argv，<code>ps</code> 一眼看见）。</p>
               <div class="row">
                 <button class="ghost" data-act="bootstrap" ${S.busy ? "disabled" : ""}>装公钥（用密码，一次）</button>
               </div>
               <p class="dim" style="margin:6px 0 0;font-size:11px">面板现生一对 ed25519，用密码把公钥追加进目标机的 authorized_keys，**验通之后**才覆盖登记 —— 上游的凭证闸是二选一，所以登记成功那一刻，编目里的密码就没了。</p>`
        }
      </section>

      <section class="d-sec">
        <h3>危险区</h3>
        ${
          S.deleteArm
            ? `<div class="danger-confirm">
                 <strong>确认删掉 ${esc(m.name)}？</strong>它会从 /opt/Data-001/vps/vps.json 里消失。名字是唯一键、没有 rename —— 回来得重新登记，而且要再过一次 SSH 连线测试。
                 <div class="row">
                   <button class="danger" data-act="delete-yes">确认删除</button>
                   <button class="ghost" data-act="delete-no">算了</button>
                 </div>
               </div>`
            : `<button class="danger" data-act="delete">从编目里删除</button>`
        }
      </section>
    </div>

    <div class="d-foot">
      <button class="ghost" data-act="close">关闭</button>
      <span class="spacer"></span>
      <button class="primary" data-act="edit">覆盖登记</button>
    </div>`;
}

// ── 渲染：表单 ────────────────────────────────────────────────────────────

// ── 渲染：检查器的空态（= 编目自身的情况） ────────────────────────────
//
// 没选中机器时，右边不该是一片白：把「这份编目是谁的、上游在哪、还有哪些坑」摆出来。
// 这三段全部来自 /api/bootstrap，不额外发请求 —— 它本来就是给人看的那一半。

function renderOverview() {
  const up = S.upstream ?? {};
  const c = S.counts ?? {};
  const drawer = el("drawer");
  const emptyTags = (S.tags ?? []).filter((t) => (t.machines ?? []).length === 0).length;

  drawer.innerHTML = `
    <div class="d-head">
      <div>
        <h2>编目</h2>
        <p class="mono">${esc(up.catalogPath || "/opt/Data-001/vps/vps.json")}</p>
      </div>
    </div>

    <div class="d-body">
      <section class="d-sec">
        <h3>这一刻的样子</h3>
        <dl class="kv">
          <dt>机器</dt><dd>${esc(c.machines ?? 0)} 台（${esc(c.tagged ?? 0)} 台有标签，${esc(c.annotated ?? 0)} 台有注解）</dd>
          <dt>标签</dt><dd>${esc((S.tags ?? []).length)} 个，其中 ${esc(emptyTags)} 个空着</dd>
          <dt>上游</dt><dd class="mono">${esc(up.base ?? S.source.panel ?? "")}</dd>
          <dt>取回</dt><dd class="mono">${up.at ? esc(new Date(up.at).toLocaleString("zh-CN", { hour12: false })) : "—"} · ${esc(up.ms ?? 0)}ms${up.cached ? " · 缓存" : ""}</dd>
          <dt>nw</dt><dd class="mono">${esc(up.nw || "—")}</dd>
          <dt>标签编目</dt><dd class="mono">${esc(up.tagsPath || "—")}</dd>
        </dl>
      </section>

      <section class="d-sec">
        <h3>这台面板的其余编目</h3>
        ${
          (S.domains ?? [])
            .map((d) => `<div class="dom"><div class="p">${esc(d.path)}${d.schema ? ` <span class="dim">· ${esc(d.schema)}</span>` : ""}</div><div class="w">${esc(d.what)}</div></div>`)
            .join("") || `<p class="dim" style="margin:0">data/vps.json 里没写</p>`
        }
      </section>

      <section class="d-sec">
        <h3>坑 · data/vps.json</h3>
        <ul class="caveats">
          ${(S.caveats ?? []).map((x) => `<li>${esc(x)}</li>`).join("") || `<li class="dim">没写</li>`}
        </ul>
      </section>

      <section class="d-sec">
        <h3>怎么读左边那张图</h3>
        <p style="margin:0" class="dim">
          左是清单的唯一来源（nw-panel），右是按标签分组的机器。连线上的毫秒是<b>本机对编目里那个端口</b>的实测握手耗时，
          不是面板报的。点一个块看详情、覆盖登记或删除；红色虚线的是探不通。
        </p>
      </section>
    </div>

    <div class="d-foot">
      <span class="dim" style="font-size:11px">点左边一台机器 →</span>
      <span class="spacer"></span>
      <button class="primary" data-act="add">登记服务器</button>
    </div>`;
}

function newDraft(mode, m = null) {
  return {
    mode,
    name: m?.name ?? "",
    note: m?.note ?? "",
    user: m?.user ?? "root",
    port: String(m?.port ?? 22),
    ips: m?.ips?.length ? m.ips.map((i) => ({ ...i })) : [{ label: "公网", addr: "" }],
    tags: m?.tags ? [...m.tags] : [],
    credMode: "key",
    password: "",
    keyFile: null,
    errors: [],
  };
}

function renderForm() {
  const d = S.draft;
  const editing = d.mode === "edit";
  const drawer = el("drawer");

  drawer.innerHTML = `
    <div class="d-head">
      <div>
        <h2>${editing ? `覆盖登记 ${esc(d.name)}` : "登记新机器"}</h2>
        <p class="dim">写到 j1900:/opt/Data-001/vps/vps.json</p>
      </div>
      <button class="d-close" data-act="close" aria-label="关闭">×</button>
    </div>

    <div class="d-body" style="padding:0">
      <div class="form">
        <div class="field">
          <span class="lab">名字 <span class="req">*</span>
            <span class="hint">${editing ? "唯一键，CLI 没有 rename" : "小写 [a-z0-9-]+；重名 = 覆盖登记"}</span>
          </span>
          <input class="mono" data-f="name" value="${esc(d.name)}" ${editing ? "disabled" : ""} placeholder="机器名，比如 xy" autocomplete="off" />
        </div>

        <div class="field">
          <span class="lab">地址 <span class="req">*</span><span class="hint">每条带 label；第一条是 SSH 连线测试打的那条</span></span>
          <div data-ips>${d.ips.map(ipRow).join("")}</div>
          <button class="ghost" data-act="ip-add" style="align-self:flex-start;margin-top:6px">+ 再加一条地址</button>
        </div>

        <div class="field">
          <span class="lab">连接</span>
          <div class="grid2">
            <input class="mono" data-f="user" value="${esc(d.user)}" placeholder="root" />
            <input class="mono" data-f="port" value="${esc(d.port)}" inputmode="numeric" placeholder="22" />
          </div>
          <span class="hint dim">user 缺省 root · 端口缺省 22</span>
        </div>

        <div class="field">
          <span class="lab">凭证 <span class="req">${editing ? "" : "*"}</span>
            <span class="hint">${editing ? "留空 = 保留旧凭证" : "面板要用它做一次 SSH 连线测试"}</span>
          </span>
          <div class="seg">
            <button data-cred="key" aria-pressed="${d.credMode === "key"}">密钥文件</button>
            <button data-cred="password" aria-pressed="${d.credMode === "password"}">密码</button>
          </div>
          ${
            d.credMode === "key"
              ? `<label class="file">
                   <input type="file" data-f="keyFile" />
                   <span class="hint">${d.keyFile ? `已选：${esc(d.keyFile.name)}` : "选一个私钥文件 —— 只走 stdin，不进 argv，不进 URL"}</span>
                 </label>`
              : `<input type="password" data-f="password" value="${esc(d.password)}" placeholder="服务器密码" autocomplete="new-password" />
                 <div class="warn-line">密码是<b>明文</b>落进 vps.json 的（面板自己的提示 #194）。能用密钥就别用密码。</div>`
          }
        </div>

        <div class="field">
          <span class="lab">备注</span>
          <input data-f="note" value="${esc(d.note)}" placeholder="自由文本，比如「测试机」" />
        </div>

        <div class="field">
          <span class="lab">标签<span class="hint">空格或逗号分隔；不存在会自动建</span></span>
          <input class="mono" data-f="tags" value="${esc(d.tags.join(" "))}" placeholder="公网 debian12" />
        </div>

        <div class="warn-line">保存时面板会先用这个凭证 SSH 打第一条地址（10 秒超时）：<b>连不上就不落盘</b>。所以这里可能要等十几秒。</div>

        ${d.errors.length ? `<div class="err-line"><ul>${d.errors.map((e) => `<li>${esc(e)}</li>`).join("")}</ul></div>` : ""}
      </div>
    </div>

    <div class="d-foot">
      <button class="ghost" data-act="close">取消</button>
      <span class="spacer"></span>
      <button class="primary" data-act="submit" ${S.busy ? "disabled" : ""}>${S.busy ? esc(S.busyLabel) : editing ? "覆盖登记" : "登记"}</button>
    </div>`;
}

function ipRow(ip, idx) {
  return `
    <div class="iprow" data-idx="${idx}" style="margin-bottom:6px">
      <input class="mono" data-ip="label" value="${esc(ip.label)}" placeholder="公网" />
      <input class="mono" data-ip="addr" value="${esc(ip.addr)}" placeholder="1.2.3.4" inputmode="decimal" />
      <button data-act="ip-del" data-idx="${idx}" title="删掉这条地址">×</button>
    </div>`;
}

function renderAll() {
  notice(S.errors.length ? "err" : "warn");
  renderCanvas();
  renderDrawer();
}

// ── 动作 ──────────────────────────────────────────────────────────────────

function openDetail(name) {
  S.selected = name;
  S.mode = "detail";
  S.deleteArm = false;
  renderCanvas();
  renderDrawer();
}

function openForm(mode, m = null) {
  S.mode = mode;
  S.deleteArm = false;
  if (mode === "create") S.selected = null;
  S.draft = newDraft(mode, m);
  renderCanvas();
  renderDrawer();
}

function closeDrawer() {
  S.selected = null;
  S.draft = null;
  S.mode = "overview";
  S.deleteArm = false;
  S.busy = false;
  renderCanvas();
  renderDrawer();
}

async function submitForm() {
  const d = S.draft;
  if (!d || S.busy) return;

  const ips = d.ips.filter((i) => i.addr.trim());
  const tags = d.tags.map((t) => String(t).trim().toLowerCase()).filter(Boolean);
  const problems = [];
  if (!NAME_RE.test(d.name.trim())) problems.push(`名字只许 [a-z0-9-]+，收到「${d.name.trim()}」`);
  if (ips.length === 0) problems.push("至少要一条地址");
  if (ips.some((i) => !i.label.trim())) problems.push("每条地址都要有 label（公网 / 内网 / tailscale）");
  const port = Number(d.port);
  if (!Number.isInteger(port) || port < 1 || port > 65535) problems.push("端口须在 1..65535");
  const hasCred = d.credMode === "key" ? !!d.keyFile : !!d.password;
  if (d.mode === "create" && !hasCred) problems.push("登记必须给凭证（密码或密钥文件）—— 上游要拿它先连一次");
  if (tags.some((t) => !TAG_RE.test(t))) problems.push("标签只许 [a-z0-9-]+");

  if (problems.length) {
    d.errors = problems;
    renderForm();
    return;
  }

  const body = {
    name: d.name.trim(),
    ips: ips.map((i) => ({ label: i.label.trim(), addr: i.addr.trim() })),
    user: d.user.trim() || undefined,
    port,
    note: d.note.trim() || undefined,
    tags,
  };
  if (hasCred) {
    if (d.credMode === "key") body.keyFile = d.keyFile;
    else body.password = d.password;
  }

  S.busy = true;
  S.busyLabel = "连线测试中…";
  renderForm();

  const path = d.mode === "edit" ? `api/vps/${encodeURIComponent(d.name)}` : "api/vps";
  const r = await api(path, { method: d.mode === "edit" ? "PUT" : "POST", body: JSON.stringify(body) });

  // 凭证从内存里立刻清掉，成功失败都不留。
  d.password = "";
  d.keyFile = null;
  S.busy = false;

  if (!r.ok) {
    d.errors = r.errors.length ? r.errors : [`上游回了 ${r.status}`];
    renderForm();
    toast(d.errors[0], "err");
    return;
  }

  const name = d.name.trim();
  // 上游可能「写进去了、没推上 GitHub」。那不是失败，但也不能当没事：照样落一条，长一点。
  const warns = (r.body?.warnings ?? []).filter(Boolean);
  toast(d.mode === "edit" ? `已覆盖登记 ${name}` : `已登记 ${name}`, warns.length ? "warn" : "ok");
  for (const w of warns) toast(w, "warn");
  S.mode = "detail";
  S.selected = name;
  S.draft = null;
  await load(true);
  renderDrawer();
}

async function removeMachine(name) {
  S.busy = true;
  const r = await api(`api/vps/${encodeURIComponent(name)}`, { method: "DELETE" });
  S.busy = false;
  if (!r.ok) {
    toast(r.errors[0] ?? "删除失败", "err");
    renderDrawer();
    return;
  }
  toast(`${name} 已从编目里删掉`);
  closeDrawer();
  await load(true);
}

/** 密码型 → 密钥型：密码只用一次，用一次的目的就是让它变成密钥型。 */
async function bootstrapKey(m) {
  const addr = m.ips[0]?.addr ?? "?";
  const ok = confirm(
    `用编目里记着的密码，给 ${m.name}（${m.user}@${addr}:${m.port}）装一把新公钥？\n\n` +
      "装好并验通之后才覆盖登记；那一步会**把编目里的密码清掉**（上游的凭证闸是二选一），这台就变成密钥型。\n\n" +
      "撤回来：目标机 ~/.ssh/authorized_keys 里删掉 nous@" + m.name + " 那一行。",
  );
  if (!ok) return;
  S.busy = true;
  toast(`装公钥中：连 ${addr} + 覆盖登记，最多 90 秒`);
  const r = await api(`api/vps/${encodeURIComponent(m.name)}/bootstrap-key`, { method: "POST" });
  S.busy = false;
  if (!r.ok) {
    toast(r.errors[0] ?? "装公钥失败", "err");
    return renderDrawer();
  }
  toast(`${m.name} 现在是密钥型（${r.data?.fingerprint ?? "已登记"}）—— 密码已从编目里清掉`);
  closeDrawer();
  await load(true);
}

async function addTag(name, tag, quiet = false) {
  const r = await api(`api/vps/${encodeURIComponent(name)}/tags`, { method: "POST", body: JSON.stringify({ name: tag }) });
  if (!r.ok) {
    if (!quiet) return toast(r.errors[0] ?? "挂标签失败", "err");
    throw new Error(r.errors[0] ?? `挂标签失败：${tag}`);
  }
  if (!quiet) toast(`给 ${name} 挂上 ${tag}`);
  await load(true);
}

async function removeTag(name, tag, quiet = false) {
  const r = await api(`api/vps/${encodeURIComponent(name)}/tags/${encodeURIComponent(tag)}`, { method: "DELETE" });
  if (!r.ok) {
    if (!quiet) return toast(r.errors[0] ?? "摘标签失败", "err");
    throw new Error(r.errors[0] ?? `摘标签失败：${tag}`);
  }
  if (!quiet) toast(`从 ${name} 摘掉 ${tag}`);
  await load(true);
}

// ── 右键菜单 ──────────────────────────────────────────────────────────────
//
// 为什么自己画：原生菜单里放不进「挂 important」这种带状态的项，也排不出「删除单独一栏、
// 和别的动作隔开」。品种就三样：头部（说这是谁）、分隔线、动作。
//
// 三个入口走同一个 openMenu：鼠标右键、键盘菜单键（Shift+F10 / 菜单键）、触屏长按 550ms——
// 手机上没有右键，不给等价入口等于手机上少一半功能。
//
// 菜单里的动作一律拿「打开菜单时的那台机器」，不看 S.selected：右键不该顺手把抽屉打开
// （抽屉一开画布重排，菜单底下的东西就在脚底下动了）。

const MENU_HEAD_MAX = 34; // 头部那行地址太长就截断，菜单不能被一条 IPv6 撑宽

function openMenu(x, y, items, back = null) {
  const menu = el("menu");
  const reg = [];
  menu.innerHTML = items
    .map((it) => {
      if (it === "-") return `<div class="m-sep" role="separator"></div>`;
      if (it.head) {
        return `<div class="m-head">
          <span class="m-t">${esc(it.head.t)}</span>
          ${it.head.s ? `<span class="m-s mono" title="${esc(it.head.s)}">${esc(cut(it.head.s, MENU_HEAD_MAX))}</span>` : ""}
        </div>`;
      }
      const i = reg.push(it) - 1;
      return `<button class="mi" role="menuitem" data-mi="${i}"${it.often ? ' data-often="1"' : ""}>
        <span class="m-l">${esc(it.label)}</span>
        ${it.hint ? `<span class="m-h mono">${esc(it.hint)}</span>` : ""}
      </button>`;
    })
    .join("");

  S.menu = { items: reg, back: back ?? document.activeElement };
  menu.hidden = false;
  // 先量再放：菜单宽度由内容定，猜不出来。放不下就翻到光标另一侧，别跑出窗口。
  const w = menu.offsetWidth;
  const h = menu.offsetHeight;
  const left = Math.max(8, Math.min(x + 2, window.innerWidth - w - 8));
  const top = Math.max(8, Math.min(y + 2, window.innerHeight - h - 8));
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  ($(".mi:not([disabled])", menu) ?? menu).focus({ preventScroll: true });
}

function closeMenu(refocus = false) {
  const menu = el("menu");
  if (menu.hidden) return;
  const back = S.menu?.back;
  S.menu = null;
  menu.hidden = true;
  menu.innerHTML = "";
  if (refocus && back?.isConnected) back.focus({ preventScroll: true });
}

const cut = (s, n) => (s.length > n ? `${s.slice(0, n - 1)}…` : s);

async function runMenu(el2) {
  const it = S.menu?.items[Number(el2.dataset.mi)];
  closeMenu(true);
  if (!it || it.disabled) return;
  try {
    await it.run();
  } catch (e) {
    toast(String(e.message ?? e), "err");
  }
}

function machineMenu(m) {
  const addr = m.ips[0] ? `${m.ips[0].addr}:${m.port}` : "没有地址";
  const items = [
    { head: { t: m.name, s: `${m.user}@${addr}` } },
    "-",
    { label: "打开详情", run: () => openDetail(m.name) },
    {
      label: m.reach?.[0] ? "重新探测" : "探测一次",
      run: async () => {
        await probeAll(true, [m]);
        if (S.selected === m.name && S.mode === "detail") renderDrawer();
      },
    },
    {
      label: "复制 ssh 命令",
      hint: `-p ${m.port}`,
      disabled: !m.ips.length,
      run: () => copy(`ssh -p ${m.port} ${m.user}@${m.ips[0].addr}`),
    },
    {
      label: m.ips.length > 1 ? `复制 ${m.ips.length} 条地址` : "复制地址",
      disabled: !m.ips.length,
      run: () => copy(m.ips.map((i) => i.addr).join("\n")),
    },
    "-",
    { label: "覆盖登记…", run: () => openForm("edit", m) },
  ];

  // 标签直接摆成动作，不做二级菜单：二级菜单在触屏上就是「点进去、点回来、点丢了」。
  const known = S.tags.map((t) => t.name);
  const own = m.tags ?? [];
  for (const t of own) items.push({ label: `摘掉标签 ${t}`, run: () => removeTag(m.name, t) });
  for (const t of known) {
    if (!own.includes(t)) items.push({ label: `挂上标签 ${t}`, run: () => addTag(m.name, t) });
  }

  items.push("-");
  items.push({
    // 这里不直接删：删除的两段确认长在抽屉里，第二套确认就是第二套谎话。
    label: "从编目里删除…",
    kind: "danger",
    run: () => {
      openDetail(m.name);
      S.deleteArm = true;
      renderDrawer();
      $('[data-act="delete-yes"]', el("drawer"))?.focus({ preventScroll: true });
    },
  });
  return items;
}

function groupMenu(tag, list) {
  const items = [
    { head: { t: tag || "未分组", s: `${list.length} 台` } },
    "-",
    { label: `探测这一组（${list.length}）`, run: () => probeAll(false, list) },
  ];
  if (list.length) {
    items.push({ label: "复制机器名", run: () => copy(list.map((m) => m.name).join(" ")) });
  }
  for (const t of S.tags.map((x) => x.name).filter((n) => n !== tag).slice(0, 3)) {
    const names = list.map((m) => m.name);
    // 一次请求一次刷新，组大了就是刷屏式的白活。超过 6 台就不提供这条，让他们写接口。
    if (names.length === 0 || names.length > 6) break;
    items.push({ label: `把整组挂到 ${t}`, run: () => Promise.all(names.map((n) => addTag(n, t, true))) });
  }
  return items;
}

function hubMenu() {
  const base = S.upstream.base ?? "";
  return [
    { head: { t: "nw-panel", s: base.replace(/^https?:\/\//, "") } },
    "-",
    { label: "打开面板 ↗", disabled: !base, run: () => window.open(base, "_blank", "noreferrer") },
    { label: "检查一次", run: () => loadHealth() },
    { label: "刷新清单", run: () => load(true) },
    { label: "打开 api/vps ↗", disabled: !base, run: () => window.open(`${base}/api/vps`, "_blank", "noreferrer") },
    "-",
    { label: "复制 catalogPath", disabled: !S.upstream.catalogPath, run: () => copy(S.upstream.catalogPath) },
  ];
}

function canvasMenu() {
  return [
    { head: { t: "画布", s: `${S.machines.length} 台机器` } },
    "-",
    { label: "登记服务器…", run: () => openForm("create") },
    { label: "刷新清单", often: true, run: () => load(true) },
    { label: "检查上游", run: () => loadHealth() },
    "-",
    {
      label: "复制整份清单 JSON",
      run: () =>
        copy(
          JSON.stringify(
            { machines: S.machines.map((m) => ({ ...m, reach: undefined })), tags: S.tags, upstream: S.upstream.base },
            null,
            2,
          ),
        ),
    },
    { label: "打开 api/catalog ↗", run: () => window.open(new URL("api/catalog", ROOT).toString(), "_blank", "noreferrer") },
  ];
}

// ── 事件 ──────────────────────────────────────────────────────────────────

document.addEventListener("click", (e) => {
  // 长按开菜单后，手指抬起时浏览器还会补一个 click —— 吞掉它，
  // 否则「长按看菜单」会变成「长按看菜单 + 把抽屉也打开」。必须在 closeMenu 之前判。
  if (S.touched) {
    S.touched = false;
    return;
  }
  if (e.target.closest("#menu")) return; // 菜单自己的点击在 #menu 上处理
  closeMenu();

  const node = e.target.closest("[data-node]");
  if (node) return openDetail(node.dataset.node);

  const copyBtn = e.target.closest("[data-copy]");
  if (copyBtn) return copy(copyBtn.dataset.copy);

  const cred = e.target.closest("[data-cred]");
  if (cred && S.draft) {
    S.draft.credMode = cred.dataset.cred;
    return renderForm();
  }

  const untag = e.target.closest("[data-untag]");
  if (untag) return removeTag(S.selected, untag.dataset.untag);

  const act = e.target.closest("[data-act]")?.dataset.act;
  if (!act) return;

  const m = S.selected ? machine(S.selected) : null;

  if (act === "add") return openForm("create");
  if (act === "close") return closeDrawer();
  if (act === "edit" && m) return openForm("edit", m);
  if (act === "submit") return submitForm();
  if (act === "probe" && m) return probeAll(true).then(() => renderDrawer());
  if (act === "bootstrap" && m) return bootstrapKey(m);
  if (act === "delete") {
    S.deleteArm = true;
    return renderDrawer();
  }
  if (act === "delete-no") {
    S.deleteArm = false;
    return renderDrawer();
  }
  if (act === "delete-yes" && m) return removeMachine(m.name);
  if (act === "tag-add" && m) {
    const tag = $("[data-tagpick]", el("drawer"))?.value;
    if (!tag) return toast("先选一个标签", "err");
    return addTag(m.name, tag);
  }
  if (act === "ip-add" && S.draft) {
    S.draft.ips.push({ label: "", addr: "" });
    S.draft.ips = S.draft.ips;
    return renderForm();
  }
  if (act === "ip-del" && S.draft) {
    const idx = Number(e.target.closest("[data-idx]").dataset.idx);
    S.draft.ips.splice(idx, 1);
    if (!S.draft.ips.length) S.draft.ips.push({ label: "公网", addr: "" });
    return renderForm();
  }
});

document.addEventListener("input", (e) => {
  const d = S.draft;
  if (!d) return;

  const ipInput = e.target.closest("[data-ip]");
  if (ipInput) {
    const idx = Number(ipInput.closest("[data-idx]").dataset.idx);
    d.ips[idx][ipInput.dataset.ip] = e.target.value;
    return;
  }

  const f = e.target.dataset.f;
  if (!f) return;
  if (f === "tags") d.tags = e.target.value.split(/[\s,]+/).filter(Boolean);
  else if (f === "keyFile") {
    // 文件只读一次、只留 base64；不显示内容、不回填。
    const file = e.target.files?.[0];
    if (!file) return;
    const reader = new FileReader();
    reader.onload = () => {
      const bytes = new Uint8Array(reader.result);
      let bin = "";
      for (let i = 0; i < bytes.length; i += 32768) bin += String.fromCharCode(...bytes.subarray(i, i + 32768));
      d.keyFile = { name: file.name, content: btoa(bin) };
      renderForm();
    };
    reader.readAsArrayBuffer(file);
  } else d[f] = e.target.value;
});

el("menu").addEventListener("click", (e) => {
  const mi = e.target.closest("[data-mi]");
  if (mi) runMenu(mi);
});

// ── 右键 ─────────────────────────────────────────────────────────────────
//
// 判定顺序有讲究：先认机器卡，再认组头，最后才是「画布空白」。反过来的话，
// 任何一张卡都会先撞上「它在画布里」那条，变成三个一模一样的空白菜单。

document.addEventListener("contextmenu", (e) => {
  const canvas = el("canvas");
  if (!canvas.contains(e.target)) return; // 抽屉、页脚里的右键还给浏览器

  const node = e.target.closest(".node[data-node]");
  if (node) {
    const m = machine(node.dataset.node);
    if (!m) return;
    e.preventDefault();
    return openMenu(e.clientX, e.clientY, machineMenu(m), node);
  }
  if (e.target.closest("#hub")) {
    e.preventDefault();
    return openMenu(e.clientX, e.clientY, hubMenu(), el("hub"));
  }

  const group = e.target.closest(".group[data-tag]");
  if (group) {
    const tag = group.dataset.tag;
    const list = S.machines.filter((m) => (m.tags[0] ?? "") === tag);
    e.preventDefault();
    return openMenu(e.clientX, e.clientY, groupMenu(tag, list), $(".g-head", group));
  }

  e.preventDefault();
  openMenu(e.clientX, e.clientY, canvasMenu());
});

// ── 长按 = 右键（触屏） ────────────────────────────────────────────────────

let press = 0;
const cancelPress = () => {
  if (press) clearTimeout(press);
  press = 0;
};

document.addEventListener("pointerdown", (e) => {
  if (e.pointerType === "mouse") return;
  S.touched = false;
  const node = e.target.closest(".node[data-node]");
  if (!node) return;
  const { clientX: x, clientY: y } = e;
  press = setTimeout(() => {
    press = 0;
    S.touched = true;
    navigator.vibrate?.(8);
    const m = machine(node.dataset.node);
    if (m) openMenu(x, y, machineMenu(m), node);
  }, 550);
});
document.addEventListener("pointerup", cancelPress);
document.addEventListener("pointercancel", cancelPress);
document.addEventListener("pointermove", (e) => {
  // 手指一动就是在滚画布，不是想长按。
  if (press && (Math.abs(e.movementX) > 2 || Math.abs(e.movementY) > 2)) cancelPress();
});

// 菜单是 fixed 的，滚动后它就会飘在另一个东西上面 —— 直接收掉。
window.addEventListener("scroll", () => closeMenu(), { passive: true, capture: true });
window.addEventListener("resize", () => closeMenu());

$("#health").addEventListener("click", loadHealth);
$("#hub").addEventListener("click", loadHealth);
$("#refresh").addEventListener("click", () => load(true));
$("#add").addEventListener("click", () => openForm("create"));

el("docs-link").addEventListener("click", (e) => {
  // 在宿主里它是 /p/<board>/<id>/api/catalog —— 相对链接就够了。
  e.currentTarget.href = new URL("api/catalog", ROOT).toString();
});

document.addEventListener("keydown", (e) => {
  if (S.menu) {
    const items = $$(".mi:not([disabled])", el("menu"));
    const at = items.indexOf(document.activeElement);
    if (e.key === "Escape") {
      e.preventDefault();
      return closeMenu(true);
    }
    if (e.key === "Tab") return closeMenu();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = e.key === "ArrowDown" ? at + 1 : at - 1;
      (items[(next + items.length) % items.length] ?? items[0])?.focus({ preventScroll: true });
      return;
    }
    if (e.key === "Home" || e.key === "End") {
      e.preventDefault();
      (e.key === "Home" ? items[0] : items[items.length - 1])?.focus({ preventScroll: true });
      return;
    }
  }

  // 键盘菜单键：跟鼠标右键汇到同一个菜单上。
  if ((e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) && document.activeElement?.closest) {
    const node = document.activeElement.closest(".node[data-node]");
    const m = node ? machine(node.dataset.node) : null;
    if (m) {
      e.preventDefault();
      const r = node.getBoundingClientRect();
      return openMenu(r.left + 18, r.top + r.height / 2, machineMenu(m), node);
    }
  }

  if (e.key !== "Escape") return;
  if (S.mode === "create" || S.mode === "edit") return closeDrawer();
  if (S.selected) return closeDrawer();
});

let raf = 0;
const redraw = () => {
  cancelAnimationFrame(raf);
  raf = requestAnimationFrame(drawEdges);
};
new ResizeObserver(redraw).observe(el("canvas"));
window.addEventListener("resize", redraw);
window.addEventListener("scroll", redraw, { passive: true });

renderAll();
loadHealth();
load();
