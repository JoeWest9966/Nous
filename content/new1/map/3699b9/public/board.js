// board.js —— 编辑抽屉里的那块盘面（版式照 hb7k3j，数据改成编 preset.json）。
//
// 三个域 + 中央椭圆，各开各的门：
//   模型   铁路图（供应商 › 模型 › 思考档，跟 hb7k3j 同一张图）
//   上下文 两个框：技能（集合）、模板（单值）—— 后者正文整段每轮都进，所以是单选
//   扩展   一族一行，变体缩进；族内互斥，族间独立
//   权限   这一页只有一个真东西：`readOnly`
// 中央是落子的地方：启动 / 保存。发布在抽屉头部（那是「提交」不是「落子」）。
//
// 这一页比 hb7k3j 多一样东西：**档位**。hb7k3j 是新建一条会话，勾了什么就是什么；
// preset 是反复复用的配置，每个多重字段还有第三态「跟随默认」（= 这个键根本不写）。
// 「勾满现在的全部」和「跟随默认」长得一样、后果相反：后者以后多了技能会自动进来。
// 所以档位单独一排 chip 摆在弹窗顶上 —— 盘面上只显示结果。

const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
const UNCATEGORIZED = "\u0000未分类";
/** 内置类别：这一页的右栏只放**人自己建的**类别（跟扩展那边一个判据）。 */
const DEFAULT_CATEGORY = "Basics";
const LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const PH = 30, R = 10, PITCH = 40, COLGAP = 40;

const pillW = (t) => {
  let w = 0;
  for (const ch of String(t)) w += ch.charCodeAt(0) > 0x2e7f ? 13 : 7.8;
  return Math.max(34, Math.ceil(w) + 22);
};
const catsOf = (o) => (Array.isArray(o?.category) ? o.category : []).map((c) => String(c ?? "").trim()).filter(Boolean);
const underCategory = (label, wanted) => {
  const got = String(label ?? "").trim().toLowerCase();
  const want = String(wanted ?? "").trim().toLowerCase();
  return !!got && !!want && (got === want || got.startsWith(`${want}/`));
};
const inCategory = (o, wanted) => catsOf(o).some((label) => underCategory(label, wanted));
const categoryLabel = (label) => (label === UNCATEGORIZED ? "未分类" : String(label).replace(/\//g, " › "));
const leafOf = (label) => String(label).split("/").filter(Boolean).pop() ?? String(label);
const familyOf = (name) => {
  const at = String(name).indexOf("@");
  return at === -1 ? String(name) : String(name).slice(0, at);
};
/** 词表 ∪ 身上挂着的类别，补上缺的祖先，再按需剔掉内置类别。 */
const collectCats = (vocab, items, { dropBasics }) => {
  const names = new Set((Array.isArray(vocab) ? vocab : []).map((c) => String(c ?? "").trim()).filter(Boolean));
  for (const item of items) for (const c of catsOf(item)) names.add(c);
  for (const label of [...names]) {
    const parts = label.split("/").filter(Boolean);
    for (let i = 1; i < parts.length; i += 1) names.add(parts.slice(0, i).join("/"));
  }
  return [...names]
    .filter((label) => (dropBasics ? !underCategory(label, DEFAULT_CATEGORY) : true))
    .sort((a, b) => a.localeCompare(b, "zh-CN"));
};

export function createBoard(deps) {
  const { api, root, onChange, onSave, onPublish, getName, onState } = deps;
  const $ = (id) => document.getElementById(id);
  const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)").matches;

  /** 草稿永远是外面那一份（编辑抽屉的 `editor.draft`）—— 这里不存副本，两张皮必分叉。 */
  const draft = deps.getDraft;
  const models = deps.getModels;
  const deviceModel = deps.getDeviceModel;

  // ── 名册（一次编辑只拉一次）──────────────────────────────────────────────
  let loaded = false;
  let skills = [];          // `/v1/skills` 的 skills[]（带 category / load）
  let templates = [];       // `/v1/prompts` 的 prompts[]
  let families = [];        // `/v1/extensions`：本体 + 变体合成一族
  let cats = { skill: [], tmpl: [], ext: [] };
  let skillsPage = null;
  let promptsPage = null;
  const bodies = new Map(); // 预览过的正文，读过贴住

  // ── 视图状态：只影响「看见什么」，一个字节都不动选区 ──────────────────────
  const view = {
    skill: { v: "all", cat: null, only: false },
    tmpl: { v: "all", cat: null, only: false },
    ext: { v: "all", cat: null, only: false },
  };
  let pickProvider = null;
  let kb = { on: false, col: 0, row: 0 };
  let body = "";            // drawRule 往里追加 <g>

  const cur = () => draft() ?? {};

  // 目录浏览器状态（函数在下面，状态在这儿 —— load() 要写 dirStart）
  let dirSeq = 0;        // 只认最后一次请求，切目录竞态时旧回复别乱涂屏
  let dirStart = null;   // 服务端 startDirs[0]（浏览器首页）
  let dirCur = "";       // 当前浏览到的目录（规范绝对路径）

  // ── 名册装载 ─────────────────────────────────────────────────────────────
  async function load() {
    if (loaded) return;
    loaded = true;
    try {
      const [sk, pr, ex, ctx, dev] = await Promise.all([
        api.get("v1/skills").catch(() => null),
        api.get("v1/prompts").catch(() => null),
        api.get("v1/extensions").catch(() => null),
        api.get("__nous/ctx").catch(() => null),
        api.get("v1/device").catch(() => null),
      ]);
      dirStart = dev?.startDirs?.[0] ?? null;
      if (sk) {
        skills = (sk.skills ?? []).filter((s) => s.load?.state === "ok" && !s.hidden);
        // 技能那边**不剔 Basics**：扩展剔是因为它有「默认勾 Basics」这条规则，技能没有。
        cats.skill = collectCats(sk.categories, skills, { dropBasics: false });
      }
      if (pr) {
        templates = (pr.prompts ?? []).filter((p) => p.load?.state === "ok");
        cats.tmpl = collectCats(pr.categories, templates, { dropBasics: true });
      }
      if (ex) {
        const installed = ex.installed ?? [];
        families = (ex.catalog ?? [])
          .filter((e) => installed.includes(e.name) && !String(e.name).startsWith("skill:"))
          .map((e) => ({ name: e.name, ext: e, variants: (ex.variants?.[e.name] ?? []).map((v) => ({ ...v })) }))
          .sort((a, b) => a.name.localeCompare(b.name));
        cats.ext = collectCats(ex.categories, families.map((f) => f.ext), { dropBasics: true });
      }
      const blocks = ctx?.blocks ?? [];
      skillsPage = blocks.find((b) => b.id === "68kazk") ?? null;
      promptsPage = blocks.find((b) => b.id === "6ysg01") ?? null;
      const skillGo = $("b-skill-rail-go");
      const tmplGo = $("b-tmpl-rail-go");
      const extGo = $("b-ext-rail-go");
      if (skillsPage && skillGo) skillGo.href = skillsPage.url;
      if (promptsPage && tmplGo) tmplGo.href = promptsPage.url;
      if (promptsPage && extGo) extGo.href = promptsPage.url;
    } catch (error) {
      onState?.(`名册拿不到：${String(error?.message ?? error)}`);
    }
    render();
  }

  // ── 盘面 ─────────────────────────────────────────────────────────────────
  const levelsOf = () => {
    const m = cur().model;
    const hit = m && models().find((x) => x.provider === m.provider && x.modelId === m.modelId);
    return hit?.thinkingLevels ?? ["off"];
  };
  const clampLevel = (level, list) => {
    if (!level) return "";
    if (list.includes(level)) return level;
    const at = LEVELS.indexOf(level);
    if (at === -1) return list[0] ?? "off";
    for (let i = at; i < LEVELS.length; i += 1) if (list.includes(LEVELS[i])) return LEVELS[i];
    for (let i = at - 1; i >= 0; i -= 1) if (list.includes(LEVELS[i])) return LEVELS[i];
    return list[0] ?? "off";
  };

  /** 盘面节点/副标题的文案，全部照 hb7k3j 的格式（英文、同一套说法）。 */
  const skillsName = () => {
    const d = cur();
    if (d.skillsMode === "default") return "default";
    if (d.skillsMode === "none") return "none";
    return `${(d.skills ?? []).length}/${skills.length}`;
  };
  const tmplName = () => {
    const d = cur();
    if (d.templateMode === "default") return "default";
    if (d.templateMode === "none") return "none";
    return d.template || "—";
  };
  const extName = () => {
    const d = cur();
    if (d.extMode === "default") return "default";
    if (d.extMode === "none") return "none";
    const picked = d.extensions ?? [];
    if (!picked.length) return "none";
    const variantPicked = picked.filter((n) => n.includes("@"));
    const bare = picked.filter((n) => !n.includes("@"));
    if (!variantPicked.length && bare.length === families.length) return `all ${families.length}`;
    if (picked.length === 1) return picked[0];
    return `${picked[0]} +${picked.length - 1}`;
  };

  function render() {
    const d = cur();
    const name = getName?.() ?? "";
    // name 是机器键（小写、只留 . _ -）；显示时把下划线读成空格：session_template → Session Template。
    $("b-name").textContent = name ? name.replace(/_/g, " ") : "—";
    $("b-name").title = name;

    // 模型（照 hb7k3j：节点 = modelId + provider，副标题 = N available · thinking X）
    const m = d.model;
    const prov = m ? (models().find((x) => x.provider === m.provider && x.modelId === m.modelId)?.provider ?? m.provider) : "";
    $("b-model-id").textContent = m ? m.modelId : (models().length ? "device default" : "—");
    $("b-model-tag").textContent = m ? prov : (models().length ? "change ›" : "no model");
    $("b-model").disabled = !models().length;
    const lv = levelsOf();
    $("b-model-sub").textContent = m
      ? `${models().length} available${lv.length > 1 ? ` · thinking ${d.thinkingLevel || "off"}` : ""}`
      : `${models().length} available · device default`;

    // 上下文（照 hb7k3j：skills N/M · prompt X）
    $("b-skills-tag").textContent = `${skillsName()} ›`;
    $("b-tmpl-tag").textContent = `${tmplName()} ›`;
    $("b-ctx-sub").textContent = `skills ${skillsName()} · prompt ${tmplName()}`;

    // 扩展（照 hb7k3j：节点 = 逻辑名 + change ›，副标题 = installed N · …）
    $("b-ext-id").textContent = families.length ? extName() : "none available";
    $("b-ext-tag").textContent = "change ›";
    $("b-ext").disabled = !families.length;
    const extN = d.extMode === "picked" ? (d.extensions ?? []).length : d.extMode;
    $("b-ext-sub").textContent = families.length
      ? `installed ${families.length} · ${extN === "default" || extN === "none" ? extN : `picked ${extN}`}`
      : "no extensions";

    // 权限：preset 唯一比 hb7k3j 多的真字段。外观照它，副标题留空（那里本来就是空域）。
    $("b-ro-tag").textContent = d.readOnly ? "on ›" : "off ›";
    $("b-readonly").setAttribute("aria-pressed", String(!!d.readOnly));

    // 工作目录：三态（跟随默认 / 无 / 钉路径）
    $("b-dir-id").textContent = d.cwdMode === "picked" ? shortDir(d.cwd) : d.cwdMode === "none" ? "none" : "default";
    $("b-dir-tag").textContent = d.cwdMode === "picked" ? "change ›" : "browse ›";

    // 机器：三态（跟随默认 / 本机 / 钉机器）
    $("b-machine-id").textContent = d.machineMode === "picked" ? d.machine : d.machineMode === "none" ? "本机" : "default";
    $("b-machine-tag").textContent = d.machineMode === "picked" ? "change ›" : "browse ›";

    // 中央
    const dirty = deps.isDirty?.() ?? false;
    $("b-hub-sub").textContent = dirty ? "unsaved · save first" : "saved · publish to commit";
    $("b-save").disabled = !name;
    $("b-publish").disabled = !name;
  }

  // ── 弹窗开合 ─────────────────────────────────────────────────────────────
  const openSheet = (scrimId, from, focusIn) => {
    const scrim = $(scrimId);
    if (from && !reduceMotion) {
      const r = from.getBoundingClientRect();
      scrim.querySelector(".bsheet").style.transformOrigin = `${r.left + r.width / 2}px ${r.top + r.height / 2}px`;
    }
    scrim.hidden = false;
    requestAnimationFrame(() => scrim.classList.add("on"));
    focusIn();
  };
  const closeSheet = (scrimId) => {
    const scrim = $(scrimId);
    scrim.classList.remove("on");
    setTimeout(() => { scrim.hidden = true; }, reduceMotion ? 160 : 260);
  };
  const openScrims = () => ["b-machine-scrim", "b-dir-scrim", "b-ext-scrim", "b-tmpl-scrim", "b-skill-scrim", "b-model-scrim"].filter((id) => !$(id).hidden);

  const openModel = (from) => {
    $("b-mq").value = "";
    kb = { on: false, col: 0, row: 0 };
    openSheet("b-model-scrim", from, () => { drawModel(); $("b-mq").focus(); });
  };
  const openSkills = (from) => {
    $("b-sq").value = "";
    view.skill = { v: "all", cat: null, only: false };
    openSheet("b-skill-scrim", from, () => {
      drawSkills();
      ($("b-skill-list").querySelector(".brow") ?? $("b-skill-done")).focus();
    });
  };
  const openTmpl = (from) => {
    $("b-tq").value = "";
    view.tmpl = { v: "all", cat: null, only: false };
    openSheet("b-tmpl-scrim", from, () => {
      drawTmpl();
      ($("b-tmpl-list").querySelector(".brow") ?? $("b-tmpl-done")).focus();
    });
  };
  const openExt = (from) => {
    $("b-eq").value = "";
    view.ext = { v: "all", cat: null, only: false };
    openSheet("b-ext-scrim", from, () => {
      drawExt();
      ($("b-ext-list").querySelector(".brow") ?? $("b-ext-done")).focus();
    });
  };

  // ── 工作目录：跟 hb7k3j 同一套目录浏览器，多了「跟随默认」这一态 ──
  const shortDir = (p) => {
    if (!p) return "—";
    if (p === "/") return "/";
    return p.replace(/\/+$/, "").split("/").pop() || "/";
  };

  const browseDir = async (p) => {
    const mine = ++dirSeq;
    $("b-dir-hint").textContent = "…";
    try {
      const d = cur();
      // 钉了机器就列那台的目录，否则列本机 —— 跟 hb7k3j 一条纪律。
      const machine = d.machineMode === "picked" && d.machine ? `&machine=${encodeURIComponent(d.machine)}` : "";
      const listing = await api.get(`v1/fs/dirs?path=${encodeURIComponent(p ?? "")}${machine}`);
      if (mine !== dirSeq) return;
      dirCur = listing.path || "";
      $("b-dq").value = dirCur;
      renderDirRows(listing);
      $("b-dir-hint").textContent = "—";
    } catch (error) {
      if (mine !== dirSeq) return;
      $("b-dir-hint").textContent = String(error?.message ?? error);
    }
  };

  const renderDirRows = (listing) => {
    const list = $("b-dir-list");
    list.textContent = "";
    const add = (ico, name, path) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "bdir";
      b.setAttribute("aria-selected", String(path === dirCur && path !== ""));
      b.title = path;
      const i = document.createElement("span");
      i.className = "bdir-ico";
      i.textContent = ico;
      const n = document.createElement("span");
      n.className = "bdir-name";
      n.textContent = name;
      b.append(i, n);
      b.onclick = () => browseDir(path);
      list.append(b);
    };
    const d = cur();
    const remote = d.machineMode === "picked" && d.machine;
    add("⌂", remote ? "Home" : "Start dirs", "");
    if (listing.parent) add("↑", "..", listing.parent);
    if (!(listing.dirs?.length)) {
      const hint = document.createElement("div");
      hint.className = "mnone";
      hint.textContent = "No subdirectories.";
      list.append(hint);
      return;
    }
    for (const d of listing.dirs) add("▸", d.name, d.path);
  };

  const renderDirModes = () => {
    const mode = cur().cwdMode ?? "default";
    for (const b of $("b-dir-modes").querySelectorAll(".bmode")) {
      b.setAttribute("aria-pressed", String(b.dataset.mode === mode));
    }
  };

  const openDir = (from) => {
    renderDirModes();
    openSheet("b-dir-scrim", from, () => {
      const d = cur();
      const remote = d.machineMode === "picked" && d.machine;
      // 钉了机器：回到上次选中的远端路径；否则从远端 home（空 path）开始。
      dirCur = d.cwdMode === "picked" && d.cwd ? d.cwd : (remote ? "" : (dirStart ?? ""));
      $("b-dq").value = dirCur;
      browseDir(dirCur);
      $("b-dq").focus();
    });
  };
  const closeDir = () => closeSheet("b-dir-scrim");

  $("b-dgo").onclick = () => browseDir($("b-dq").value.trim());
  $("b-dq").addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); browseDir($("b-dq").value.trim()); }
  });
  $("b-dir-done").onclick = async () => {
    const v = $("b-dq").value.trim();
    if (v && v !== dirCur) await browseDir(v);
    const d = cur();
    if (d.cwdMode === "picked") d.cwd = dirCur || v || dirStart || "";
    onChange();
    render();
    closeDir();
  };
  $("b-dir-modes").addEventListener("click", (e) => {
    const chip = e.target.closest?.(".bmode");
    if (!chip) return;
    const d = cur();
    d.cwdMode = chip.dataset.mode;
    if (chip.dataset.mode !== "picked") d.cwd = "";
    else if (!d.cwd && dirStart) d.cwd = dirStart;
    onChange();
    render();
    renderDirModes();
  });

  // ── 机器：三态（跟随默认 / 本机 / 钉机器）。清单只在「钉机器」时有用 ──
  let machineList = [];
  let machineSeq = 0;

  const fetchMachines = async () => {
    const mine = ++machineSeq;
    $("b-machine-hint").textContent = "…";
    try {
      const data = await api.get("v1/machines");
      if (mine !== machineSeq) return;
      machineList = data.machines ?? [];
      renderMachineRows();
      $("b-machine-hint").textContent = data.error ? String(data.error) : "—";
    } catch (error) {
      if (mine !== machineSeq) return;
      $("b-machine-hint").textContent = String(error?.message ?? error);
    }
  };

  const renderMachineRows = () => {
    const list = $("b-machine-list");
    list.textContent = "";
    const d = cur();
    const add = (name, sub, picked) => {
      const b = document.createElement("button");
      b.type = "button";
      b.className = "bdir";
      b.setAttribute("aria-selected", String(picked));
      b.title = sub;
      const i = document.createElement("span");
      i.className = "bdir-ico";
      i.textContent = picked ? "✓" : "▸";
      const n = document.createElement("span");
      n.className = "bdir-name";
      n.textContent = name;
      const s = document.createElement("span");
      s.className = "bdir-name";
      s.style.color = "var(--muted)";
      s.style.fontSize = "clamp(9.5px,1.25vmin,11px)";
      s.textContent = sub;
      const wrap = document.createElement("span");
      wrap.style.display = "flex";
      wrap.style.flexDirection = "column";
      wrap.style.gap = ".15em";
      wrap.append(n, s);
      b.append(i, wrap);
      b.onclick = () => {
        const curDraft = cur();
        curDraft.machineMode = "picked";
        curDraft.machine = name;
        // 换了机器，路径跟着作废 —— 一台机器上的路径在另一台上没意义。
        curDraft.cwdMode = "default";
        curDraft.cwd = "";
        onChange();
        render();
        renderMachineRows();
        renderMachineModes();
      };
      list.append(b);
    };
    for (const m of machineList) {
      add(m.name, `${m.user}@${m.addr}:${m.port}`, d.machineMode === "picked" && d.machine === m.name);
    }
    if (!machineList.length) {
      const hint = document.createElement("div");
      hint.className = "mnone";
      hint.textContent = "No machines in catalog.";
      list.append(hint);
    }
  };

  const renderMachineModes = () => {
    const mode = cur().machineMode ?? "default";
    for (const b of $("b-machine-modes").querySelectorAll(".bmode")) {
      b.setAttribute("aria-pressed", String(b.dataset.mode === mode));
    }
  };

  const openMachine = (from) => {
    renderMachineModes();
    renderMachineRows();
    openSheet("b-machine-scrim", from, () => {
      fetchMachines();
    });
  };
  const closeMachine = () => closeSheet("b-machine-scrim");

  $("b-machine-done").onclick = () => {
    onChange();
    render();
    closeMachine();
  };
  $("b-machine-modes").addEventListener("click", (e) => {
    const chip = e.target.closest?.(".bmode");
    if (!chip) return;
    const d = cur();
    const prevMode = d.machineMode;
    d.machineMode = chip.dataset.mode;
    if (chip.dataset.mode !== "picked") d.machine = "";
    // 机器语境变了，路径跟着作废（除非只是原地重选同一档）。
    if (d.machineMode !== prevMode) { d.cwdMode = "default"; d.cwd = ""; }
    onChange();
    render();
    renderMachineModes();
    renderMachineRows();
  });

  // ── 模型语法图（跟 hb7k3j 同一张铁路图，只是多了「缺省」那一档）──────────
  function drawRule(name, items, cx0, cx1, top) {
    const ty = top + 24;
    const picked = cur().model;
    const thinking = cur().thinkingLevel ?? "";
    const def = deviceModel();
    const ws = items.map((it) => {
      const sel = it.isLevel
        ? thinking === it.value
        : it.isProvider
          ? picked?.provider === it.m.provider && !$("b-mq").value.trim()
          : !!picked && picked.provider === it.m.provider && picked.modelId === it.m.modelId;
      const isDef = !it.isProvider && !it.isLevel && !!def && def.provider === it.m.provider && def.modelId === it.m.modelId;
      return { ...it, sel, isDef, w: pillW(it.label ?? it.m.modelId) + (sel ? 18 : 0) + (isDef ? 32 : 0) };
    });
    const rows = ws.map((p, i) => ({ ...p, x: cx0 + 16, y: ty + i * PITCH }));
    const n = rows.length;
    let d = `M${cx0 - 16},${ty}H${cx0}M${cx1},${ty}H${cx1 + 16}`;
    if (n === 1) {
      d += `M${cx0},${ty}H${rows[0].x}M${rows[0].x + rows[0].w},${ty}H${cx1}`;
    } else {
      const first = rows[0], last = rows[n - 1];
      const xR = cx1 - 2 * R;
      d += `M${cx0},${ty}H${first.x}M${first.x + first.w},${ty}H${cx1}`;
      d += `M${cx0},${ty}a${R},${R} 0 0 1 ${R},${R}V${last.y - R}a${R},${R} 0 0 0 ${R},${R}H${last.x}`;
      d += `M${last.x + last.w},${last.y}H${xR - R}a${R},${R} 0 0 0 ${R},${-R}V${ty + R}a${R},${R} 0 0 1 ${R},${-R}`;
      for (const r of rows.slice(1, -1)) {
        d += `M${cx0 + R},${r.y - R}a${R},${R} 0 0 0 ${R},${R}H${r.x}`;
        d += `M${r.x + r.w},${r.y}H${xR - R}a${R},${R} 0 0 0 ${R},${-R}`;
      }
    }
    const pills = rows.map((p) => {
      const spans = [];
      if (p.sel) spans.push('<tspan fill="var(--go)">✓ </tspan>');
      spans.push(esc(p.label ?? p.m.modelId));
      if (p.isDef) spans.push('<tspan fill="var(--warn)" font-size="10"> ·默认</tspan>');
      const cls = `pill${p.sel ? " sel" : ""}${p.isProvider && pickProvider === p.m.provider ? " cur" : ""}`
        + `${p.isProvider ? " pv" : ""}${p.isLevel ? " lv" : ""}${p.dim ? " dim" : ""}`;
      const label = p.isLevel ? `思考档位 ${p.label}`
        : p.isProvider ? `${p.m.provider}（切换）`
        : `${p.m.provider} / ${p.m.modelId}`;
      const val = p.isLevel ? esc(p.value) : "";
      return `<g class="${cls}" data-p="${esc(p.m.provider)}" data-m="${esc(val || p.m.modelId)}" role="button" tabindex="0" aria-label="${esc(label)}"><rect x="${p.x}" y="${p.y}" width="${p.w}" height="${PH}" rx="${PH / 2}"/><text x="${p.x + p.w / 2}" y="${p.y + PH / 2 + 4.5}" text-anchor="middle" style="font-family:var(--mono)" font-size="13">${spans.join("")}</text></g>`;
    }).join("");
    body += `<g class="rule"><text x="${cx0}" y="${top + 4}" font-size="14" font-weight="700" text-anchor="start">${esc(name)}</text><path d="${d}"/><circle cx="${cx0 - 16}" cy="${ty}" r="4"/><circle cx="${cx1 + 16}" cy="${ty}" r="4"/>${pills}</g>`;
    return n;
  }

  function drawModel() {
    const box = $("b-diagram");
    const list = models();
    const picked = cur().model;
    if (!list.length) { box.innerHTML = '<div class="mnone">这台机器还没有能用的模型。</div>'; return; }
    const q = $("b-mq").value.trim().toLowerCase();
    const hit = (m) => !q || `${m.provider} ${m.modelId} ${m.name ?? ""}`.toLowerCase().includes(q);

    const byProvider = new Map();
    for (const m of list) {
      if (!byProvider.has(m.provider)) byProvider.set(m.provider, []);
      byProvider.get(m.provider).push(m);
    }
    const provs = [...byProvider.keys()];
    if (!provs.includes(pickProvider)) pickProvider = picked?.provider ?? provs[0];

    let left, right, rightName;
    if (q) {
      left = provs.map((p) => ({ m: { modelId: p, provider: p }, isProvider: true, dim: true }));
      right = list.filter(hit).map((m) => ({ m }));
      rightName = "匹配";
      if (!right.length) { box.innerHTML = '<div class="mnone">没有匹配的模型。</div>'; return; }
    } else {
      left = provs.map((p) => ({ m: { modelId: p, provider: p }, isProvider: true }));
      right = (byProvider.get(pickProvider) ?? []).map((m) => ({ m }));
      rightName = `${pickProvider} · 模型`;
    }

    const levels = levelsOf();
    const cols = [{ name: "供应商", items: left }, { name: rightName, items: right }];
    // 「缺省」也占一格：preset 可以**不写**档位，那不叫 off，叫「引擎自己定」。
    if (levels.length > 1) {
      cols.push({
        name: "思考",
        items: [{ label: "缺省", value: "", isLevel: true, m: { modelId: "", provider: "" } },
          ...levels.map((l) => ({ label: l, value: l, isLevel: true, m: { modelId: l, provider: "" } }))],
      });
    }

    const W = Math.min(720, Math.max(340, box.clientWidth || 520));
    const x0 = 28, xE = W - 28, avail = xE - x0;
    const labelOf = (it) => it.label ?? it.m.modelId;
    const widthOf = (c) => Math.max(...c.items.map((it) => pillW(labelOf(it)))) + 16;

    body = "";
    let height;
    const widest = (a) => a.reduce((s, c) => s + widthOf(c), 0);

    if (widest(cols) + COLGAP * (cols.length - 1) <= avail) {
      let cx = x0, rows = 1;
      for (const c of cols) {
        const w = widthOf(c);
        rows = Math.max(rows, drawRule(c.name, c.items, cx, cx + w, 22));
        cx += w + COLGAP;
      }
      height = 22 + 24 + (rows - 1) * PITCH + PH + 16;
    } else {
      let top = 22;
      const pairs = [];
      let cx = x0, used = 0, pair = [];
      for (const c of cols) {
        const w = widthOf(c);
        const add = pair.length ? COLGAP + w : w;
        if (pair.length && used + add > avail) { pairs.push(pair); pair = [c]; used = w; }
        else { pair.push(c); used += add; }
      }
      if (pair.length) pairs.push(pair);
      for (const row of pairs) {
        let rows = 1;
        cx = x0;
        for (const c of row) {
          const w = row.length === 1 ? avail : widthOf(c);
          rows = Math.max(rows, drawRule(c.name, c.items, cx, cx + w, top));
          cx += w + COLGAP;
        }
        top += 24 + (rows - 1) * PITCH + PH + 30;
      }
      height = top - 30 + 16;
    }
    box.innerHTML = `<svg viewBox="0 0 ${W} ${Math.round(height)}" role="img" aria-roledescription="railroad" aria-label="模型语法图">${body}</svg>`;
    const sel = box.querySelector(".pill.sel");
    if (sel && !kb.on) sel.scrollIntoView({ block: "nearest", behavior: reduceMotion ? "auto" : "smooth" });
    paintKb();
  }

  const kbCols = () => [...$("b-diagram").querySelectorAll("g.rule")].map((r) => [...r.querySelectorAll("g.pill")]);
  const kbHome = (cols) => {
    for (let c = 0; c < cols.length; c += 1) {
      const r = cols[c].findIndex((g) => g.classList.contains("sel"));
      if (r !== -1) return { col: c, row: r };
    }
    return { col: Math.min(1, cols.length - 1), row: 0 };
  };
  const kbOff = () => {
    kb.on = false;
    $("b-diagram").querySelectorAll(".pill.kb").forEach((g) => g.classList.remove("kb"));
  };
  const paintKb = () => {
    const cols = kbCols();
    if (!kb.on || !cols.length) return;
    kb.col = Math.max(0, Math.min(kb.col, cols.length - 1));
    const col = cols[kb.col];
    if (!col.length) return;
    kb.row = Math.max(0, Math.min(kb.row, col.length - 1));
    $("b-diagram").querySelectorAll(".pill.kb").forEach((g) => g.classList.remove("kb"));
    const g = col[kb.row];
    g.classList.add("kb");
    g.focus?.({ preventScroll: true });
    g.scrollIntoView({ block: "nearest", behavior: reduceMotion ? "auto" : "smooth" });
  };

  /** 药丸的三种走法，键盘和鼠标用同一份判断。 */
  const actOn = (g) => {
    const d = cur();
    if (g.classList.contains("lv")) {
      d.thinkingLevel = g.dataset.m;
      onChange();
      render();
      drawModel();
      return;
    }
    if (g.classList.contains("pv")) { pickProvider = g.dataset.p; drawModel(); return; }
    d.model = { provider: g.dataset.p, modelId: g.dataset.m };
    d.thinkingLevel = clampLevel(d.thinkingLevel, levelsOf());
    onChange();
    render();
    drawModel();
    setTimeout(() => closeSheet("b-model-scrim"), reduceMotion ? 0 : 180);
  };

  // ── 三档配置：跟随默认 / 都不要 / 挑几个 ────────────────────────────────
  const modeOf = (kind) =>
    kind === "skill" ? cur().skillsMode : kind === "tmpl" ? cur().templateMode : cur().extMode;
  const setMode = (kind, mode) => {
    const d = cur();
    if (kind === "skill") {
      d.skillsMode = mode;
      if (mode === "none") d.skills = [];
    } else if (kind === "tmpl") {
      d.templateMode = mode;
      if (mode === "none") d.template = "";
    } else {
      d.extMode = mode;
      if (mode === "none") d.extensions = [];
    }
    onChange();
  };
  const paintModes = (kind) => {
    const box = $(`b-${kind === "skill" ? "skill" : kind}-modes`);
    if (!box) return;
    const mode = modeOf(kind);
    for (const btn of box.querySelectorAll(".bmode")) {
      btn.setAttribute("aria-pressed", String(btn.dataset.mode === mode));
    }
  };

  // ── 技能清单 ─────────────────────────────────────────────────────────────
  const skillPrimary = (s) => catsOf(s)[0] ?? UNCATEGORIZED;

  function drawSkillViews() {
    const box = $("b-skill-views");
    const v = view.skill;
    const uncategorized = skills.filter((s) => !catsOf(s).length).length;
    const picked = (cur().skills ?? []).filter((n) => skills.some((s) => s.name === n)).length;
    const paint = (key, pressed, html) => {
      const btn = box.querySelector(`[data-view="${key}"]`);
      if (!btn) return;
      btn.setAttribute("aria-pressed", String(pressed));
      btn.innerHTML = html;
    };
    paint("all", !v.cat && v.v === "all", `全部<span class="n">${skills.length}</span>`);
    paint("none", !v.cat && v.v === "none", `未分类<span class="n">${uncategorized}</span>`);
    const only = box.querySelector("[data-only]");
    only.setAttribute("aria-pressed", String(v.only));
    only.innerHTML = `只看已选<span class="n">${picked}</span>`;
    if (picked) { only.removeAttribute("aria-disabled"); only.tabIndex = 0; }
    else { only.setAttribute("aria-disabled", "true"); only.tabIndex = -1; }
  }

  function drawSkillRail() {
    const rail = $("b-skill-rail");
    const has = cats.skill.length > 0;
    $("b-skill-scrim").querySelector(".bsheet").classList.toggle("has-rail", has);
    $("b-skill-body").classList.toggle("has-rail", has);
    rail.hidden = !has;
    if (!has) { $("b-skill-rail-list").innerHTML = ""; return; }
    const picked = new Set(cur().skills ?? []);
    $("b-skill-rail-list").innerHTML = cats.skill.map((label) => {
      const rows = skills.filter((s) => catsOf(s).some((c) => underCategory(c, label)));
      const depth = label.split("/").filter(Boolean).length - 1;
      return `<button class="bcat${depth ? " child" : ""}${rows.length ? "" : " zero"}" type="button"`
        + ` aria-pressed="${view.skill.cat === label}" data-cat="${esc(label)}" title="${esc(categoryLabel(label))}">`
        + (rows.some((s) => picked.has(s.name)) ? '<span class="dot"></span>' : "")
        + `<span class="cl">${esc(leafOf(label))}</span><span class="n">${rows.length}</span></button>`;
    }).join("");
  }

  function drawSkills() {
    const v = view.skill;
    const needle = $("b-sq").value.trim().toLowerCase();
    const hit = (t) => !needle || String(t).toLowerCase().includes(needle);
    const d = cur();
    const picked = new Set(d.skills ?? []);

    paintModes("skill");
    drawSkillViews();
    drawSkillRail();

    if (!skills.length) {
      $("b-skill-list").innerHTML = '<div class="bnone">这台机器一个技能都没装。'
        + (skillsPage ? `<br><a href="${esc(skillsPage.url)}/" target="_blank" rel="noreferrer">去技能面板装一个 ›</a>` : "")
        + "</div>";
      $("b-skill-hint").textContent = "0 个技能";
      return;
    }

    const inScope = (s) =>
      v.cat ? catsOf(s).some((c) => underCategory(c, v.cat))
        : v.v === "none" ? !catsOf(s).length
          : true;
    const shown = skills
      .filter(inScope)
      .filter((s) => !v.only || picked.has(s.name))
      .filter((s) => hit(`${s.name} ${s.description ?? ""} ${catsOf(s).join(" ")}`));

    // 仓里有、清单里没有的：不能静默消失 —— 那是这条 preset 真在用的名字。
    const ghosts = (d.skills ?? [])
      .filter((n) => !skills.some((s) => s.name === n))
      .filter((n) => hit(n))
      .map((n) => ({ name: n, description: "", ghost: true, category: [] }));

    if (!shown.length && !ghosts.length) {
      const ways = [
        needle ? '<button class="bchip" type="button" data-clear="1">清空搜索 ›</button>' : "",
        v.only ? '<button class="bchip" type="button" data-only="1">取消「只看已选」›</button>' : "",
        v.cat ? '<button class="bchip" type="button" data-cat="">全部类别 ›</button>' : "",
      ].filter(Boolean);
      $("b-skill-list").innerHTML = `<div class="bnone">没有匹配的技能。${ways.length ? `<br>${ways.join(" ")}` : ""}</div>`;
    } else {
      const groups = [];
      if (v.cat || v.v !== "all") groups.push({ label: null, rows: shown });
      else {
        const order = [...new Set(shown.map(skillPrimary))]
          .sort((a, b) => (a === UNCATEGORIZED) - (b === UNCATEGORIZED) || a.localeCompare(b, "zh-CN"));
        for (const label of order) {
          const rows = shown.filter((s) => skillPrimary(s) === label);
          if (rows.length) groups.push({ label, rows });
        }
      }
      const row = (s) => {
        const on = picked.has(s.name);
        const desc = String(s.description ?? "").replace(/\s+/g, " ").trim();
        const group = String(s.path ?? "").includes("/") ? String(s.path).split("/").slice(0, -1).join("/") : "";
        const meta = s.ghost
          ? "仓里有 · 这台机器上没装成"
          : [group ? `${group}/` : "", catsOf(s).join(" · ")].filter(Boolean).join(" · ");
        return `<button class="brow" type="button" role="checkbox" aria-checked="${on}" data-s="${esc(s.name)}">`
          + `<span class="bbox">${on ? "✓" : ""}</span>`
          + `<span><span class="bname">${esc(s.name)}</span>`
          + (desc ? `<span class="bdesc">${esc(desc)}</span>` : "")
          + (meta ? `<span class="bmeta">${esc(meta)}</span>` : "")
          + "</span></button>";
      };
      $("b-skill-list").innerHTML =
        groups.map(({ label, rows }) => (label ? `<div class="bhead">${esc(categoryLabel(label))}</div>` : "") + rows.map(row).join("")).join("")
        + (ghosts.length ? `<div class="bhead">仓里有</div>${ghosts.map(row).join("")}` : "");
    }
    const n = picked.size;
    $("b-skill-hint").textContent = d.skillsMode === "picked"
      ? `技能 ${n}/${skills.length} · 勾完按完成`
      : d.skillsMode === "none" ? "一个都不要 · 点一条就改成「挑几个」" : "跟随默认 · 点一条就改成「挑几个」";
  }

  // ── 模板清单（单选）────────────────────────────────────────────────────
  const tmplPrimary = (p) => catsOf(p)[0] ?? UNCATEGORIZED;
  const NONE_ROW = { name: "", summary: "不往系统提示词里加任何模板", category: [], none: true };

  const tmplRow = (p) => {
    const d = cur();
    const key = p.none ? "" : p.name;
    const on = p.none ? d.templateMode === "none" : d.templateMode === "picked" && d.template === p.name;
    const meta = p.none ? ""
      : p.ghost ? "仓里有 · 私库里没有"
        : [`${p.bytes ?? 0} 字节`, ...(catsOf(p).length ? [catsOf(p).join(" / ")] : [])].join(" · ");
    return `<div class="brow" role="radio" tabindex="0" aria-checked="${on}" data-t="${esc(key)}">`
      + `<span class="bbox"></span>`
      + `<span><span class="bname">${esc(p.none ? "无模板" : p.name)}</span>`
      + `<span class="bdesc">${esc(String(p.summary ?? ""))}</span>`
      + (meta ? `<span class="bmeta">${esc(meta)}</span>` : "")
      + (p.none ? "" : `<button class="bpeek" type="button" data-peek="${esc(p.name)}">预览</button>`
        + `<pre class="bpeek-body" data-body="${esc(p.name)}" hidden></pre>`)
      + "</span></div>";
  };

  async function togglePeek(button) {
    const name = button.dataset.peek;
    const pre = button.parentElement.querySelector(`pre[data-body="${CSS.escape(name)}"]`);
    if (!pre) return;
    if (!pre.hidden) { pre.hidden = true; button.textContent = "预览"; return; }
    pre.hidden = false;
    button.textContent = "收起";
    if (pre.dataset.loaded) return;
    if (bodies.has(name)) { pre.textContent = bodies.get(name); pre.dataset.loaded = "1"; return; }
    pre.textContent = "读…";
    try {
      const file = await api.get(`v1/prompts/file?name=${encodeURIComponent(name)}&path=PROMPT.md`);
      bodies.set(name, file.text);
      pre.textContent = file.text;
      pre.dataset.loaded = "1";
    } catch (error) {
      pre.textContent = `读不到：${String(error?.message ?? error)}`;
    }
  }

  function drawTmplViews() {
    const box = $("b-tmpl-views");
    const v = view.tmpl;
    const bas = templates.filter((p) => inCategory(p, DEFAULT_CATEGORY)).length;
    const none = templates.filter((p) => !catsOf(p).length).length;
    const paint = (key, pressed, html) => {
      const btn = box.querySelector(`[data-view="${key}"]`);
      if (!btn) return;
      btn.setAttribute("aria-pressed", String(pressed));
      btn.innerHTML = html;
    };
    paint("all", !v.cat && v.v === "all", `全部<span class="n">${templates.length}</span>`);
    paint("basics", !v.cat && v.v === "basics", `${esc(DEFAULT_CATEGORY)}<span class="n">${bas}</span>`);
    paint("none", !v.cat && v.v === "none", `未分类<span class="n">${none}</span>`);
    const only = box.querySelector("[data-only]");
    const on = cur().templateMode === "picked" && cur().template ? 1 : 0;
    only.setAttribute("aria-pressed", String(v.only));
    only.innerHTML = `只看已选<span class="n">${on}</span>`;
  }

  function drawTmplRail() {
    const rail = $("b-tmpl-rail");
    const has = cats.tmpl.length > 0;
    $("b-tmpl-scrim").querySelector(".bsheet").classList.toggle("has-rail", has);
    $("b-tmpl-body").classList.toggle("has-rail", has);
    rail.hidden = !has;
    if (!has) { $("b-tmpl-rail-list").innerHTML = ""; return; }
    const d = cur();
    $("b-tmpl-rail-list").innerHTML = cats.tmpl.map((label) => {
      const rows = templates.filter((p) => catsOf(p).some((c) => underCategory(c, label)));
      const depth = label.split("/").filter(Boolean).length - 1;
      const dot = d.templateMode === "picked" && d.template && rows.some((p) => p.name === d.template);
      return `<button class="bcat${depth ? " child" : ""}${rows.length ? "" : " zero"}" type="button"`
        + ` aria-pressed="${view.tmpl.cat === label}" data-cat="${esc(label)}" title="${esc(categoryLabel(label))}">`
        + (dot ? '<span class="dot"></span>' : "")
        + `<span class="cl">${esc(leafOf(label))}</span><span class="n">${rows.length}</span></button>`;
    }).join("");
  }

  function drawTmpl() {
    const v = view.tmpl;
    const needle = $("b-tq").value.trim().toLowerCase();
    const hit = (p) => !needle || `${p.name} ${p.summary ?? ""} ${catsOf(p).join(" ")}`.toLowerCase().includes(needle);
    const d = cur();
    const picked = d.templateMode === "picked" ? d.template : null;

    paintModes("tmpl");
    drawTmplViews();
    drawTmplRail();

    if (!templates.length) {
      $("b-tmpl-list").innerHTML = '<div class="bnone">私库里还没有模板。'
        + (promptsPage ? `<br><a href="${esc(promptsPage.url)}/" target="_blank" rel="noreferrer">去模板库建一条 ›</a>` : "")
        + "</div>";
      $("b-tmpl-hint").textContent = "0 条模板";
      return;
    }

    const inScope = (p) =>
      v.cat ? catsOf(p).some((c) => underCategory(c, v.cat))
        : v.v === "none" ? !catsOf(p).length
          : v.v === "basics" ? inCategory(p, DEFAULT_CATEGORY)
            : true;
    const shown = templates.filter(inScope).filter((p) => !v.only || picked === p.name).filter(hit);
    const ghosts = picked && !templates.some((p) => p.name === picked) && hit({ name: picked, summary: "", category: [] })
      ? [{ name: picked, summary: "", category: [], ghost: true }]
      : [];
    // 「无模板」只在「全部」里摆（它不属于任何类别）；只看已选时只在真的什么都没选时出。
    const noneRow = !v.cat && (v.v === "all")
      && (!v.only || d.templateMode === "none") && hit(NONE_ROW);

    if (!shown.length && !noneRow && !ghosts.length) {
      const elsewhere = needle ? templates.filter(hit).length : 0;
      const ways = [
        needle ? '<button class="bchip" type="button" data-clear="1">清空搜索 ›</button>' : "",
        v.only ? '<button class="bchip" type="button" data-only="1">取消「只看已选」›</button>' : "",
        v.cat && elsewhere ? `<button class="bchip" type="button" data-cat="">全部类别里有 ${elsewhere} 条 ›</button>` : "",
      ].filter(Boolean);
      $("b-tmpl-list").innerHTML = `<div class="bnone">没有匹配的模板。${ways.length ? `<br>${ways.join(" ")}` : ""}</div>`;
    } else {
      const groups = [];
      if (v.cat || v.v !== "all") groups.push({ label: null, rows: shown });
      else {
        const order = [DEFAULT_CATEGORY];
        const rest = [...new Set(shown.map(tmplPrimary))]
          .filter((l) => l !== DEFAULT_CATEGORY && l !== UNCATEGORIZED)
          .sort((a, b) => a.localeCompare(b));
        order.push(...rest, UNCATEGORIZED);
        for (const label of order) {
          const rows = shown.filter((p) => tmplPrimary(p) === label);
          if (rows.length) groups.push({ label, rows });
        }
      }
      const head = (label) => (label ? `<div class="bhead">${esc(categoryLabel(label))}</div>` : "");
      $("b-tmpl-list").innerHTML = (noneRow ? tmplRow(NONE_ROW) : "")
        + groups.map(({ label, rows }) => head(label) + rows.map(tmplRow).join("")).join("")
        + (ghosts.length ? `<div class="bhead">仓里有</div>${ghosts.map(tmplRow).join("")}` : "");
    }
    $("b-tmpl-hint").textContent = d.templateMode === "picked" && d.template
      ? `${templates.length} 条模板 · 已选 ${d.template}`
      : d.templateMode === "none" ? `${templates.length} 条模板 · 一条都不要`
        : `${templates.length} 条模板 · 跟随默认 · 点一条就钉住它`;
  }

  function pickTemplate(name) {
    const d = cur();
    if (!name) { d.templateMode = "none"; d.template = ""; }
    else { d.templateMode = "picked"; d.template = name; }
    onChange();
    render();
    drawTmpl();
    $("b-tmpl-list").querySelector(name ? `.brow[data-t="${CSS.escape(name)}"]` : '.brow[data-t=""]')?.focus();
  }

  // ── 扩展清单（族内互斥）────────────────────────────────────────────────
  const famPicked = (fam, picked) => picked.some((n) => familyOf(n) === fam.name);
  const famInCat = (fam, label) => (label === UNCATEGORIZED ? !catsOf(fam.ext).length : inCategory(fam.ext, label));

  const extRow = ({ name, desc, meta, on, off, variant }) =>
    `<button class="brow${variant ? " variant" : ""}" type="button" role="radio" aria-checked="${on}"`
    + ` aria-disabled="${off}" data-e="${esc(name)}"${off ? ' tabindex="-1"' : ""}>`
    + `<span class="bbox"></span>`
    + `<span><span class="bname">${esc(name)}</span>`
    + (desc ? `<span class="bdesc">${esc(desc)}</span>` : "")
    + (meta ? `<span class="bmeta">${esc(meta)}</span>` : "")
    + "</span></button>";

  function drawExtViews() {
    const box = $("b-ext-views");
    const v = view.ext;
    const picked = cur().extensions ?? [];
    const n = families.filter((f) => famPicked(f, picked)).length;
    const bas = families.filter((f) => inCategory(f.ext, DEFAULT_CATEGORY)).length;
    const none = families.filter((f) => !catsOf(f.ext).length).length;
    const paint = (key, pressed, html) => {
      const btn = box.querySelector(`[data-view="${key}"]`);
      if (!btn) return;
      btn.setAttribute("aria-pressed", String(pressed));
      btn.innerHTML = html;
    };
    paint("all", !v.cat && v.v === "all", `全部<span class="n">${families.length}</span>`);
    paint("basics", !v.cat && v.v === "basics", `${esc(DEFAULT_CATEGORY)}<span class="n">${bas}</span>`);
    paint("none", !v.cat && v.v === "none", `未分类<span class="n">${none}</span>`);
    const only = box.querySelector("[data-only]");
    only.setAttribute("aria-pressed", String(v.only));
    only.innerHTML = `只看已选<span class="n">${n}</span>`;
    if (n) { only.removeAttribute("aria-disabled"); only.tabIndex = 0; }
    else { only.setAttribute("aria-disabled", "true"); only.tabIndex = -1; }
  }

  function drawExtRail() {
    const rail = $("b-ext-rail");
    const has = cats.ext.length > 0;
    $("b-ext-scrim").querySelector(".bsheet").classList.toggle("has-rail", has);
    $("b-ext-body").classList.toggle("has-rail", has);
    rail.hidden = !has;
    if (!has) { $("b-ext-rail-list").innerHTML = ""; return; }
    const picked = cur().extensions ?? [];
    $("b-ext-rail-list").innerHTML = cats.ext.map((label) => {
      const rows = families.filter((f) => famInCat(f, label));
      const depth = label.split("/").filter(Boolean).length - 1;
      return `<button class="bcat${depth ? " child" : ""}${rows.length ? "" : " zero"}" type="button"`
        + ` aria-pressed="${view.ext.cat === label}" data-cat="${esc(label)}" title="${esc(categoryLabel(label))}">`
        + (rows.some((f) => famPicked(f, picked)) ? '<span class="dot"></span>' : "")
        + `<span class="cl">${esc(leafOf(label))}</span><span class="n">${rows.length}</span></button>`;
    }).join("");
  }

  function drawExt() {
    const v = view.ext;
    const needle = $("b-eq").value.trim().toLowerCase();
    const hit = (t) => !needle || String(t).toLowerCase().includes(needle);
    const d = cur();
    const picked = d.extensions ?? [];
    const inScope = (fam) =>
      v.cat ? famInCat(fam, v.cat)
        : v.v === "none" ? !catsOf(fam.ext).length
          : v.v === "basics" ? inCategory(fam.ext, DEFAULT_CATEGORY)
            : true;
    const rowHit = (fam) => hit(`${fam.name} ${fam.ext.summary ?? ""} ${catsOf(fam.ext).join(" ")}`);

    paintModes("ext");
    drawExtViews();
    drawExtRail();

    if (!families.length) {
      $("b-ext-list").innerHTML = '<div class="bnone">进程里一个可选扩展都没有。</div>';
      $("b-ext-hint").textContent = "0 个扩展";
      return;
    }

    const shown = families
      .filter(inScope)
      .filter((fam) => !v.only || famPicked(fam, picked))
      .map((fam) => {
        if (rowHit(fam)) return { fam, variants: fam.variants };
        const variants = fam.variants.filter((x) => hit(x.key ?? `${fam.name}@${x.id}`));
        return variants.length ? { fam, variants } : null;
      })
      .filter(Boolean);

    if (!shown.length) {
      const elsewhere = needle
        ? families.filter((fam) => rowHit(fam) || fam.variants.some((x) => hit(x.key ?? `${fam.name}@${x.id}`))).length
        : 0;
      const ways = [
        needle ? '<button class="bchip" type="button" data-clear="1">清空搜索 ›</button>' : "",
        v.only ? '<button class="bchip" type="button" data-only="1">取消「只看已选」›</button>' : "",
        v.cat && elsewhere ? `<button class="bchip" type="button" data-cat="">全部类别里有 ${elsewhere} 个 ›</button>` : "",
      ].filter(Boolean);
      $("b-ext-list").innerHTML = `<div class="bnone">没有匹配的扩展。${ways.length ? `<br>${ways.join(" ")}` : ""}</div>`;
    } else {
      const groups = [];
      if (v.cat || v.v !== "all") groups.push({ label: null, rows: shown });
      else {
        const order = [DEFAULT_CATEGORY];
        const rest = [...new Set(shown.map(({ fam }) => catsOf(fam.ext)[0] ?? UNCATEGORIZED))]
          .filter((l) => l !== DEFAULT_CATEGORY && l !== UNCATEGORIZED)
          .sort((a, b) => a.localeCompare(b));
        order.push(...rest, UNCATEGORIZED);
        for (const label of order) {
          const rows = shown.filter(({ fam }) => (catsOf(fam.ext)[0] ?? UNCATEGORIZED) === label);
          if (rows.length) groups.push({ label, rows });
        }
      }
      $("b-ext-list").innerHTML = groups.map(({ label, rows }) => {
        const head = label ? `<div class="bhead">${esc(categoryLabel(label))}</div>` : "";
        return head + rows.map(({ fam, variants }) => {
          const meta = [catsOf(fam.ext).join(" · "), fam.ext.version ? `v${fam.ext.version}` : ""].filter(Boolean).join(" · ");
          const node = extRow({ name: fam.name, desc: String(fam.ext.summary ?? "").replace(/\s+/g, " ").trim(), meta, on: picked.includes(fam.name), off: false, variant: false });
          const kids = variants.map((x) => {
            const key = x.key ?? `${fam.name}@${x.id}`;
            const failed = x.load?.state === "failed";
            const vmeta = [
              x.source === "repo" ? (Object.keys(x.params ?? {}).length ? `参数 ${JSON.stringify(x.params)}` : "仓库里 · 同代码") : x.branch ? `分支 ${x.branch}` : "本机目录",
              failed ? `装载失败：${String(x.load?.error ?? "").slice(0, 80)}` : !x.installed ? "进程里没装上" : "",
            ].filter(Boolean).join(" · ");
            return extRow({ name: key, desc: "", meta: vmeta, on: picked.includes(key), off: failed || !x.installed, variant: true });
          }).join("");
          return node + kids;
        }).join("");
      }).join("");
    }
    const n = picked.length;
    $("b-ext-hint").textContent = d.extMode === "picked"
      ? `${families.length} 个扩展 · ${n ? `已选 ${n}` : "一个都没选"}`
      : d.extMode === "none" ? `${families.length} 个扩展 · 一个都不要` : `${families.length} 个扩展 · 跟随默认`;
  }

  const pickExt = (name) => {
    const d = cur();
    const picked = new Set(d.extMode === "picked" ? d.extensions ?? [] : []);
    if (picked.has(name)) picked.delete(name);
    else {
      const fam = familyOf(name);
      for (const other of [...picked]) if (familyOf(other) === fam) picked.delete(other);
      picked.add(name);
    }
    d.extMode = "picked";
    d.extensions = [...picked];
    onChange();
    render();
    drawExt();
    $("b-ext-list").querySelector(`.brow[data-e="${CSS.escape(name)}"]`)?.focus();
  };

  const toggleSkill = (name) => {
    const d = cur();
    const picked = new Set(d.skillsMode === "picked" ? d.skills ?? [] : []);
    if (picked.has(name)) picked.delete(name); else picked.add(name);
    d.skillsMode = "picked";
    d.skills = [...picked];
    onChange();
    render();
    drawSkills();
    $("b-skill-list").querySelector(`.brow[data-s="${CSS.escape(name)}"]`)?.focus();
  };

  // ── 事件：盘面 ───────────────────────────────────────────────────────────
  root.addEventListener("click", (e) => {
    const btn = e.target.closest?.(".node");
    if (!btn) return;
    if (btn.id === "b-model") openModel(btn);
    else if (btn.id === "b-dir") openDir(btn);
    else if (btn.id === "b-machine") openMachine(btn);
    else if (btn.id === "b-skills") openSkills(btn);
    else if (btn.id === "b-tmpl") openTmpl(btn);
    else if (btn.id === "b-ext") openExt(btn);
    else if (btn.id === "b-readonly") {
      const d = cur();
      d.readOnly = !d.readOnly;
      onChange();
      render();
    } else if (btn.id === "b-save") { void onSave?.(); }
    else if (btn.id === "b-publish") { void onPublish?.(); }
  });

  // ── 事件：弹窗 ───────────────────────────────────────────────────────────
  for (const id of ["b-model-scrim", "b-skill-scrim", "b-tmpl-scrim", "b-ext-scrim", "b-dir-scrim", "b-machine-scrim"]) {
    $(id).addEventListener("click", (e) => {
      if (e.target === $(id)) closeSheet(id);
    });
  }
  // 弹窗开着的时候，Esc 只收这**一层**，不能顺手把编辑抽屉一起关了。
  // 这一页的 Esc / Alt+D / ⌘S 都挂在 document 上（冒泡比 window 早），所以这里必须用捕获，
  // 在事件往下走之前就截住 —— 早先写成冒泡，结果一按 Esc 连抽屉都没了。
  addEventListener("keydown", (e) => {
    if (!openScrims().length) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      closeSheet(openScrims()[0]);
      return;
    }
    if (e.altKey && e.key.toLowerCase() === "d") { e.preventDefault(); e.stopPropagation(); return; }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") { e.preventDefault(); e.stopPropagation(); return; }
  }, true);

  // 模型弹窗
  $("b-mq").oninput = () => drawModel();
  $("b-mq").onkeydown = (e) => {
    if (e.key !== "Enter") return;
    const first = $("b-diagram").querySelectorAll("g.rule")[1]?.querySelector(".pill");
    if (!first) return;
    e.preventDefault();
    actOn(first);
  };
  $("b-diagram").addEventListener("click", (e) => {
    const g = e.target.closest(".pill");
    if (g) actOn(g);
  });
  $("b-diagram").addEventListener("keydown", (e) => {
    if (e.key !== "Enter" && e.key !== " ") return;
    const g = e.target.closest?.(".pill");
    if (!g) return;
    e.preventDefault();
    actOn(g);
  });
  $("b-model-auto").onclick = () => {
    const d = cur();
    d.model = null;
    onChange();
    render();
    closeSheet("b-model-scrim");
  };
  addEventListener("keydown", (e) => {
    if ($("b-model-scrim").hidden) return;
    if (e.altKey || e.ctrlKey || e.metaKey) return;
    const inQ = e.target === $("b-mq");
    const cols = kbCols();
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const dir = e.key === "ArrowDown" ? 1 : -1;
      if (!kb.on) {
        if (dir < 0) return;
        kb = { on: true, ...kbHome(cols) };
      } else if (dir < 0 && kb.row === 0 && kb.col === 0) {
        kbOff(); $("b-mq").focus(); return;
      } else {
        const len = (cols[kb.col] ?? []).length;
        if (kb.row + dir < 0 || kb.row + dir >= len) { e.preventDefault(); return; }
        kb.row += dir;
      }
      e.preventDefault(); paintKb(); return;
    }
    if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
      if (!kb.on) return;
      e.preventDefault();
      kb.col = Math.max(0, Math.min(cols.length - 1, kb.col + (e.key === "ArrowRight" ? 1 : -1)));
      paintKb(); return;
    }
    if (e.key === "Enter") {
      if (inQ) return;
      const g = (cols[kb.col] ?? [])[kb.row];
      if (!g) return;
      e.preventDefault(); actOn(g); return;
    }
    if (kb.on && !inQ && (e.key.length === 1 || e.key === "Backspace")) {
      e.preventDefault(); kbOff();
      $("b-mq").focus();
      $("b-mq").value = e.key === "Backspace" ? $("b-mq").value.slice(0, -1) : $("b-mq").value + e.key;
      drawModel();
    }
  });

  // 三张清单共用的几件事：档位 chip、视图 chip、右栏、搜索、键盘
  const wireList = (kind, { scrim, list, rail, search, done, draw, drawSheet }) => {
    $(`b-${kind === "skill" ? "skill" : kind}-modes`).addEventListener("click", (e) => {
      const chip = e.target.closest?.(".bmode");
      if (!chip) return;
      setMode(kind, chip.dataset.mode);
      render();
      drawSheet();
      list.querySelector(".brow")?.focus();
    });
    // 视图 chip
    $(`b-${kind === "skill" ? "skill" : kind}-views`).addEventListener("click", (e) => {
      const chip = e.target.closest?.(".bchip");
      if (!chip || chip.getAttribute("aria-disabled") === "true") return;
      const v = view[kind];
      if (chip.dataset.only) v.only = !v.only;
      else if (chip.dataset.view) { v.cat = null; v.v = v.v === chip.dataset.view && chip.dataset.view !== "all" ? "all" : chip.dataset.view; }
      drawSheet();
    });
    // 右栏类别
    rail.addEventListener("click", (e) => {
      const cat = e.target.closest?.(".bcat");
      if (!cat) return;
      const v = view[kind];
      if (v.cat === cat.dataset.cat) { v.cat = null; v.v = "all"; }
      else { v.cat = cat.dataset.cat; v.v = null; }
      drawSheet();
    });
    rail.addEventListener("keydown", (e) => {
      if (e.key !== "ArrowUp" && e.key !== "ArrowDown" && e.key !== "ArrowLeft") return;
      const nodes = [...rail.querySelectorAll(".bcat")];
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        (list.querySelector('.brow[aria-checked="true"]') ?? list.querySelector(".brow"))?.focus();
        return;
      }
      const at = nodes.indexOf(document.activeElement);
      e.preventDefault();
      const i = Math.max(0, Math.min(nodes.length - 1, at < 0 ? 0 : at + (e.key === "ArrowDown" ? 1 : -1)));
      nodes[i]?.focus();
    });
    // 搜索
    search.oninput = () => drawSheet();
    $(done).onclick = () => closeSheet(scrim);
    // 清单里的点击：空状态出口 / 预览 / 选一行
    list.addEventListener("click", (e) => {
      const chip = e.target.closest?.(".bchip");
      if (chip && chip.getAttribute("aria-disabled") !== "true") {
        const v = view[kind];
        if (chip.dataset.clear) { search.value = ""; drawSheet(); search.focus(); return; }
        if (chip.dataset.only) v.only = false;
        else { v.cat = null; v.v = "all"; }
        drawSheet();
        return;
      }
      const peek = e.target.closest?.(".bpeek");
      if (peek) { void togglePeek(peek); return; }
      const row = e.target.closest?.(".brow");
      if (!row || row.getAttribute("aria-disabled") === "true") return;
      if ("t" in row.dataset) pickTemplate(row.dataset.t);
      else if ("s" in row.dataset) toggleSkill(row.dataset.s);
      else if ("e" in row.dataset) pickExt(row.dataset.e);
    });
    // 键盘：行内 ↑↓、←→ 换到右栏、Enter/Space 选（模板行是 div，得手写）
    list.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        const row = document.activeElement;
        if (!row?.dataset) return;
        if ("t" in row.dataset) { e.preventDefault(); pickTemplate(row.dataset.t); }
        else if ("s" in row.dataset) { e.preventDefault(); toggleSkill(row.dataset.s); }
        else if ("e" in row.dataset) { e.preventDefault(); pickExt(row.dataset.e); }
        return;
      }
      if (e.key === "ArrowRight" && !rail.hidden) {
        e.preventDefault();
        rail.querySelector(".bcat")?.focus();
        return;
      }
      if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
      const rows = [...list.querySelectorAll('.brow:not([aria-disabled="true"])')];
      const at = rows.indexOf(document.activeElement);
      e.preventDefault();
      const i = Math.max(0, Math.min(rows.length - 1, at < 0 ? 0 : at + (e.key === "ArrowDown" ? 1 : -1)));
      rows[i]?.focus();
    });
  };

  wireList("skill", { scrim: "b-skill-scrim", list: $("b-skill-list"), rail: $("b-skill-rail"), search: $("b-sq"), done: "b-skill-done", draw: drawSkills, drawSheet: drawSkills });
  wireList("tmpl", { scrim: "b-tmpl-scrim", list: $("b-tmpl-list"), rail: $("b-tmpl-rail"), search: $("b-tq"), done: "b-tmpl-done", draw: drawTmpl, drawSheet: drawTmpl });
  wireList("ext", { scrim: "b-ext-scrim", list: $("b-ext-list"), rail: $("b-ext-rail"), search: $("b-eq"), done: "b-ext-done", draw: drawExt, drawSheet: drawExt });

  render();
  return { render, load };
}
