// extend · 台账页逻辑。全部跟着 /api/inventory 走，动作之后重扫一遍。

const $ = (sel, root = document) => root.querySelector(sel);

// 块挂在 /p/<board>/<id>/ 下面，写 "/api/xxx" 会打到**宿主根**去（那边回的是 HTML，
// JSON.parse 直接炸，整个台账一行为空）。所有请求都得挂在这一块自己的路径上。
const SELF = location.pathname.endsWith("/") ? location.pathname : location.pathname + "/";

const ledger = $("#ledger");
const empty = $("#empty");
const toastEl = $("#toast");
let toastTimer;

function toast(msg, kind = "") {
  toastEl.textContent = msg;
  toastEl.dataset.kind = kind;
  toastEl.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (toastEl.hidden = true), 2600);
}

async function api(path, body) {
  const res = await fetch(SELF + String(path).replace(/^\//, ""), body ? { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) } : undefined);
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || res.status);
  return data;
}

function setDot(state) {
  const dot = $("#refresh .dot");
  dot.dataset.state = state;
}

// ── 渲染 ────────────────────────────────────────────────────────────────────

function rowEl(row) {
  const el = document.createElement("article");
  el.className = "row" + (row.lib ? "" : " uncollected");
  el.dataset.id = row.id ?? "";

  const head = document.createElement("div");
  head.className = "row-head";

  const name = document.createElement("span");
  name.className = "row-name";
  name.textContent = row.name;
  head.append(name);

  const scope = document.createElement("span");
  scope.className = "scope-tag";
  scope.textContent = row.scope;
  head.append(scope);

  if (!row.lib) {
    const un = document.createElement("span");
    un.className = "dim small";
    un.textContent = "未收编 —— 盘上有，库里没有";
    head.append(un);
  }

  el.append(head);

  // 安装位
  const ul = document.createElement("ul");
  ul.className = "row-installs";
  for (const inst of row.installs) {
    const li = document.createElement("li");
    li.className = "inst";

    const on = document.createElement("span");
    on.className = "state";
    on.dataset.on = inst.disabled ? "0" : "1";
    on.textContent = inst.disabled ? "disabled" : "enabled";
    li.append(on);

    const p = document.createElement("span");
    p.className = "inst-path";
    p.textContent = inst.dir;
    p.title = inst.mtime ? "盘上改动时间 " + new Date(inst.mtime).toLocaleString() : "";
    li.append(p);

    const actions = document.createElement("span");
    actions.className = "row-actions";

    const tgl = document.createElement("button");
    tgl.className = "btn";
    tgl.textContent = inst.disabled ? "启用" : "停用";
    tgl.onclick = async () => {
      try {
        await api("/api/toggle", { dir: inst.dir, name: row.name, enable: !!inst.disabled });
        toast(inst.disabled ? "已启用，pi 里 /reload 生效" : "已停用，pi 里 /reload 生效", "ok");
        load();
      } catch (e) {
        toast(e.message, "bad");
      }
    };
    actions.append(tgl);

    if (row.lib) {
      const pull = document.createElement("button");
      pull.className = "btn";
      pull.textContent = "收回库";
      pull.title = "盘上的这份可能改过 —— 拷回库里覆盖";
      pull.onclick = async () => {
        try {
          await api("/api/pull", { id: row.id, dir: inst.dir });
          toast("已收回库里", "ok");
          load();
        } catch (e) {
          toast(e.message, "bad");
        }
      };
      actions.append(pull);
    } else {
      const adopt = document.createElement("button");
      adopt.className = "btn primary";
      adopt.textContent = "收进库";
      adopt.title = "把这份源文件拷进块内库（data/lib/）";
      adopt.onclick = async () => {
        try {
          await api("/api/adopt", { dir: inst.dir, name: row.name });
          toast("已收进库", "ok");
          load();
        } catch (e) {
          toast(e.message, "bad");
        }
      };
      actions.append(adopt);
    }

    li.append(actions);
    ul.append(li);
  }
  if (row.installs.length === 0) {
    const li = document.createElement("li");
    li.className = "inst dim small";
    li.textContent = "库里存着，盘上还没装到任何地方";
    ul.append(li);
  }
  el.append(ul);

  // 库内源文件
  if (row.lib) {
    const files = document.createElement("div");
    files.className = "row-installs";
    for (const f of row.lib) {
      const line = document.createElement("div");
      line.className = "inst";
      const a = document.createElement("a");
      a.href = "#";
      a.className = "inst-path";
      a.textContent = "lib/" + row.id + "/" + f.name;
      a.onclick = (ev) => {
        ev.preventDefault();
        openFactory(row.id, f.name);
      };
      line.append(a);
      files.append(line);
    }
    const installBtn = document.createElement("div");
    installBtn.className = "inst";
    installBtn.innerHTML = "";
    const b = document.createElement("button");
    b.className = "btn";
    b.textContent = "装到…";
    b.onclick = () => installDialog(row);    installBtn.append(b, Object.assign(document.createElement("span"), { className: "dim small", textContent: "拷到某个白名单目录" }));
    const rm = document.createElement("button");
    rm.className = "btn danger";
    rm.style.marginLeft = "auto";
    rm.textContent = "删库条目";
    rm.title = "只删台账和库内文件，不碰安装位";
    rm.onclick = async () => {
      if (!confirm("删掉库里 " + row.name + "？安装位上的文件不受影响。")) return;
      try {
        await api("/api/remove", { id: row.id });
        toast("已删除", "ok");
        load();
      } catch (e) {
        toast(e.message, "bad");
      }
    };
    installBtn.append(rm);
    files.append(installBtn);
    el.append(files);
  }

  // 备注
  const notes = document.createElement("div");
  notes.className = "notes";
  const input = document.createElement("input");
  input.type = "text";
  input.placeholder = "备注（这个扩展是干嘛的、为什么留下）";
  input.value = row.notes ?? "";
  let saveTimer;
  input.oninput = () => {
    clearTimeout(saveTimer);
    saveTimer = setTimeout(async () => {
      if (!row.id) return;
      try {
        await api("/api/save-note", { id: row.id, notes: input.value });
      } catch (e) {
        toast(e.message, "bad");
      }
    }, 600);
  };
  notes.append(input);
  el.append(notes);

  return el;
}

function installDialog(row) {
  const dirs = [state.globalDir, ...state.watch];
  const picked = prompt(
    "装到哪个目录？\n\n白名单：\n" + dirs.map((d) => "· " + d).join("\n") + "\n\n粘贴完整路径：",
    dirs[0] ?? "",
  );
  if (!picked) return;
  api("/api/install", { id: row.id, dir: picked.trim() })
    .then(() => {
      toast("已装到 " + picked + "，pi 里 /reload 生效", "ok");
      load();
    })
    .catch((e) => toast(e.message, "bad"));
}

async function view(id, file) {
  try {
    const { text } = await api(`/api/file/${encodeURIComponent(id)}/${file}`);
    $("#viewer-title").textContent = "lib/" + id + "/" + file;
    $("#viewer-code").textContent = text;
    $("#viewer").showModal();
  } catch (e) {
    toast(e.message, "bad");
  }
}
$("#viewer-close").onclick = () => $("#viewer").close();

// ── 白名单 ──────────────────────────────────────────────────────────────────

function renderWatch(watch) {
  const ul = $("#watchlist");
  ul.textContent = "";
  for (const dir of watch) {
    const li = document.createElement("li");
    const code = document.createElement("span");
    code.textContent = dir;
    li.append(code);
    const rm = document.createElement("button");
    rm.className = "btn ghost danger";
    rm.textContent = "移除";
    rm.onclick = async () => {
      try {
        await api("/api/watch", { dir, remove: true });
        load();
      } catch (e) {
        toast(e.message, "bad");
      }
    };
    li.append(rm);
    ul.append(li);
  }
  if (!watch.length) {
    const li = document.createElement("li");
    li.className = "dim";
    li.textContent = "（还没有 —— 比如把 /opt/nw-136 加进来）";
    ul.append(li);
  }
}

$("#watch-form").onsubmit = async (ev) => {
  ev.preventDefault();
  const dir = $("#watch-dir").value.trim();
  if (!dir) return;
  try {
    await api("/api/watch", { dir });
    $("#watch-dir").value = "";
    load();
  } catch (e) {
    toast(e.message, "bad");
  }
};

// ── 主循环 ──────────────────────────────────────────────────────────────────

let state = { rows: [], watch: [], globalDir: "" };

async function load() {
  setDot("wait");
  try {
    state = await api("/api/inventory");
    const toolbar = document.createElement("div");
  toolbar.className = "toolbar";
  const h = document.createElement("h2");
  h.textContent = "台账";
  toolbar.append(h);
  ledger.textContent = "";
  ledger.append(toolbar);
  for (const row of state.rows) ledger.append(rowEl(row));
    empty.hidden = state.rows.length > 0;
    $("#count").textContent = state.rows.length + " 个扩展";
    renderWatch(state.watch);
    setDot("ok");
  } catch (e) {
    setDot("wait");
    toast(e.message, "bad");
  }
}

$("#refresh").onclick = load;
load();

// ── 车间：新扩展 + 编辑 + 出厂检查 + 试装 ──────────────────────────────────

const TEMPLATES = [
  { id: "guard", label: "守门员", desc: "hook tool_call，拦下不想让发生的调用" },
  { id: "tool", label: "工具", desc: "registerTool，LLM 能调用的工具，带 typebox 参数" },
  { id: "command", label: "命令", desc: "registerCommand，敲 /xxx 触发" },
  { id: "blank", label: "空白", desc: "只有入口和 import，其余自己写" },
];

const factory = $("#factory");
const editor = $("#f-editor");
let fItem = null; // 当前车间里的 { id, name, file }
let fDirty = false;
let fTimer;

// ── 新扩展弹窗 ──

let pickedTpl = "guard";
function renderTplGrid() {
  const grid = $("#new-tpl");
  grid.textContent = "";
  for (const t of TEMPLATES) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "tpl";
    b.setAttribute("aria-pressed", String(t.id === pickedTpl));
    const strong = document.createElement("strong");
    strong.textContent = t.label;
    const span = document.createElement("span");
    span.textContent = t.desc;
    b.append(strong, span);
    b.onclick = () => {
      pickedTpl = t.id;
      renderTplGrid();
    };
    grid.append(b);
  }
}

$("#new-ext").onclick = () => {
  renderTplGrid();
  $("#new-name").value = "";
  $("#newdlg").showModal();
  $("#new-name").focus();
};
$("#new-cancel").onclick = () => $("#newdlg").close();
$("#newform").onsubmit = async (ev) => {
  ev.preventDefault();
  const name = $("#new-name").value.trim();
  if (!name) return;
  try {
    const item = await api("/api/create", { name, template: pickedTpl, scope: $("#new-scope").value });
    $("#newdlg").close();
    toast("已开档：" + name, "ok");
    await load();
    openFactory(item.id, item.files[0]);
  } catch (e) {
    toast(e.message, "bad");
  }
};

// ── 编辑抽屉 ──

async function openFactory(id, file) {
  try {
    const { text } = await api(`/api/file/${encodeURIComponent(id)}/${file}`);
    const row = state.rows.find((r) => r.id === id);
    fItem = { id, file, name: row?.name ?? id };
    fDirty = false;
    $("#f-file").textContent = `lib/${id}/${file}`;
    $("#f-status").textContent = "";
    editor.value = text;
    renderLint([{ id: "idle", ok: 1, label: "还没检查 —— 改完点「跑一遍检查」", dim: true }]);
    renderTargets();
    renderRevs();
    factory.showModal();
  } catch (e) {
    toast(e.message, "bad");
  }
}
$("#factory-close").onclick = () => {
  if (fDirty && !confirm("还没保存，关掉就丢了。关？")) return;
  factory.close();
};

editor.addEventListener("input", () => {
  fDirty = true;
  $("#f-status").textContent = "未保存";
  clearTimeout(fTimer);
  fTimer = setTimeout(saveFactory, 1200); // 停手 1.2s 自动存
});
editor.addEventListener("keydown", (ev) => {
  if ((ev.metaKey || ev.ctrlKey) && ev.key === "s") {
    ev.preventDefault();
    saveFactory();
  }
  if (ev.key === "Tab") {
    ev.preventDefault();
    const { selectionStart: s, selectionEnd: e, value } = editor;
    editor.value = value.slice(0, s) + "  " + value.slice(e);
    editor.selectionStart = editor.selectionEnd = s + 2;
    fDirty = true;
    $("#f-status").textContent = "未保存";
  }
});

async function saveFactory() {
  if (!fItem) return;
  try {
    const { savedAt } = await api("/api/write", { id: fItem.id, file: fItem.file, text: editor.value });
    fDirty = false;
    $("#f-status").textContent = "已存 " + new Date(savedAt).toLocaleTimeString();
    renderRevs();
  } catch (e) {
    $("#f-status").textContent = "存失败";
    toast(e.message, "bad");
  }
}

// ── 出厂检查 ──

function renderLint(checks) {
  const ul = $("#f-lint");
  ul.textContent = "";
  for (const c of checks) {
    const li = document.createElement("li");
    if (c.dim) li.className = "dim";
    const tick = document.createElement("span");
    tick.className = "tick";
    tick.dataset.ok = c.ok ? "1" : "0";
    tick.textContent = c.ok ? "✓" : "✗";
    li.append(tick, document.createTextNode(c.label));
    ul.append(li);
  }
}

$("#f-check").onclick = async () => {
  await saveFactory();
  const { checks } = await api("/api/lint", { text: editor.value });
  renderLint(checks);
};

// ── 试装 ──

function renderTargets() {
  const sel = $("#f-target");
  sel.textContent = "";
  const dirs = [state.globalDir, ...state.watch];
  for (const d of dirs) {
    const o = document.createElement("option");
    o.value = d;
    o.textContent = d;
    sel.append(o);
  }
}

$("#f-install").onclick = async () => {
  if (!fItem) return;
  await saveFactory();
  const dir = $("#f-target").value;
  if (!dir) return toast("没有可装的目标目录", "bad");
  try {
    await api("/api/install", { id: fItem.id, dir });
    toast("已装到 " + dir + " —— 去 pi 里 /reload", "ok");
    load();
  } catch (e) {
    toast(e.message, "bad");
  }
};

// ── 快照 ──

async function renderRevs() {
  if (!fItem) return;
  try {
    const { revs } = await api(`/api/revs?id=${encodeURIComponent(fItem.id)}`);
    $("#f-revs").textContent = revs.length
      ? `已留 ${revs.length} 版快照（最多 20 版）。`
      : "每次保存自动留一版，最多 20 版。";
  } catch {
    /* 无所谓 */
  }
}

$("#f-revlist").onclick = async () => {
  if (!fItem) return;
  try {
    const { revs } = await api(`/api/revs?id=${encodeURIComponent(fItem.id)}`);
    if (!revs.length) return toast("还没有快照", "");
    const pick = prompt(
      revs.map((r, i) => `${i + 1}. ${r.rev}（${r.text.split("\n").length} 行）`).join("\n") +
        "\n\n输入编号看那一版（只读，关掉即走）：",
    );
    const n = Number(pick);
    if (!n || n < 1 || n > revs.length) return;
    $("#viewer-title").textContent = `lib/${fItem.id}/ 快照 ${revs[n - 1].rev}`;
    $("#viewer-code").textContent = revs[n - 1].text;
    $("#viewer").showModal();
  } catch (e) {
    toast(e.message, "bad");
  }
};

// ── 第二栏：Nous 自研扩展（问进程，不扫盘）──────────────────────────────────
//
// 上面那一栏的真相在**安装位**（pi 只认盘上的文件）。这一栏没有安装位 —— 扩展是
// 进程里的对象，`registry.install()` 装进去就生效，不落盘、不靠 /reload。
// 所以只能问那个正在跑的块：块之间只走 HTTP，地址只从宿主的 ctx 拿，一个 /p/… 都不拼。

const li = (className, text) => {
  const el = document.createElement("li");
  el.className = className;
  if (text) el.textContent = text;
  return el;
};

/** 一小行「谁在用」：6 条会话就写 6 条，多了截断 —— 页面不是数据库。 */
const usageText = (ids) => {
  if (!ids?.length) return "没人选";
  const shown = ids.slice(0, 8).join(", ");
  return `${ids.length} 条会话在用（${shown}${ids.length > 8 ? ", …" : ""}）`;
};

function nousRow(ext, usage, defaults) {
  const el = li("nx");
  const head = document.createElement("div");
  head.className = "nx-head";

  const name = document.createElement("span");
  name.className = "nx-name";
  name.textContent = ext.name;
  head.append(name);

  const ver = document.createElement("span");
  ver.className = "nx-ver";
  ver.textContent = "v" + ext.version;
  head.append(ver);

  if (defaults?.includes(ext.name)) {
    const tag = document.createElement("span");
    tag.className = "nx-tag";
    tag.textContent = "新会话默认选";
    head.append(tag);
  }
  el.append(head);

  const meta = document.createElement("div");
  meta.className = "nx-meta";
  meta.textContent = [
    ext.sections?.length ? `提示词段：${ext.sections.join("、")}` : "不塞提示词",
    Object.keys(ext.needs ?? {}).length ? `要喂：${Object.keys(ext.needs).join("、")}` : null,
    usageText(usage?.[ext.name]),
  ].filter(Boolean).join(" · ");
  el.append(meta);

  if (ext.summary) {
    const sum = document.createElement("div");
    sum.className = "nx-sum";
    sum.textContent = ext.summary;
    el.append(sum);
  }
  return el;
}

async function loadNous() {
  const list = $("#nous-list");
  const src = $("#nous-src");
  list.textContent = "";
  try {
    const ctx = await fetch(SELF + "__nous/ctx").then((r) => r.json());
    const tag =
      ctx.blocks?.find((b) => b.id === "qrd3wh") ??
      ctx.blocks?.find((b) => b.kind === "group" && b.name === "session");
    if (!tag) {
      src.textContent = "这一盘里没有 session 块";
      list.append(li("nx dim small", "没有可问的对象 —— 扩展装在 qrd3wh 那个进程里，它不在就没得问。"));
      return;
    }

    const base = tag.url.endsWith("/") ? tag.url : tag.url + "/";
    const data = await fetch(base + "v1/extensions?usage=1").then(async (r) => {
      const body = await r.json().catch(() => null);
      if (!r.ok) throw new Error(body?.error ?? `HTTP ${r.status}`);
      return body;
    });

    // 版本那一句：钉的和仓库 HEAD 不一致就红 —— 这是「仓库改了但没重启」的唯一可见信号。
    const r = data.repo ?? {};
    const short = (sha) => (sha ? sha.slice(0, 7) : "?");
    const drift = r.drift ? " · ⚠ 仓库 HEAD 跟 lock 不一致（改完没重启）" : "";
    src.textContent = `${r.path} · lock ${short(r.lock)} · HEAD ${short(r.head)}${drift}`;
    src.dataset.drift = r.drift ? "1" : "0";

    for (const ext of data.catalog ?? []) {
      // 菜谱上有、厨房里没有 —— 这才是这一栏存在的理由：扫盘永远看不出这种漂移。
      const row = nousRow(ext, data.usage, data.defaults);
      if (!data.installed?.includes(ext.name)) row.classList.add("nx-missing");
      list.append(row);
    }
    if (!data.catalog?.length) list.append(li("nx dim small", "仓库的 index.json 里一条都没有。"));

    // 自带的单独一行：不是我们写的，不该跟自研的混一个列表。
    if (data.builtin?.length) {
      list.append(li("nx dim small", `pi-durable 自带的（不在仓库里，也不归这页管）：${data.builtin.join("、")}`));
    }
    // 名牌同理：那是 skills 运行时按技能生成的家族，不是一个独立扩展。
    if (data.nameplates?.count) {
      const { names } = data.nameplates;
      const shown = names.slice(0, 8).join(", ");
      list.append(li("nx dim small", `技能名牌 ${names.length} 个（${shown}${names.length > 8 ? ", …" : ""}）—— 由 skills 扩展运行时生成。`));
    }
  } catch (error) {
    src.textContent = "没问到";
    list.append(li("nx dim small", String(error?.message ?? error)));
  }
}

loadNous();
