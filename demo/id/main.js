/* 块 / 组 · 唯一 id —— 十二个方案。
 *
 * 同一棵树、同一串操作，十二种给节点起 id 的规则。
 * 每按一次上面的按钮，所有卡同时重算 —— 谁变了、谁指错了人，一眼可见。
 *
 * 判据只有三条：
 *   稳定  改完结构，同一个 id 还认得出同一块吗（变了 = 麻烦，指错人 = 灾难）
 *   套娃  子 id 是不是以父 id 开头（是的话 startsWith 就等于「我这一整支」）
 *   可念  人能不能在群里打出来、在电话里念出来
 */

/* ── 随机与编码 ─────────────────────────────────────────────────────────────
   32 字母表，去掉 i l o u —— 剩下的随便念都不会和别的字混。
   256 / 32 整除，所以 `x & 31` 取出来的分布没有偏。 */
const A32 = "0123456789abcdefghjkmnpqrstvwxyz";

const rand = (n) => {
  const bytes = new Uint8Array(n);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => A32[b & 31]).join("");
};

const pick = (list) => list[Math.floor(Math.random() * list.length)];

const ADJ = "brave quiet amber solid lunar velvet stark plain swift hollow salt ivory dusk even".split(" ");
const NOUN =
  "otter mango cedar comet harbor falcon willow basalt quartz pylon marlin thistle adder".split(" ");
const wordPair = () => `${pick(ADJ)}-${pick(NOUN)}`;

/** FNV-1a。给「内容派生」那一格用，不追求抗碰撞，只要确定性。 */
const fnv = (s) => {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
};

const slug = (s) =>
  s.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");

const esc = (s) =>
  String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

/** 路径太长就切掉，但切在分隔符上 —— “…a·2/alpha.2” 不如 “…/alpha.2/alpha.2.1” 好读。全长始终在 title 里。 */
const tail = (id) => {
  if (id.length <= 22) return id;
  const cut = id.length - 21;
  const at = id.slice(cut).search(/[/@]/);
  if (at < 0) return `…${id.slice(cut)}`;
  return at === 0 ? `…${id.slice(cut + 1)}` : `…${id.slice(cut + at)}`;
};

const fmtPos = (p) => String(Math.round(p * 1000) / 1000);

/* ── 盘面 ─────────────────────────────────────────────────────────────────── */
let nodeSeq = 0; // 永不重置 —— 它代表「这个对象本身」，重开后也不许复用
let serial = 0; // 重开归零 —— 它代表现状那个模块级计数器，一刷新就重号

const mk = (name, data) => ({
  key: `k${++nodeSeq}`,
  name,
  seg: rand(4), // 段路径用的那段，创建时定，永不变
  uid: rand(8), // 随机八位 / 双标识兜底
  word: wordPair(), // 双词助记
  n: ++serial, // 全局递增
  t: Date.now().toString(32), // 时间序用真时间戳 —— 它不是计数器，重开也不撞
  pos: 0, // 序号留空用的排序键
  ...data,
});

const leaf = (name, value) => mk(name, { value });
const group = (name, children) => mk(name, { children });

const isGroup = (n) => Array.isArray(n.children);
const valueOf = (n) =>
  n.children ? n.children.reduce((sum, c) => sum + valueOf(c), 0) : (n.value ?? 0);

const seedPos = (list) => {
  list.forEach((n, i) => {
    n.pos = (i + 1) * 10;
    if (n.children) seedPos(n.children);
  });
};

/** 开局那棵树：三层，够看出「套娃」是不是真的在 id 里。 */
const build = () => {
  const t = group("N", [
    group("alpha", [
      leaf("alpha.1", 3),
      group("alpha.2", [leaf("alpha.2.1", 2), leaf("alpha.2.2", 1)]),
    ]),
    leaf("beta", 4),
  ]);
  seedPos(t.children);
  return t;
};

let tree;
let anchor;
let renameCount = 0;

/** 重开 = 换一盘棋。计数器归零，但每个节点的 key 永不复用 —— 否则「指错人」测不出来。 */
function rebuild() {
  serial = 0;
  renameCount = 0;
  tree = build();

  const rows = flatten(tree);
  const byName = (name) => rows.find((r) => r.node.name === name).node;
  anchor = { alpha: byName("alpha"), a1: byName("alpha.1"), a2: byName("alpha.2") };
}

/** 摊平成一张表，每一行带着「我爹是谁、我是第几个、我的祖先链」。方案全靠它算 id。 */
function flatten(root) {
  const rows = [];
  const walk = (node, parent, index, ancestors) => {
    const row = { node, parent, index, ancestors, depth: ancestors.length };
    rows.push(row);
    (node.children ?? []).forEach((c, i) => walk(c, row, i, [...ancestors, row]));
  };
  walk(root, null, 0, []);
  return rows;
}

/* ── 十二个方案 ─────────────────────────────────────────────────────────────
   sep 是「父子之间那个分隔符」，只用来做套娃判据：子 id 必须以 `父 id + sep` 开头。
   sep 为 null 的方案结构上就不含关系，判据直接不成立（而不是靠人肉声明）。 */
const SCHEMES = [
  {
    id: "seq",
    fam: "token",
    name: "全局递增",
    examples: "n1 n2 n3",
    read: 3,
    blurb:
      "现状。短、能排序、看得出谁先建的。但它是「这一次运行」的编号，不是「这一块」的名字 —— 刷新、重开、导入别人的盘，全都会重号。",
    make: (r) => `n${r.node.n}`,
  },
  {
    id: "ulid",
    fam: "token",
    name: "时间序",
    examples: "01j8zk07",
    read: 2,
    blurb:
      "时间戳打头（这里是真的 Date.now()），字典序就是创建顺序。它压根不用计数器，所以重开一盘棋也不会和旧盘撞 —— 压到最后一步你会看到它全身而退。",
    make: (r) => `${r.node.t}${r.node.n.toString(32).padStart(2, "0")}`,
  },
  {
    id: "rnd",
    fam: "token",
    name: "随机八位",
    examples: "k7fq2m9x",
    read: 1,
    blurb:
      "32^8 ≈ 1.1 万亿，一盘几千个节点，撞车在噪声以下 —— 永远不用查重。代价是人念不出来，一个字都念不出来。",
    make: (r) => r.node.uid,
  },
  {
    id: "words",
    fam: "token",
    name: "双词助记",
    examples: "brave-otter",
    read: 3,
    blurb:
      "docker 的命名法。比 k7fq2m9x 好念二十倍，代价是词表有限、真的会撞 —— 所以真实系统后面都还得挂一串数字。",
    make: (r) => r.node.word,
  },
  {
    id: "hash",
    fam: "token",
    name: "内容派生",
    examples: "h4f1c",
    read: 1,
    blurb:
      "名字 + 值喂给 FNV。同样的内容永远同一个 id，天然去重 —— 去重到「重开一盘棋」都会撞。改名它要变；更糟的是祖先：子块一加，父组的值就变，一整个祖先链的 id 跟着变。",
    make: (r) => `h${fnv(`${r.node.name}:${valueOf(r.node)}`).toString(32).slice(0, 5)}`,
  },
  {
    id: "typed",
    fam: "token",
    name: "类型前缀",
    examples: "b-k7fq · g-9xm4",
    read: 2,
    blurb:
      "让「块还是组」从 id 里就能看出来。听着很值，但哪天允许「块变组」，前缀就开始说谎 —— 而 id 是存储键，改不了。",
    make: (r) => `${isGroup(r.node) ? "g" : "b"}-${r.node.seg}`,
  },
  {
    id: "namePath",
    fam: "path",
    sep: "/",
    name: "名字路径",
    examples: "N/alpha/alpha.1",
    read: 3,
    blurb:
      "读起来就是这棵树，最好看。也是唯一一个「改一个名字就把一整支的 id 全换掉」的 —— 它把给人看的名字和给机器认的位置绑死了。",
    make: (r) => [...r.ancestors.map((a) => a.node.name), r.node.name].join("/"),
  },
  {
    id: "numPath",
    fam: "path",
    sep: ".",
    name: "序号路径",
    examples: "1.2.1",
    read: 3,
    blurb:
      "书目分类法。最短、套娃最彻底。删掉中间一个，后面的全部改号 —— 而且新号正是旧号，指向的却是另一块。",
    make: (r) => [...r.ancestors.map((a) => a.index + 1), r.index + 1].join("."),
  },
  {
    id: "numGap",
    fam: "path",
    sep: "/",
    name: "序号留空",
    examples: "10/20/10",
    read: 2,
    blurb:
      "10、20、30，插中间就取中点。不再重排，代价是号会退化成像坐标的东西：10/5、10/7.5、10/7.5/5。",
    make: (r) => [...r.ancestors.map((a) => fmtPos(a.node.pos)), fmtPos(r.node.pos)].join("/"),
  },
  {
    id: "segPath",
    fam: "mix",
    sep: "/",
    name: "段路径",
    examples: "k2fq/9xm4/tkd8",
    pick: true,
    read: 1,
    blurb:
      "每段是那个节点自己的不可变短 id，父子靠前缀相连。改名不影响，插兄弟不影响，只有搬家会让地址变 —— 而搬家本来就该变。",
    make: (r) => [...r.ancestors.map((a) => a.node.seg), r.node.seg].join("/"),
  },
  {
    id: "nameAt",
    fam: "mix",
    name: "名字 @ 段",
    examples: "alpha.1@4tkd",
    read: 3,
    blurb:
      "人读前面，机器认后面。名字随便改，@ 后面那截不动，所以永远找得回来。字有点长，但这是把「可读」和「稳定」拆开的最便宜做法。",
    make: (r) => `${r.node.name}@${r.node.seg}`,
  },
  {
    id: "handle",
    fam: "mix",
    name: "双标识",
    examples: "alpha-1-4tkd",
    read: 3,
    blurb:
      "GitHub 那种：登录名给人，uid 给机器。id 里带 handle 只是为了好看，真正的引用是那条永不失效的 uid。想让人分享得出口，又不想一改名全崩，就这个。",
    make: (r) => `${slug(r.node.name)}-${r.node.seg}`,
  },
];

const FAMILIES = [
  { id: "all", label: "全部" },
  { id: "token", label: "不含关系 · token" },
  { id: "path", label: "含关系 · 地址" },
  { id: "mix", label: "地址 + 不可变段" },
];

/* ── 记账 ───────────────────────────────────────────────────────────────────
   H 历史：这个 id 曾经指向谁（用来抓「指错人」）
   L 上一帧：这一块上次叫什么（用来抓「变了」）
   C 累计：一共被改掉多少次、指错过多少次 */
const H = new Map();
const L = new Map();
const C = new Map();

let RESULT = [];
let filter = "all";
const log = [];

/* ── 量一次 ───────────────────────────────────────────────────────────────── */
function idsFor(scheme) {
  const rows = flatten(tree);
  const map = new Map();
  rows.forEach((r) => map.set(r.node.key, scheme.make(r)));
  return { rows, map };
}

function measure() {
  RESULT = SCHEMES.map((scheme) => {
    const { rows, map } = idsFor(scheme);

    const hist = H.get(scheme.id) ?? new Map();
    const last = L.get(scheme.id) ?? new Map();
    const cum = C.get(scheme.id) ?? { changed: 0, wrong: 0 };

    const status = new Map();
    for (const [key, id] of map) {
      // 这个 id 在历史上指向过别人 —— 比「变了」更糟：它不报错，它静静指错。
      if (hist.has(id) && hist.get(id) !== key) status.set(key, "wrong");
      else if (last.has(key) && last.get(key) !== id) status.set(key, "changed");
      else status.set(key, "ok");
    }

    for (const st of status.values()) {
      if (st === "changed") cum.changed++;
      if (st === "wrong") cum.wrong++;
    }

    for (const [key, id] of map) {
      hist.set(id, key);
      last.set(key, id);
    }
    // 历史不许无限长，不然卡越按越慢。
    if (hist.size > 400) for (const k of [...hist.keys()].slice(0, hist.size - 400)) hist.delete(k);
    H.set(scheme.id, hist);
    L.set(scheme.id, last);
    C.set(scheme.id, cum);

    const ids = [...map.values()];
    const lens = ids.map((v) => v.length);
    const nests =
      scheme.sep !== undefined &&
      rows.every(
        (r) =>
          !r.parent ||
          map.get(r.node.key).startsWith(`${map.get(r.parent.node.key)}${scheme.sep}`),
      );

    const count = (want) => [...status.values()].filter((x) => x === want).length;

    return {
      scheme,
      rows,
      map,
      status,
      cum,
      nests,
      lenMin: Math.min(...lens),
      lenMax: Math.max(...lens),
      ok: count("ok"),
      changed: count("changed"),
      wrong: count("wrong"),
    };
  });
}

/* ── 画 ───────────────────────────────────────────────────────────────────── */
const deck = document.getElementById("deck");
const tabs = document.querySelector(".tabs");
const logEl = document.getElementById("log");
const findEl = document.getElementById("find");

const dots = (read) => "●".repeat(read) + "○".repeat(3 - read);

function rowHTML(r) {
  return (row) => {
    const id = r.map.get(row.node.key);
    const st = r.status.get(row.node.key);
    const g = isGroup(row.node);
    return `
      <div class="row" style="--d:${row.depth}">
        <span class="chev">${g ? "▸" : ""}</span>
        <span class="nm ${g ? "" : "leaf"}">${esc(row.node.name)}</span>
        ${g ? `<span class="count">${row.node.children.length} 块</span>` : ""}
        <span class="val">${valueOf(row.node)}</span>
        <button class="id st-${st}" data-id="${esc(id)}" title="${esc(id)}（点一下复制）">${esc(tail(id))}</button>
      </div>`;
  };
}

function cardHTML(r, i) {
  const s = r.scheme;
  const no = String(i + 1).padStart(2, "0");
  return `
    <article class="card" data-fam="${s.fam}" id="card-${s.id}">
      <div class="card-head">
        <span class="num">${no}</span>
        <div>
          <h2>${s.name}<span class="tag ${s.pick ? "pick" : ""}">${
            s.pick ? "推荐" : s.examples
          }</span></h2>
          <p>${s.blurb}</p>
          <div class="specs">
            <span>长度 <b>${r.lenMin}–${r.lenMax}</b></span>
            <span>套娃 <b class="${r.nests ? "" : "no"}">${r.nests ? "✓" : "✗"}</b></span>
            <span>可念 <b>${dots(s.read)}</b></span>
          </div>
        </div>
      </div>
      <div class="plate">
        <div class="panel">
          <div class="panel-head">
            <span class="grow">结构</span><span class="hval">值</span><span class="hid">id</span>
          </div>
          <div class="rows">${r.rows.map(rowHTML(r)).join("")}</div>
        </div>
        <div class="stats">
          <span class="s"><i class="dot" data-k="ok"></i>没动 <b>${r.ok}</b></span>
          <span class="s changed"><i class="dot" data-k="changed"></i>变了 <b>${r.changed}</b></span>
          <span class="s wrong"><i class="dot" data-k="wrong"></i>指错人 <b>${r.wrong}</b></span>
          <span class="cum">累计 变了 ${r.cum.changed} · 指错 ${r.cum.wrong}</span>
        </div>
        <div class="find-line" data-scheme="${s.id}"></div>
      </div>
    </article>`;
}

function paint() {
  deck.innerHTML = RESULT.map(cardHTML).join("");
  applyFilter();
  updateFind();
}

function applyFilter() {
  deck.querySelectorAll(".card").forEach((el) => {
    el.hidden = filter !== "all" && el.dataset.fam !== filter;
  });
}

function renderLog() {
  if (!log.length) {
    logEl.innerHTML = `<span class="step">按上面任意一个按钮 —— 十二张卡会同时重算，颜色只标「上一步」的损伤。</span>`;
    return;
  }
  logEl.innerHTML = log
    .slice(-4)
    .reverse()
    .map(
      (step, i) => `
      <span class="step ${i === 0 ? "latest" : ""}">
        <span class="no">${String(step.no).padStart(2, "0")}</span>${esc(step.text)}
      </span>`,
    )
    .join("");
}

/* ── 粘一个 id 回去 ───────────────────────────────────────────────────────── */
function updateFind() {
  const q = findEl.value.trim();
  deck.querySelectorAll(".find-line").forEach((el) => {
    const s = SCHEMES.find((x) => x.id === el.dataset.scheme);
    if (!q) {
      el.innerHTML = `<span>粘一个 id 进来，看这一格认不认得。</span>`;
      return;
    }
    const rows = flatten(tree);
    const hit = rows.find((r) => s.make(r) === q);
    if (hit) {
      el.innerHTML = `<span class="ok">✓ 认得出：</span>${esc(hit.node.name)} <code>${esc(
        s.make(hit),
      )}</code>`;
      return;
    }
    const sub = rows.filter((r) => s.make(r).startsWith(q));
    if (sub.length) {
      el.innerHTML = `<span class="ok">✓ 前缀命中一整支：</span>${sub.length} 块，${esc(
        sub[0].node.name,
      )} 起`;
      return;
    }
    el.innerHTML = `<span class="bad">✗ 找不到这一块。</span>`;
  });
}

/* ── 压力测试 ─────────────────────────────────────────────────────────────── */
const kids = (n) => n.children ?? [];

/** 在两个兄弟之间塞一个号：中点。两端各 +10 / 折半。 */
function gapPos(list, i) {
  const prev = i > 0 ? list[i - 1].pos : null;
  const next = i < list.length ? list[i].pos : null;
  if (prev === null && next === null) return 10;
  if (prev === null) return next / 2;
  if (next === null) return prev + 10;
  return (prev + next) / 2;
}

const rowOf = (node) => flatten(tree).find((r) => r.node.key === node.key);

const OPS = {
  insert() {
    const p = anchor.alpha;
    const k = leaf(`${p.name}.new`, 2);
    k.pos = gapPos(kids(p), 0);
    p.children.unshift(k);
    return `在 ${p.name} 最前面插一块 —— 序号派后面的兄弟全部换号，新号正是旧号`;
  },
  delete() {
    const target = anchor.a1;
    const r = rowOf(target);
    if (!r?.parent) return `${target.name} 已经不在了`;
    r.parent.node.children.splice(r.index, 1);
    return `删掉中间的 ${target.name} —— 位置派集体前移一格`;
  },
  rename() {
    renameCount += 1;
    const t = anchor.alpha;
    const before = t.name;
    t.name = `alpha·${renameCount + 1}`;
    return `把 ${before} 改名成 ${t.name} —— 名字派一整支换号，随机派一个都不动`;
  },
  move() {
    const t = anchor.a2;
    const r = rowOf(t);
    if (!r?.parent) return `${t.name} 已经不在了`;
    r.parent.node.children.splice(r.index, 1);
    t.pos = gapPos(kids(tree), kids(tree).length);
    tree.children.push(t);
    return `把 ${t.name} 整支搬到顶层 —— 含地址的整支变号；只带自己名字的（名字@段、双标识）纹丝不动，因为它们本来就不表示位置`;
  },
  reset() {
    rebuild();
    return "重开 —— 计数器归零，第 1 块又叫 n1；而上辈子的 n1 是另一块，所以序号派在这里「指错人」";
  },
};

function run(opId) {
  const text = OPS[opId]();
  log.push({ no: log.length + 1, text });
  measure();
  paint();
  renderLog();
}

document.querySelectorAll(".op").forEach((b) => {
  b.addEventListener("click", () => run(b.dataset.op));
});

/* ── 复制 id ──────────────────────────────────────────────────────────────── */
const copy = async (text) => {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    document.body.appendChild(ta);
    ta.select();
    document.execCommand("copy");
    ta.remove();
  }
};

deck.addEventListener("click", (e) => {
  const btn = e.target.closest(".id");
  if (!btn) return;
  copy(btn.dataset.id);
  btn.classList.add("copied");
  setTimeout(() => btn.classList.remove("copied"), 700);
});

/* ── 筛选 / 主题 / 粘贴 ───────────────────────────────────────────────────── */
function renderTabs() {
  const count = (fam) => SCHEMES.filter((s) => fam === "all" || s.fam === fam).length;
  tabs.innerHTML = FAMILIES.map(
    (f) => `<button class="chip" data-id="${f.id}" aria-pressed="${
      f.id === filter
    }">${f.label} · ${count(f.id)}</button>`,
  ).join("");

  tabs.querySelectorAll(".chip").forEach((chip) => {
    chip.addEventListener("click", () => {
      filter = chip.dataset.id;
      tabs.querySelectorAll(".chip").forEach((c) => c.setAttribute("aria-pressed", String(c === chip)));
      applyFilter();
    });
  });
}

const root = document.documentElement;
const themeBtn = document.getElementById("theme");
const paintTheme = () => {
  const dark = root.dataset.theme !== "light";
  themeBtn.textContent = dark ? "浅色" : "深色";
  themeBtn.setAttribute("aria-pressed", String(!dark));
};
themeBtn.addEventListener("click", () => {
  root.dataset.theme = root.dataset.theme === "light" ? "default" : "light";
  localStorage.setItem("demo-theme", root.dataset.theme);
  paintTheme();
});
root.dataset.theme = localStorage.getItem("demo-theme") ?? "default";
paintTheme();

findEl.addEventListener("input", updateFind);

/* ── 起 ───────────────────────────────────────────────────────────────────── */
rebuild();
measure();
paint();
renderTabs();
renderLog();
