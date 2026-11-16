// k3skws element 页面：左边清单 / 中间编辑 / 右边 JSON。
//
// 数据全在 /v1 上：读 `GET v1/state`，写 `PUT v1/item/<kind>/<id>`（整体覆盖），
// 删 `DELETE v1/item/<kind>/<id>`。每一次写都对应仓库里的一条提交 —— 页面不缓存状态，
// 保存完就重读一遍，看见的就是盘上的。
//
// 两种编辑方式：
//   A 表单 —— 表单为主，右侧 JSON 只读预览（默认）
//   B JSON —— JSON 为主（直接打字，写歪了当场报错并禁掉保存），右侧变只读摘要
//
// 字段不做枚举：除了 id / name 两个必填，其余全由「加字段」那块决定。

const $ = (s) => document.querySelector(s);
const API = "v1";
const BASE = ["id", "name", "desc"];

let state = { groups: [], packages: [] };
let repo = null;
let sel = { kind: "group", id: null };
let draft = null;
let dirty = false;
let broken = null;
let mode = "a";

/* ── 小工具 ─────────────────────────────────────────────────────── */

const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const clone = (o) => JSON.parse(JSON.stringify(o));
const slug = (s) => (s || "").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 40);
const stripMeta = (o) => {
  const out = { ...o };
  delete out.__mtime;
  delete out.__bytes;
  delete out.__broken;
  return out;
};
const listOf = (kind) => (kind === "group" ? state.groups : state.packages);
const findItem = (kind, id) => listOf(kind).find((x) => x.id === id);

function toast(text, kind = "ok") {
  const el = document.createElement("div");
  el.textContent = text;
  if (kind === "bad") el.style.background = "#b03a2e";
  $("#toast").appendChild(el);
  setTimeout(() => el.remove(), 3400);
}

function banner(text) {
  const el = $("#banner");
  if (!text) {
    el.classList.remove("on");
    el.textContent = "";
    return;
  }
  el.classList.add("on");
  el.textContent = text;
}

/* ── 读 ─────────────────────────────────────────────────────────── */

async function load({ keep = true } = {}) {
  const res = await fetch(`${API}/state`);
  if (!res.ok) throw new Error(`读 state 失败：HTTP ${res.status}`);
  const data = await res.json();
  state.groups = data.groups ?? [];
  state.packages = data.packages ?? [];
  repo = data.repo ?? null;

  const stillThere = keep && sel.id && findItem(sel.kind, sel.id);
  if (stillThere) open(sel.kind, sel.id, { refetch: false });
  else {
    const first = state.groups[0] ?? state.packages[0];
    if (first && !first.__broken) {
      open(first.__broken ? "package" : state.groups[0] ? "group" : "package", first.id, { refetch: false });
    } else {
      sel = { kind: sel.kind, id: null };
      draft = null;
      dirty = false;
      renderAll();
    }
  }
  fillSuggestions();
}

/* ── 选中 / 草稿 ────────────────────────────────────────────────── */

function open(kind, id, { refetch = true } = {}) {
  const item = findItem(kind, id);
  if (!item) return;
  sel = { kind, id };
  dirty = false;
  broken = item.__broken ?? null;
  if (broken) {
    // 坏了就别装能编辑：给个只有 id 的草稿，保存按钮禁掉，删除照常可用。
    draft = { id, name: id };
    banner(`这份 JSON 读不出来：${broken} —— 可以删掉重建，或去仓库里手改那一份。`);
  } else {
    draft = stripMeta(item);
    banner("");
  }
  if (refetch) renderAll();
}

function newItem(kind) {
  sel = { kind, id: null };
  broken = null;
  banner("");
  draft = kind === "group"
    ? { id: "", name: "", desc: "", packages: [] }
    : { id: "", name: "", desc: "", kind: "prompt", tags: [] };
  dirty = true;
  renderAll();
  setTimeout(() => document.querySelector('.field input[data-k="id"]')?.focus(), 0);
}

/* ── 渲染：清单 ─────────────────────────────────────────────────── */

function renderRail() {
  const q = $("#search").value.trim().toLowerCase();
  const hit = (o) => !q || JSON.stringify(o).toLowerCase().includes(q);

  const draw = (kind, host) => {
    const rows = listOf(kind).filter(hit);
    host.innerHTML = rows.map((o) => {
      const selected = sel.kind === kind && sel.id === o.id;
      const isDirty = selected && dirty;
      const mark = o.__broken ? '<span class="bad">坏</span>' : isDirty ? '<span class="dirty" title="有未保存改动"></span>' : "";
      return `<div class="row" ${selected ? 'aria-selected="true"' : ""} data-kind="${kind}" data-id="${esc(o.id)}" role="option">
        <span class="nm">${esc(o.name || o.id || "(未命名)")}</span>
        <span class="id mono">${esc(o.id || "—")}</span>
        ${mark}
      </div>`;
    }).join("") || `<div class="empty small">${q ? "没有匹配" : "还没有"}</div>`;
  };

  draw("group", $("#rowsGroup"));
  draw("package", $("#rowsPackage"));
  $("#nGroup").textContent = state.groups.length;
  $("#nPackage").textContent = state.packages.length;
  $("#count").textContent = `${state.groups.length} 组 · ${state.packages.length} 包`;
}

function fillSuggestions() {
  $("#pkgList").innerHTML = state.packages.map((p) => `<option value="${esc(p.id)}">`).join("");
}

/* ── 渲染：编辑区 ───────────────────────────────────────────────── */

function renderEditor() {
  const kind = sel.kind;
  $("#badge").textContent = kind === "group" ? "组" : "包";
  $("#badge").dataset.kind = kind;
  $("#headId").textContent = draft?.id || "(新)";
  $("#filePath").textContent = `${kind === "group" ? "groups" : "packages"}/${draft?.id || "<id>"}.json`;

  const host = $("#form");
  host.innerHTML = "";
  if (!draft) return;

  const core = [
    ["id", "文件名", draft.id === "" ? "小写、连字符，存盘即定" : "改名 = 换 id，引用会断"],
    ["name", "显示名", ""],
    ["desc", "说明", ""],
  ];
  for (const [k, hint, ph] of core) host.appendChild(fieldRow(k, hint, ph));
  for (const k of Object.keys(draft)) {
    if (!BASE.includes(k)) host.appendChild(fieldRow(k, "", ""));
  }

  const known = new Set(Object.keys(draft));
  const add = document.createElement("div");
  add.className = "addfield";
  add.innerHTML = `<input id="fk" placeholder="新字段名（英文，如 notes / weight / url）" />
    <select id="ft">
      <option value="string">文本</option>
      <option value="number">数字</option>
      <option value="boolean">开关</option>
      <option value="array">列表</option>
      <option value="object">对象</option>
    </select>
    <button class="btn" id="fadd">＋ 加字段</button>`;
  host.appendChild(add);

  $("#fadd").onclick = () => {
    const k = $("#fk").value.trim().replace(/[^\w-]/g, "");
    const t = $("#ft").value;
    if (!k) return toast("先起个字段名", "bad");
    if (known.has(k)) return toast(`「${k}」已经有了`, "bad");
    draft[k] = t === "number" ? 0 : t === "boolean" ? false : t === "array" ? [] : t === "object" ? {} : "";
    dirty = true;
    renderAll();
  };
  $("#fk").addEventListener("keydown", (e) => { if (e.key === "Enter") $("#fadd").click(); });
}

function fieldRow(key, hint, placeholder) {
  const row = document.createElement("div");
  row.className = "field";
  const val = draft[key];
  const kind = Array.isArray(val) ? "array" : val === null ? "string" : typeof val;
  const labels = { string: "文本", number: "数字", boolean: "开关", array: "列表", object: "对象" };
  const locked = key === "id" || broken;

  row.innerHTML = `<div class="k"><span class="kname">${esc(key)}</span>
      <span class="khint">${esc(hint || labels[kind] || kind)}</span></div>
    <div class="v"></div>${locked ? "" : '<button class="kill" title="删掉这个字段">×</button>'}`;

  const v = row.querySelector(".v");
  if (kind === "boolean") {
    v.innerHTML = `<label class="small dim" style="display:flex;gap:6px;align-items:center;padding-top:6px">
      <input type="checkbox" ${val ? "checked" : ""} style="width:auto"> ${val ? "开" : "关"}</label>`;
    v.querySelector("input").onchange = (e) => { draft[key] = e.target.checked; touch(); };
  } else if (kind === "number") {
    v.innerHTML = `<input type="number" value="${val}" style="max-width:180px" ${broken ? "disabled" : ""}>`;
    v.querySelector("input").oninput = (e) => { draft[key] = Number(e.target.value || 0); touch(); };
  } else if (kind === "array") {
    v.innerHTML = `<div class="chips"></div>`;
    const chips = v.querySelector(".chips");
    const drawChips = () => {
      chips.innerHTML = draft[key]
        .map((x) => `<span class="chip">${esc(x)}<button data-x="${esc(x)}" title="移除">×</button></span>`)
        .join("")
        + (broken ? "" : `<span class="chip-add"><input placeholder="＋ 加一项" ${key === "packages" ? 'list="pkgList"' : ""}></span>`);
      chips.querySelectorAll("button[data-x]").forEach((b) => {
        b.onclick = () => { draft[key] = draft[key].filter((x) => x !== b.dataset.x); touch(); drawChips(); };
      });
      const input = chips.querySelector(".chip-add input");
      if (!input) return;
      input.onkeydown = (e) => {
        if (e.key !== "Enter" && e.key !== ",") return;
        e.preventDefault();
        const x = input.value.trim();
        if (!x) return;
        if (!draft[key].includes(x)) draft[key].push(x);
        input.value = "";
        touch();
        drawChips();
      };
    };
    drawChips();
  } else if (kind === "object") {
    v.innerHTML = `<textarea spellcheck="false" ${broken ? "disabled" : ""}></textarea>`;
    const ta = v.querySelector("textarea");
    ta.value = JSON.stringify(val ?? {}, null, 2);
    ta.style.minHeight = "84px";
    ta.oninput = () => {
      try {
        draft[key] = JSON.parse(ta.value || "{}");
        ta.style.borderColor = "";
        touch();
      } catch {
        ta.style.borderColor = "var(--bad)";
      }
    };
  } else {
    v.innerHTML = `<input type="text" data-k="${esc(key)}" value="${esc(val)}" placeholder="${esc(placeholder)}" ${broken ? "disabled" : ""}>`;
    const inp = v.querySelector("input");
    inp.oninput = (e) => {
      let s = e.target.value;
      if (key === "id") { s = slug(s); e.target.value = s; $("#headId").textContent = s || "(新)"; }
      draft[key] = s;
      if (key === "id") $("#filePath").textContent = `${sel.kind === "group" ? "groups" : "packages"}/${s || "<id>"}.json`;
      if (key === "id" || key === "name") renderRailSoon();
      touch();
    };
  }

  const kill = row.querySelector(".kill");
  if (kill) kill.onclick = () => { delete draft[key]; touch(); renderAll(); };
  return row;
}

let railTimer = null;
const renderRailSoon = () => { clearTimeout(railTimer); railTimer = setTimeout(renderRail, 120); };

function touch() {
  dirty = true;
  renderJson();
  renderStatus();
  renderRail();
}

/* ── 渲染：JSON / 摘要 ──────────────────────────────────────────── */

function renderJson() {
  if (!draft) return;
  const text = `${JSON.stringify(draft, null, 2)}\n`;
  $("#json").innerHTML = highlight(text);
  $("#bytes").textContent = `${new Blob([text]).size} B`;
  if (mode === "b" && document.activeElement !== $("#jsonText")) {
    $("#jsonText").value = text.replace(/\n$/, "");
  }
  $("#save").disabled = !validDraft();
  renderSummary();
}

const highlight = (s) =>
  esc(s).replace(/(&quot;(?:\\.|[^&])*?&quot;)(\s*:)?|\b(true|false|null)\b|\b-?\d+(?:\.\d+)?\b/g, (m, str, colon, lit) => {
    if (str) return colon ? `<span class="key">${str}</span><span class="punct">${colon}</span>` : `<span class="str">${str}</span>`;
    if (lit) return `<span class="bool">${lit}</span>`;
    return `<span class="num">${m}</span>`;
  });

function renderSummary() {
  if (!draft) {
    $("#summary").innerHTML = "";
    return;
  }
  const rows = Object.entries(draft).map(([k, v]) => {
    const shown = Array.isArray(v) ? `[${v.join(", ")}]` : v && typeof v === "object" ? JSON.stringify(v) : String(v);
    return `<div class="srow"><span class="sk">${esc(k)}</span><span class="sv">${esc(shown || "—")}</span></div>`;
  }).join("");
  $("#summary").innerHTML = rows + `<div class="note" style="margin-top:14px"><b>B 方案。</b>
    左边是正在编辑的 JSON（改它，这份摘要跟着变）；摘要是只读的，用来对照字段有没有写歪。</div>`;
}

function validDraft() {
  if (!draft || broken) return false;
  if (!draft.id) return false;
  return !listOf(sel.kind).some((o) => o.id === draft.id && o.id !== sel.id);
}

/* ── 渲染：状态 ─────────────────────────────────────────────────── */

function renderStatus() {
  const label = $("#sync .sync-label");
  $("#sync .dot").dataset.state = dirty ? "dirty" : "ok";
  label.textContent = dirty ? "未保存" : "已同步";

  if (repo) {
    $("#repo").textContent = (repo.remote || "").replace(/^.*github\.com[:/]/, "").replace(/\.git$/, "") || repo.path;
    $("#repoPath").textContent = repo.path;
    $("#footSync").textContent = dirty ? "有未保存" : `${repo.branch}@${repo.head}`;
    const ahead = repo.ahead;
    const pending = ahead === null || ahead === undefined
      ? ""
      : ahead > 0
        ? ` · <span style="color:var(--warn)">↑${ahead} 条等待推送</span>
             <button class="btn" id="pushRetry" style="margin-left:6px;padding:1px 8px;font-size:11px">重试推送</button>`
        : ` · 远端已同步`;
    $("#gitLine").innerHTML = `分支 <span class="mono">${esc(repo.branch)}</span> @ <span class="mono">${esc(repo.head)}</span>
      · 仓库里 ${repo.dirty ?? 0} 处未提交${repo.dirty ? "（别人的，不带走）" : ""}
      · 推送 ${repo.push ? "开" : "关"}${pending}`;
    $("#gitLine").querySelector("#pushRetry")?.addEventListener("click", retryPush);
  }

  const kind = sel.kind;
  const kindLabel = kind === "group" ? "groups" : "packages";
  const clash = draft && draft.id && listOf(kind).some((o) => o.id === draft.id && o.id !== sel.id);
  $("#msg").innerHTML = !draft
    ? "从左边挑一份，或者点 ＋ 新建"
    : !draft.id
      ? "还缺 <b>id</b>（就是文件名）"
      : clash
        ? `id <b>${esc(draft.id)}</b> 已经被别的元素占用了`
        : `${dirty ? "保存后会提交" : "已提交"}：<b>feat(${kind}): ${esc(draft.id)}</b> · ${kindLabel}/${esc(draft.id)}.json`;
}

function renderAll() {
  renderRail();
  renderEditor();
  renderJson();
  renderStatus();
}

/* ── 写 ─────────────────────────────────────────────────────────── */

async function save() {
  if (!validDraft() || broken) return;
  const kind = sel.kind;
  const id = draft.id;
  setBusy(true);
  try {
    const res = await fetch(`${API}/item/${kind}/${id}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(draft),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) return toast(`没写进去：${data.error ?? res.status}`, "bad");

    if (!data.commit?.committed) {
      toast(`已写但没生成提交：${data.commit?.error ?? "内容没变化"}`, "bad");
    } else if (data.push && !data.push.pushed) {
      toast(`已提交 ${data.commit.head}，但推送失败：${data.push.error}`, "bad");
    } else if (data.push?.pushed) {
      toast(`已提交并推送 · ${data.commit.message} · ${data.commit.head}`);
    } else {
      toast(`已提交（未推送）· ${data.commit.message} · ${data.commit.head}`);
    }
    sel = { kind, id };
    await load({ keep: true });
  } catch (error) {
    toast(`网络不通：${error.message}`, "bad");
  } finally {
    setBusy(false);
  }
}

async function removeIt() {
  if (!sel.id) {
    draft = null;
    dirty = false;
    renderAll();
    return;
  }
  if (!confirm(`删掉 ${sel.kind}「${sel.id}」？会提交一条 chore 记录。`)) return;
  const kind = sel.kind;
  const id = sel.id;
  setBusy(true);
  try {
    const res = await fetch(`${API}/item/${kind}/${id}`, { method: "DELETE" });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) return toast(`没删掉：${data.error ?? res.status}`, "bad");
    toast(`已删除 · ${kind}: ${id}${data.commit?.committed ? ` · ${data.commit.head}` : ""}`);
    sel = { kind, id: null };
    draft = null;
    await load({ keep: false });
  } catch (error) {
    toast(`网络不通：${error.message}`, "bad");
  } finally {
    setBusy(false);
  }
}

function revertIt() {
  if (!sel.id) {
    draft = null;
    dirty = false;
    renderAll();
    return;
  }
  open(sel.kind, sel.id);
  toast("已还原到仓库里的那份");
}

async function retryPush() {
  const btn = $("#pushRetry");
  if (btn) {
    btn.disabled = true;
    btn.textContent = "推送中…";
  }
  try {
    const res = await fetch(`${API}/push`, { method: "POST" });
    const data = await res.json().catch(() => ({}));
    if (data.push?.pushed) toast("推送成功");
    else toast(`还是推不上去：${data.push?.error ?? res.status}`, "bad");
    await load({ keep: true });
  } catch (error) {
    toast(`网络不通：${error.message}`, "bad");
  }
}

function setBusy(on) {
  $("#save").classList.toggle("busy", on);
  $("#del").classList.toggle("busy", on);
  $("#sync .sync-label").textContent = on ? "写入中…" : dirty ? "未保存" : "已同步";
}

/* ── 事件 ───────────────────────────────────────────────────────── */

$("#rowsGroup").addEventListener("click", (e) => {
  const r = e.target.closest(".row");
  if (r) open("group", r.dataset.id);
});
$("#rowsPackage").addEventListener("click", (e) => {
  const r = e.target.closest(".row");
  if (r) open("package", r.dataset.id);
});
$("#newGroup").onclick = () => newItem("group");
$("#newPackage").onclick = () => newItem("package");
$("#search").oninput = renderRail;
$("#save").onclick = save;
$("#del").onclick = removeIt;
$("#revert").onclick = revertIt;
$("#refresh").onclick = async () => {
  if (dirty && !confirm("有没保存的改动，刷新会丢掉它们，继续？")) return;
  await load({ keep: false });
  toast("已重读仓库");
};
$("#copy").onclick = async () => {
  try {
    await navigator.clipboard.writeText($("#json").innerText);
    toast("JSON 已复制");
  } catch {
    toast("剪贴板被拒了，手动选吧", "bad");
  }
};

document.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
    e.preventDefault();
    save();
  }
});

$("#jsonText").addEventListener("input", (e) => {
  try {
    draft = JSON.parse(e.target.value);
    $("#err").style.display = "none";
    dirty = true;
    renderJson();
    renderRail();
    renderStatus();
  } catch (error) {
    const el = $("#err");
    el.style.display = "block";
    el.textContent = String(error.message);
    $("#save").disabled = true;
  }
});

function setMode(m) {
  mode = m;
  document.body.dataset.mode = m;
  $("#mA").setAttribute("aria-pressed", String(m === "a"));
  $("#mB").setAttribute("aria-pressed", String(m === "b"));
  $("#sideTitle").textContent = m === "a" ? "JSON" : "字段摘要";
  if (m === "b" && draft) $("#jsonText").value = JSON.stringify(draft, null, 2);
  renderJson();
}
$("#mA").onclick = () => setMode("a");
$("#mB").onclick = () => setMode("b");

/* ── 启动 ───────────────────────────────────────────────────────── */

load({ keep: false }).catch((error) => {
  banner(`读不出来：${error.message} —— 先看后端有没有起来（v1/health）。`);
  toast("页面加载失败", "bad");
});
