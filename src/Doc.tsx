import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type Node, findNode, treeMap } from "./game";
import { normalizeDocName } from "./docname";
import { copyText } from "./clipboard";
import { NodeMenuProvider, NodeMenuTrigger, type MenuItem } from "./NodeMenu";
import {
  type DocFile,
  DocConflictError,
  DocEditError,
  clearDocDraft,
  clearLastDoc,
  createDoc,
  listDocs,
  loadDoc,
  readDocDraft,
  readLastDoc,
  renameDoc,
  restoreDoc,
  saveDoc,
  trashDoc,
  writeDocDraft,
  writeLastDoc,
} from "./store";
import "./doc.css";

/**
 * 一块的文档页。**真实路径**：`/p/<board>/<nodeId>/__nous/doc`。
 *
 * 形状是「左列文件 + 右边一张纸」，和盘面同一套规矩：
 *
 * - **左列可以收起。** 和盘面那颗一模一样的按钮（一个框、中间一条竖线），同一个手感：
 *   宽 `248 → 0` 的宽度过渡，窄屏是一张盖在正文上的抽屉。状态记在这台浏览器里。
 * - **左列就是这个文件夹。** `map/<id>/` 里有什么就列什么 —— 列表是 `readdir` 的结果，
 *   不是宿主另存的一份清单。盘上多一个文件，这里就多一行。
 * - **鼠标端的一切入口都是右键。** 行上不放悬停才浮出来的小图标：一张菜单里有
 *   打开 / 重命名 / 复制名字 / 移到最近删除。触屏没有右键，那颗 ••• 才出现（和盘面共用）。
 * - **右边只放当前这份。** 换文件只换右边，左列的滚动位置和选中都不跳。
 * - **正文有两种看法**：源码（能改）和预览（渲染出来，像 GitHub 上看一份 `.md`）。
 *   切的是「同一条内容怎么显示」，不是两份数据 —— 没存的字在两边都是同一份。预览器是懒加载的：
 *   只看源码的人不为它付首包的体积。
 * - **`AGENTS.md` 只是名字特殊**：它在的时候能改、不能删、不能改名 —— 它是 coding agent
 *   进目录时自动找的文件名，删了整块就没人读得懂。但它不预生成：没有就是没有。
 *
 * 它盖在盘面上面、不卸载盘面：卸载会丢掉树的展开状态、滚动位置和撤销栈。
 * 每份文档有自己的 etag 和自己的本地草稿：写一篇不该把别的文档顶成 409。
 */
const SAVE_DELAY = 400;
const ENTRY = "AGENTS.md";
/** 窄到这个宽度以下，左列就是一张抽屉而不是一列。 */
const NARROW = "(max-width: 819px)";
/** 左列开着没有：住在这台浏览器里（和盘面那个 `nous-sidebar-open` 同一档，不进 tree.json）。 */
const SIDE_KEY = "nous-doc-side";
/** 左列是 248px —— 和盘面一样宽，两边切来切去不该有第二个宽度。 */
const EASE = "ease-[cubic-bezier(0.32,0.72,0,1)]";
/**
 * 右键落在清单的空白处 = 想新建。它不是一份文档，所以借一个不会和文件名撞的 id：
 * 文件名都带 `.md`（`normalizeDocName` 保证），`__padding` 撞不上。
 */
const NEW_ID = "__padding";
/**
 * 正文用哪种看法。住在浏览器里：这是「这台机器上我怎么读字」，不是文档的属性，
 * 不该跟着文件走、更不该进 `tree.json`。默认源码 —— 这一页本来就是来改东西的。
 */
const VIEW_KEY = "nous-doc-view";

/**
 * 渲染器是单独一包，第一次按「预览」才去拿。放在模块顶层（而不是 `Doc.tsx` 里 inline 一个
 * `lazy`），换文档、切模式都不会重造一个 lazy 组件 —— 那会让 Suspense 每次都重新挂一次。
 */
const DocMarkdown = lazy(() => import("./Markdown"));

type View = "source" | "preview";

type Phase = "loading" | "ready" | "missing" | "error";
type Status = "idle" | "saving" | "saved" | "conflict" | "error";

const STATUS_TEXT: Record<Status, string> = {
  idle: "",
  saving: "保存中",
  saved: "已保存",
  conflict: "冲突",
  error: "没连上",
};

const words = (t: string): number => t.replace(/\s/g, "").length;
const lines = (t: string): number => t.split("\n").length;

/** 名字拆成「干」和「尾巴」：干能截断，尾巴不截 —— `能…md` 比 `能…` 好认得多。 */
const splitName = (name: string): [string, string] => {
  const stem = name.replace(/\.md$/i, "");
  return [stem, name.slice(stem.length)];
};

/** 默认源码模式：只有你亲手切到预览、且它记下来了，才是渲染的。 */
const readView = (): View => {
  try {
    return localStorage.getItem(VIEW_KEY) === "preview" ? "preview" : "source";
  } catch {
    return "source";
  }
};

/** 左列默认开着 —— 这是文档页的主导航。只有你亲手收起来过、或者屏幕窄，它才是关的。 */
const readSide = (): boolean => {
  try {
    const v = localStorage.getItem(SIDE_KEY);
    if (v === null) return !window.matchMedia(NARROW).matches;
    return v === "1";
  } catch {
    return !window.matchMedia(NARROW).matches;
  }
};

/* ------------------------------------------------------------------ *
 * 一份文档的读写。按名字分：切文档 = 换一份 state + 换一把 etag。
 *
 * 没发出去的那一版（`pending`）和它的底（`baseEtag`）都在这里，
 * 切走 / 关页面的时候会先把它们推出去 —— 字不该因为「换了一篇」就没了。
 * ------------------------------------------------------------------ */
function useDocFile(board: string, node: string, name: string, enabled: boolean) {
  const [phase, setPhase] = useState<Phase>("loading");
  const [text, setText] = useState("");
  const [status, setStatus] = useState<Status>("idle");
  /** 磁盘上那份（409 带回来的）。两边的字都还在，等人选。 */
  const [theirs, setTheirs] = useState<{ text: string; etag: string } | null>(null);
  /** 本地草稿，但它基于的是旧版本 —— 不能无感补写。 */
  const [orphan, setOrphan] = useState<{ text: string; etag: string } | null>(null);
  const [msg, setMsg] = useState("");

  /** 还没发出去的那一版。`null` = 干净。 */
  const pending = useRef<string | null>(null);
  /** 写盘串行：两个 PUT 同时在飞，旧的后到就会把新敲的字盖掉。 */
  const writing = useRef(false);
  /** 我这一版建立在磁盘哪个 etag 上。 */
  const baseEtag = useRef("");
  /** 磁盘上确认过的最后一份。和它相同就不重复写。 */
  const lastSaved = useRef("");
  /** 撞了版本就停手，等人选 —— 继续自动写等于帮另一个标签页删字。 */
  const blocked = useRef(false);
  /** 最新的一份文本，给 flush 用（effect 闭包里拿不到 state）。 */
  const latest = useRef("");
  const timer = useRef(0);

  const drain = useCallback(async () => {
    if (writing.current || blocked.current) return;
    writing.current = true;
    try {
      while (pending.current !== null && !blocked.current) {
        const body = pending.current;
        pending.current = null;
        if (body === lastSaved.current) {
          clearDocDraft(board, node, name);
          setStatus("saved");
          continue;
        }
        try {
          const { etag } = await saveDoc(board, node, name, body, baseEtag.current);
          baseEtag.current = etag;
          lastSaved.current = body;
          clearDocDraft(board, node, name);
          setStatus("saved");
        } catch (err) {
          if (err instanceof DocConflictError) {
            blocked.current = true;
            pending.current = null;
            setTheirs({ text: err.text, etag: err.etag });
            setStatus("conflict");
          } else {
            // 网络抖一下：草稿已经落在本地了，下一次落键或离开再试。
            pending.current = body;
            setStatus("error");
            break;
          }
        }
      }
    } finally {
      writing.current = false;
    }
  }, [board, node, name]);

  /** 离开前把最后那几个字推出去。keepalive 只给真的要走的场景（它有 64KB 上限）。 */
  const flush = useCallback(
    (keepalive: boolean) => {
      const body = pending.current;
      if (body === null || body === lastSaved.current || blocked.current) return;
      writeDocDraft(board, node, name, body, baseEtag.current);
      void saveDoc(board, node, name, body, baseEtag.current, keepalive)
        .then(({ etag }) => {
          baseEtag.current = etag;
          lastSaved.current = body;
          clearDocDraft(board, node, name);
        })
        .catch(() => {
          // 失败就留在草稿里，下次进来先看它。
        });
    },
    [board, node, name],
  );

  /* —— 载入：磁盘那份 + 可能躺着的草稿 —— */
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    setPhase("loading");
    setStatus("idle");
    setTheirs(null);
    setOrphan(null);
    pending.current = null;
    blocked.current = false;
    latest.current = "";
    void (async () => {
      try {
        const doc = await loadDoc(board, node, name);
        if (!alive) return;
        if (!doc) {
          setPhase("missing");
          return;
        }
        baseEtag.current = doc.etag;
        lastSaved.current = doc.text;
        latest.current = doc.text;
        setText(doc.text);
        setPhase("ready");
        setStatus("saved");

        const draft = readDocDraft(board, node, name);
        if (draft && draft.text !== doc.text) {
          if (draft.etag === doc.etag) {
            // 只是没发出去：无感补写，不麻烦用户。
            latest.current = draft.text;
            setText(draft.text);
            pending.current = draft.text;
            setStatus("saving");
            void drain();
          } else {
            // 底不一样：磁盘被别处改过。两边都留着，等人选。
            setOrphan({ text: draft.text, etag: draft.etag });
          }
        } else if (draft) {
          clearDocDraft(board, node, name);
        }
      } catch (e) {
        if (!alive) return;
        setMsg(e instanceof Error ? e.message : "读不到");
        setPhase("error");
      }
    })();
    return () => {
      alive = false;
    };
  }, [board, node, name, enabled, drain]);

  /* —— 切后台 / 关页面：最后那几个字不能丢 —— */
  useEffect(() => {
    if (!enabled) return;
    const hide = () => {
      if (document.hidden) flush(true);
    };
    const leave = () => flush(true);
    window.addEventListener("pagehide", leave);
    document.addEventListener("visibilitychange", hide);
    return () => {
      window.removeEventListener("pagehide", leave);
      document.removeEventListener("visibilitychange", hide);
      window.clearTimeout(timer.current);
      // 换文档 / 从页面上离开（Esc / 后退）不是页面卸载，普通 fetch 就够了。
      flush(false);
    };
  }, [flush, enabled]);

  const edit = (value: string) => {
    latest.current = value;
    setText(value);
    pending.current = value;
    setStatus("saving");
    writeDocDraft(board, node, name, value, baseEtag.current);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => void drain(), SAVE_DELAY);
  };

  const saveNow = () => {
    pending.current = latest.current;
    void drain();
  };

  /** 用磁盘的：我的这几笔不要了。 */
  const takeTheirs = () => {
    if (!theirs) return;
    blocked.current = false;
    pending.current = null;
    baseEtag.current = theirs.etag;
    lastSaved.current = theirs.text;
    latest.current = theirs.text;
    setText(theirs.text);
    setTheirs(null);
    clearDocDraft(board, node, name);
    setStatus("saved");
  };

  /** 用我的：认下磁盘的新版本，然后把我的字盖上去。 */
  const keepMine = () => {
    if (!theirs) return;
    blocked.current = false;
    baseEtag.current = theirs.etag;
    pending.current = latest.current;
    setTheirs(null);
    setStatus("saving");
    void drain();
  };

  const useOrphan = () => {
    if (!orphan) return;
    pending.current = orphan.text;
    latest.current = orphan.text;
    setText(orphan.text);
    setOrphan(null);
    setStatus("saving");
    void drain();
  };

  const dropOrphan = () => {
    clearDocDraft(board, node, name);
    setOrphan(null);
  };

  return {
    phase,
    text,
    status,
    theirs,
    orphan,
    msg,
    edit,
    saveNow,
    takeTheirs,
    keepMine,
    useOrphan,
    dropOrphan,
  };
}

/* ------------------------------------------------------------------ *
 * 手机上那张清单：往左推就送它回去。
 *
 * 规矩来自「手和纸一起动」：
 *
 * - **1:1 跟手。** 上手的那一帧起就把过渡关掉 —— 中间隔一层缓动，手指和纸之间就有水。
 * - **10px 才分家。** 竖着先过 10px = 在滚清单，别再抢；横着过 10px 才认定是推纸。
 * - **右边界是橡皮筋。** 往左推到顶就停住（那边是屏幕沿）；往右没有东西可露，就递减着走。
 * - **松手看落点，不看当前位置。** 用速度把落点投出去一段（`project`），投过半张就关，
 *   投不动就弹回来 —— 一把甩出去该比慢慢推更算数。
 * - **一次 React 渲染都不发生。** 位置直接写进 DOM（这一页的正文可能有一千行，
 *   每帧 setState 是真的会卡）。只有「到底关没关」最后进一次 state。
 * ------------------------------------------------------------------ */
const DECEL = 0.998;
/** 速度 → 还会滑多远（px）。苹果那条指数衰减，不是物理课本上的 v²/2a。 */
const project = (v: number): number => ((v / 1000) * DECEL) / (1 - DECEL);

function useSheetDrag(onDismiss: () => void) {
  const box = useRef<HTMLElement | null>(null);
  const scrim = useRef<HTMLElement | null>(null);
  const g = useRef({
    id: -1,
    x0: 0,
    y0: 0,
    x: 0,
    w: 1,
    grab: false,
    dead: false,
    pts: [] as { x: number; t: number }[],
  });
  const timer = useRef(0);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  const down = (e: React.PointerEvent) => {
    const el = box.current;
    if (!el || (e.pointerType === "mouse" && e.button !== 0)) return;
    // 在输入框里横着拖是在选字，不是推纸。
    if ((e.target as HTMLElement).closest("input, textarea")) return;
    window.clearTimeout(timer.current);
    g.current = {
      id: e.pointerId,
      x0: e.clientX,
      y0: e.clientY,
      x: 0,
      w: el.offsetWidth || 1,
      grab: false,
      dead: false,
      pts: [{ x: e.clientX, t: performance.now() }],
    };
  };

  const move = (e: React.PointerEvent) => {
    const el = box.current;
    const s = g.current;
    if (!el || e.pointerId !== s.id || s.dead) return;
    const dx = e.clientX - s.x0;
    const dy = e.clientY - s.y0;

    if (!s.grab) {
      if (Math.abs(dy) > 10 && Math.abs(dy) > Math.abs(dx)) {
        s.dead = true;
        return;
      }
      if (Math.abs(dx) < 10 || Math.abs(dx) <= Math.abs(dy)) return;
      s.grab = true;
      el.style.transition = "none";
      el.style.willChange = "transform";
      try {
        el.setPointerCapture(e.pointerId);
      } catch {
        // 拿不到捕获也照样拖（手在纸里面就够）；只是手滑出去会断。
      }
    }

    s.pts.push({ x: e.clientX, t: performance.now() });
    if (s.pts.length > 6) s.pts.shift();

    // 往左跟着手指直到顶到屏幕沿；往右没有东西可露，只给一点软阻力。
    s.x = dx < 0 ? Math.max(dx, -s.w) : (dx * s.w * 0.55) / (s.w + 0.55 * Math.abs(dx));
    el.style.transform = `translate3d(${s.x}px, 0, 0)`;
    // 幕布跟着手指一起淡 —— 连续反馈，不是松手才变。只认往左那一段：
    // 往上拉橡皮筋不该把幕布又提亮（那是「往回走」，不是「接着走」）。
    if (scrim.current) {
      scrim.current.style.opacity = String(Math.max(0, 1 - 0.75 * (Math.max(0, -s.x) / s.w)));
    }
  };

  const end = (e: React.PointerEvent, cancelled = false) => {
    const el = box.current;
    const s = g.current;
    if (!el || e.pointerId !== s.id) return;
    s.id = -1;
    if (!s.grab) return;
    s.grab = false;
    el.style.willChange = "";

    // 速度只看最后 110ms：一把甩出去的话，只有尾巴算数。
    const now = performance.now();
    const tail = s.pts.filter((p) => now - p.t < 110);
    const a = tail[0];
    const b = tail[tail.length - 1];
    const v = !cancelled && a && b && b.t > a.t ? ((b.x - a.x) / (b.t - a.t)) * 1000 : 0;

    if (s.x + project(v) < -s.w * 0.4) {
      el.style.transition = "transform 240ms var(--dp-ease-out)";
      el.style.transform = "translate3d(-100%, 0, 0)";
      if (scrim.current) scrim.current.style.opacity = "0";
      // 动画走完再翻状态 —— 那时候 class 也是 -100%，接手不跳。
      timer.current = window.setTimeout(() => {
        if (box.current) {
          box.current.style.transition = "";
          box.current.style.transform = "";
        }
        if (scrim.current) scrim.current.style.opacity = "";
        onDismiss();
      }, 250);
    } else {
      // 投不动：弹簧把它拾回去。从**现在这个位置**开始，不是从 0 开始。
      el.style.transition = "transform 420ms var(--dp-spring)";
      el.style.transform = "";
      if (scrim.current) scrim.current.style.opacity = "";
      timer.current = window.setTimeout(() => {
        if (box.current) box.current.style.transition = "";
      }, 460);
    }
  };

  return {
    box,
    scrim,
    handlers: {
      onPointerDown: down,
      onPointerMove: move,
      onPointerUp: (e: React.PointerEvent) => end(e),
      onPointerCancel: (e: React.PointerEvent) => end(e, true),
    },
  };
}

/* ------------------------------------------------------------------ *
 * 页面
 * ------------------------------------------------------------------ */
export function DocPage({
  board,
  id,
  root,
  ready,
  onClose,
}: {
  board: string;
  id: string;
  root: Node;
  /** 树载完了没有。没载完就说「载入中」，别急着喊「查无此块」。 */
  ready: boolean;
  onClose: () => void;
}) {
  const node = findNode(root, id);
  const tree = useMemo(() => treeMap(root).get(id) ?? node?.name ?? id, [root, id, node]);

  const [narrow, setNarrow] = useState(() => window.matchMedia(NARROW).matches);
  /** 左列开着吗。宽屏决定那一列在不在，窄屏决定那张抽屉在不在 —— 一个开关，和盘面一样。 */
  const [side, setSide] = useState(() => (window.matchMedia(NARROW).matches ? false : readSide()));

  const [files, setFiles] = useState<DocFile[]>([]);
  const [hasApp, setHasApp] = useState(false);
  /** 清单至少载过一次。没载完不说「没有文档」—— 那是在猜。 */
  const [listed, setListed] = useState(false);
  const [cur, setCur] = useState<string>(() => readLastDoc(board, id) ?? "");

  /** 列表顶上那个输入框。新建和改名共用它，一次只开一个。 */
  const [adding, setAdding] = useState(false);
  /** 正在改名的那一份。`null` = 没在改。 */
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  /** 一句给眼睛看的回执（「建好了」「改名了」「没删成」）。浮起来，几秒后自己走。 */
  const [note, setNote] = useState("");
  /** 左边那条搜索框。空着就是全量列表，有字就交给服务端搜（名字 + 正文）。 */
  const [q, setQ] = useState("");
  /** 搜出来的那一拨。`null` = 没在搜。 */
  const [found, setFound] = useState<DocFile[] | null>(null);
  const [trashOpen, setTrashOpen] = useState(false);
  /** 正文里那个标题：点一下就地改名。 */
  const [titleEdit, setTitleEdit] = useState(false);
  /** 正文是源码还是渲染出来的那份。 */
  const [view, setView] = useState<View>(readView);
  /** 正文滚开了没有。滚开了，顶上那条才长出一条发丝边 —— 没滚的时候它是透明的。 */
  const [scrolled, setScrolled] = useState(false);
  /** 纸上的大标题已经滑到条子底下了没有。滑过去了，条子上才把名字拿出来（iOS 的大标题）。 */
  const [past, setPast] = useState(false);
  const body = useRef<HTMLTextAreaElement | null>(null);
  const scroll = useRef<HTMLDivElement | null>(null);
  const bar = useRef<HTMLElement | null>(null);
  const pageTitle = useRef<HTMLDivElement | null>(null);
  const skipBlur = useRef(false);
  const renamed = useRef(false);
  const search = useRef<HTMLInputElement | null>(null);
  const sheet = useSheetDrag(() => setSide(false));

  const closeInputs = useCallback(() => {
    setAdding(false);
    setRenaming(null);
  }, []);

  const pickView = useCallback((v: View) => {
    setView(v);
    try {
      localStorage.setItem(VIEW_KEY, v);
    } catch {
      /* 记不住就记不住，这一次照样切。 */
    }
  }, []);

  /* 切回源码 = 想动手了。光标直接落回纸上，不用再点一下正文。 */
  useEffect(() => {
    if (view === "source") body.current?.focus();
  }, [view]);

  /* 回执不占地方：3.4 秒，够看完一句话。 */
  useEffect(() => {
    if (!note) return;
    const t = window.setTimeout(() => setNote(""), 3400);
    return () => window.clearTimeout(t);
  }, [note]);

  const doc = useDocFile(board, id, cur, ready && !!node && !!cur);

  const refresh = useCallback(async () => {
    const list = await listDocs(board, id);
    if (!list) return null;
    setFiles(list.files);
    setHasApp(list.hasApp);
    setListed(true);
    return list.files;
  }, [board, id]);

  useEffect(() => {
    if (!ready || !node) return;
    void refresh();
  }, [ready, node, refresh]);

  useEffect(() => {
    const mq = window.matchMedia(NARROW);
    const on = () => setNarrow(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);

  /* 换了一份：标题回到「看的」状态，别把上一份的输入框留在那儿；
     纸回到顶上 —— 从一份的第 800 行跳到另一份，落点不该是它的第 800 行。 */
  useEffect(() => {
    setTitleEdit(false);
    setScrolled(false);
    setPast(false);
    scroll.current?.scrollTo({ top: 0 });
  }, [cur]);

  /* 左列的开合记在这台浏览器里。手机上那一下不动桌面上的偏好 —— 屏幕小不是「我收起来了」。 */
  useEffect(() => {
    if (narrow) return;
    try {
      localStorage.setItem(SIDE_KEY, side ? "1" : "0");
    } catch {
      // 隐私模式下写不进去就算了，开合本身照样能用。
    }
  }, [side, narrow]);

  /* —— 当前在看哪一份 → 记住；一份都没有就把记忆也抹掉 —— */
  useEffect(() => {
    if (!ready || !node) return;
    if (cur) writeLastDoc(board, id, cur);
    else clearLastDoc(board, id);
  }, [board, id, cur, ready, node]);

  const live = useMemo(() => files.filter((f) => !f.trashed), [files]);
  const gone = useMemo(() => files.filter((f) => f.trashed), [files]);

  const current = useMemo(() => files.find((f) => f.name === cur), [files, cur]);

  /* —— 载完清单才定「在看哪一份」：上次那份没了就退到第一份；一份都没有就什么都不开。 —— */
  useEffect(() => {
    if (!ready || !node || !listed) return;
    if (cur && live.some((f) => f.name === cur)) return;
    const last = readLastDoc(board, id);
    const next = live.find((f) => f.name === last) ?? live[0];
    setCur(next ? next.name : "");
  }, [ready, node, listed, live, cur, board, id]);

  /* —— 搜：名字和正文都问服务端（正文不上网，只回来命中那一行） —— */
  useEffect(() => {
    const needle = q.trim();
    if (!needle) {
      setFound(null);
      return;
    }
    let alive = true;
    // 每敲一个字都问一次太吵：压 140ms，比一格键盘的重复还快。
    const t = window.setTimeout(() => {
      void listDocs(board, id, needle).then((out) => {
        if (!alive || !out) return;
        setFound(out.files.filter((f) => !f.trashed));
      });
    }, 140);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [q, board, id, files]);

  const startAdd = useCallback(() => {
    setRenaming(null);
    setAdding(true);
    setQ("");
    setSide(true);
  }, []);

  const startRename = useCallback((name: string) => {
    setAdding(false);
    setRenaming(name);
    setQ("");
    setSide(true);
  }, []);

  /* —— Esc 从最上面那层往下关。⌘K 展开左列去搜索，⌘S 立刻写。 —— */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === "Escape") {
        // 右键菜单开着的时候 Esc 归它：那张菜单 portal 在 body 上（它得能浮在块上面），
        // 拿不到上面那行 state，只好在 DOM 里问一句。
        if (document.querySelector(".nm-menu")) return;
        e.preventDefault();
        e.stopPropagation();
        if (confirm) return setConfirm(null);
        if (adding || renaming) return closeInputs();
        if (narrow && side) return setSide(false);
        onClose();
        return;
      }
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSide(true);
        // 抽屉 / 输入框都在这一帧之后才落位，等它们摆好再抢焦点。
        requestAnimationFrame(() => {
          search.current?.focus();
          search.current?.select();
        });
        return;
      }
      if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        doc.saveNow();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [confirm, adding, renaming, narrow, side, onClose, doc, closeInputs]);

  /** 换一份：窄屏把左列收回去（那儿它是盖在正文上的），搜索清掉、输入框关掉。 */
  const open = (name: string) => {
    setCur(name);
    setQ("");
    closeInputs();
    if (narrow) setSide(false);
  };

  const newDoc = async (raw: string) => {
    const typed = normalizeDocName(raw);
    if (!typed) return setNote("名字空了 —— 至少给一个字");
    // 入口名大小写归一：`agents.md` 也建到 `AGENTS.md` 上，别在盘上留下两份。
    const name = typed.toLowerCase() === ENTRY.toLowerCase() ? ENTRY : typed;
    if (files.some((f) => f.name.toLowerCase() === name.toLowerCase())) {
      return setNote(`${name} 已经在了，换一个`);
    }
    try {
      const out = await createDoc(board, id, name);
      await refresh();
      closeInputs();
      setCur(out.name);
      if (narrow) setSide(false);
      setNote(`${out.name} 建好了 —— 直接写`);
    } catch (e) {
      setNote(
        e instanceof DocEditError && e.reason === "exists"
          ? `${name} 已经在了`
          : "没建成，名字可能不合规矩",
      );
    }
  };

  const doTrash = async (name: string) => {
    setConfirm(null);
    try {
      await trashDoc(board, id, name);
      const list = await refresh();
      setNote(`${name} 已移到最近删除 —— 左列底下能恢复`);
      if (cur === name) {
        const next = list?.find((f) => !f.trashed);
        setCur(next?.name ?? "");
      }
      if (list?.every((f) => f.trashed)) setTrashOpen(true);
    } catch (e) {
      setNote(
        e instanceof DocEditError && e.reason === "protected" ? "AGENTS.md 只能改，删不掉" : "没删成",
      );
    }
  };

  /** 标题就地改：回车 / 失焦提交。Enter 之后框会消失，别让随后的 blur 再提交一次。 */
  const commitTitle = (raw: string) => {
    if (renamed.current) return;
    renamed.current = true;
    setTitleEdit(false);
    const to = normalizeDocName(raw);
    if (to && to !== cur) void doRename(cur, raw);
    window.setTimeout(() => {
      renamed.current = false;
    }, 0);
  };

  /**
   * 改名。走的是「同一个文件夹里的文件改名」，正文一个字节都不重写 ——
   * 所以 etag 不变，另一个标签页开着它也不会被顶成 409。
   */
  const doRename = async (from: string, raw: string) => {
    const to = normalizeDocName(raw);
    if (!to) return setNote("名字空了 —— 至少给一个字");
    if (to === from) return closeInputs();
    if (files.some((f) => f.name.toLowerCase() === to)) return setNote(`${to} 已经在了，换一个`);
    try {
      const out = await renameDoc(board, id, from, to);
      await refresh();
      if (cur === from) setCur(out.name);
      closeInputs();
      setNote(`${from} → ${out.name}`);
    } catch (e) {
      const reason = e instanceof DocEditError ? e.reason : "other";
      setNote(
        reason === "exists"
          ? `${to} 已经在了`
          : reason === "protected"
            ? "AGENTS.md 是 agent 的入口，改不了名"
            : reason === "missing"
              ? "那份文件不在了"
              : "没改成，名字可能不合规矩",
      );
    }
  };

  const doRestore = async (name: string) => {
    try {
      const out = await restoreDoc(board, id, name);
      await refresh();
      setTrashOpen(false);
      open(out.name);
      setNote(`${out.name} 回来了`);
    } catch (e) {
      setNote(
        e instanceof DocEditError && e.reason === "exists" ? "同名的那份还在，先删它" : "没恢复成",
      );
    }
  };

  /**
   * 右键菜单里的条目。鼠标端的一切入口都在这里 —— 行上没有悬停才浮出来的小图标，
   * 一张菜单就够：打开 / 重命名 / 复制名字 / 移到最近删除。
   *
   * 入口那份把「重命名」「移到最近删除」摆出来但按不动（`disabled`）：
   * 「没这功能」和「这份文件不给用」是两件事，界面得说清是哪一件。
   */
  const menuFor = (docId: string): MenuItem[] => {
    if (docId === NEW_ID) {
      return [{ key: "new", label: "新建文档", icon: <IconPage />, run: startAdd }];
    }
    const file = files.find((f) => f.name === docId);
    if (!file) return [];
    if (file.trashed) {
      return [
        {
          key: "restore",
          label: "恢复",
          icon: <IconRestore />,
          run: () => void doRestore(file.name),
        },
      ];
    }
    return [
      ...(file.name === cur
        ? []
        : [{ key: "open", label: "打开", icon: <IconPage />, run: () => open(file.name) }]),
      {
        key: "rename",
        // 入口那份把动词换成理由：按不动的按钮得说清是谁不让按，别让人猜。
        label: file.protected ? "入口文件：不能改名" : "重命名",
        icon: <IconPencil />,
        separatorBefore: true,
        disabled: file.protected,
        run: () => startRename(file.name),
      },
      {
        key: "copy",
        label: "复制文件名",
        icon: <IconCopy />,
        run: () => void copyText(file.name),
      },
      {
        key: "trash",
        label: file.protected ? "入口文件：删不掉" : "移到最近删除",
        icon: <IconTrash />,
        danger: true,
        separatorBefore: true,
        disabled: file.protected,
        run: () => setConfirm(file.name),
      },
    ];
  };

  if (!ready) {
    return (
      <Shell onClose={onClose} title={id}>
        {null}
      </Shell>
    );
  }

  if (!node) {
    return (
      <Shell onClose={onClose} title={id}>
        <p className="text-[13px] leading-relaxed text-muted">
          树里没有这个节点。可能已经被删了 —— 它的文件夹还躺在 map/.gone/ 里，撤销回来就接得上。
        </p>
      </Shell>
    );
  }

  const title = cur.replace(/\.md$/i, "");
  const rows = found ?? live;
  const searching = q.trim().length > 0;
  const empty = listed && live.length === 0 && !searching;

  return (
    <NodeMenuProvider attr="data-doc-id" items={(docId) => menuFor(docId)}>
      <div
        role="dialog"
        aria-label={`${node.name} 的文档`}
        className="absolute inset-0 z-40 flex flex-col bg-canvas text-text"
      >
        {/* 一条头。宽屏就是盘面那条：返回 / 左列开关 / 位置 / ＋ 文档。
            手机上它矮一半、半透明、盖在正文上 —— 正文从它底下滚过去。上面只留三样：
            回去、这是哪份、（看一眼 / 新建）。左列开关在手机上没了：标题自己就是那颗开关。 */}
        <header
          ref={bar}
          data-narrow={narrow}
          data-scrolled={scrolled}
          className="dp-bar flex shrink-0 items-center gap-1.5 px-3"
        >
          <IconBtn label="返回盘面（Esc）" onClick={onClose}>
            <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
              <path
                d="M9.5 3.5 5 8l4.5 4.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </IconBtn>

          {/* 左列的开关。**和盘面那颗一模一样**：一个框，中间一条竖线 —— 说的是「这一列」，
              不是一个抽象的箭头。开的时候字色亮着，关着的时候是暗的。
              手机上它也在，因为这个列在手机上真的存在（只是盖在纸上了）。 */}
          <IconBtn
            label={side ? "收起左列" : "展开左列"}
            onClick={() => setSide(!side)}
            className={side ? "text-text/80" : "text-muted"}
          >
            <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
              <rect
                x="1.5"
                y="2.5"
                width="13"
                height="11"
                rx="2.5"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.2"
              />
              <path d="M6 2.5v11" stroke="currentColor" strokeWidth="1.2" />
            </svg>
          </IconBtn>

          {narrow ? (
            /* 大标题：手机上中间就是一块空，等纸上的名字滑到条子底下，它才浮上来。
               不早一步（明明下面刚写过一遍）、也不晚一步（那时候名字已经在屏外了）。 */
            <span data-past={past} className="dp-bar-name" aria-hidden={!past}>
              {cur.replace(/\.md$/i, "") || node.name}
            </span>
          ) : (
            /* 位置：块名 / 当前这一份。都是字，不是按钮 —— 想回去就按左上角那颗。 */
            <div className="flex min-w-0 items-baseline gap-1.5 px-1 text-[13px]">
              <span className="truncate font-medium text-text/90" title={tree}>
                {node.name}
              </span>
              <span className="text-text/25" aria-hidden="true">
                /
              </span>
              <span className="truncate font-mono text-[11.5px] text-muted">{cur || "文档"}</span>
              {current?.protected && <span className="dp-pill">入口</span>}
            </div>
          )}

          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            {/* 有正文可看才有「怎么看」这回事。空文件夹、载入中不摆一个管不了任何东西的开关。
                宽屏是那颗两格滑块；手机上它收成一颗图标 —— 标题要地方，而手机上只有三格。 */}
            {doc.phase === "ready" &&
              (narrow ? (
                <IconBtn
                  label={view === "preview" ? "切回源码" : "预览（渲染出来）"}
                  onClick={() => pickView(view === "preview" ? "source" : "preview")}
                  className={view === "preview" ? "text-text/80" : "text-muted"}
                >
                  {view === "preview" ? <IconCode /> : <IconEye />}
                </IconBtn>
              ) : (
                <ViewSwitch view={view} onChange={pickView} />
              ))}

            {narrow ? (
              <IconBtn label="新建文档" onClick={startAdd}>
                <IconPlus />
              </IconBtn>
            ) : (
              <Pill onClick={startAdd} title="在这个文件夹里新建一份文档">
                ＋ 文档
              </Pill>
            )}
          </div>
        </header>

        <div className="relative flex min-h-0 flex-1">
          {/* ── 左列：就是这个文件夹 ─────────────────────────────────── */}
          <aside
            ref={sheet.box as React.RefObject<HTMLElement>}
            {...(narrow ? sheet.handlers : {})}
            className={
              narrow
                ? `dp-sheet absolute left-0 z-20 ${side ? "dp-sheet--open" : ""}`
                : `relative shrink-0 overflow-hidden border-r border-border bg-surface transition-[width] duration-300 ${EASE} ${
                    side ? "w-[248px]" : "w-0 border-r-0"
                  }`
            }
            aria-label="文档列表"
            inert={!side || undefined}
          >
            <div
              className={`flex h-full flex-col ${narrow ? "w-full" : "w-[248px]"} ${side ? "opacity-100" : "opacity-0"} transition-opacity duration-200`}
            >
              <div className="dp-side-search mt-2.5">
                <svg
                  viewBox="0 0 14 14"
                  className="dp-side-search-icon"
                  aria-hidden="true"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.5"
                  strokeLinecap="round"
                >
                  <circle cx="6.3" cy="6.3" r="3.5" />
                  <path d="M9 9 11.6 11.6" />
                </svg>
                <input
                  ref={search}
                  value={q}
                  onChange={(e) => setQ(e.target.value)}
                  placeholder="筛选 / 搜正文"
                  aria-label="筛选或搜正文"
                  autoCapitalize="off"
                  autoCorrect="off"
                  spellCheck={false}
                  className="dp-side-search-input"
                  onKeyDown={(e) => {
                    // 回车就开第一行：搜完不用把手从键盘上拿开去摸鼠标。
                    if (e.key === "Enter" && rows[0]) {
                      e.preventDefault();
                      open(rows[0].name);
                    }
                  }}
                />
                {searching ? (
                  <button
                    type="button"
                    onClick={() => {
                      setQ("");
                      setFound(null);
                    }}
                    aria-label="清空搜索"
                    className="dp-side-search-x"
                  >
                    <svg
                      viewBox="0 0 14 14"
                      className="h-3 w-3"
                      aria-hidden="true"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.6"
                      strokeLinecap="round"
                    >
                      <path d="M3.8 3.8l6.4 6.4M10.2 3.8l-6.4 6.4" />
                    </svg>
                  </button>
                ) : (
                  !narrow && <kbd className="dp-side-kbd">⌘K</kbd>
                )}
              </div>

              {/* 空白处右键 = 新建。整块清单带一个 `data-doc-id`，行在它里面 ——
                  `closest` 先找到行，所以行上是行的菜单，空白处才是新建。 */}
              <div className="dp-side-list" data-doc-id={NEW_ID}>
                {adding && (
                  <div className="dp-line">
                    <input
                      autoFocus
                      placeholder="新文件的名字，回车确认"
                      aria-label="新文档名"
                      autoCapitalize="off"
                      autoCorrect="off"
                      spellCheck={false}
                      className="dp-input"
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          void newDoc(e.currentTarget.value);
                        }
                      }}
                      onBlur={closeInputs}
                    />
                  </div>
                )}

                {rows.map((f) =>
                  renaming === f.name ? (
                    <div className="dp-line" key={f.name}>
                      <input
                        autoFocus
                        defaultValue={splitName(f.name)[0]}
                        aria-label="新文档名"
                        autoCapitalize="off"
                        autoCorrect="off"
                        spellCheck={false}
                        className="dp-input"
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.preventDefault();
                            void doRename(f.name, e.currentTarget.value);
                          }
                        }}
                        onBlur={closeInputs}
                      />
                    </div>
                  ) : (
                    <SideDoc key={f.name} file={f} on={f.name === cur} onPick={() => open(f.name)} />
                  ),
                )}

                {searching && rows.length === 0 && (
                  <p className="dp-side-hint">搜了 {live.length} 份，没有命中「{q.trim()}」。</p>
                )}

                {empty && (
                  <p className="dp-side-hint">
                    这个文件夹还是空的 —— 一份文档都没有。右键这里，或者按上面的「＋ 文档」。
                  </p>
                )}

                {!listed && !searching && <p className="dp-side-hint">载入中…</p>}
              </div>

              {gone.length > 0 && (
                <div className="dp-side-foot">
                  <button
                    type="button"
                    onClick={() => setTrashOpen((o) => !o)}
                    aria-expanded={trashOpen}
                    className="dp-trash-toggle"
                  >
                    <span className="min-w-0 flex-1 truncate text-left">最近删除</span>
                    <span className="tabular-nums">{gone.length}</span>
                    <svg
                      viewBox="0 0 14 14"
                      className={`h-3 w-3 shrink-0 transition-transform duration-150 ${trashOpen ? "rotate-90" : ""}`}
                      aria-hidden="true"
                      fill="none"
                      stroke="currentColor"
                      strokeWidth="1.6"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    >
                      <path d="M5.6 3.4 9.2 7l-3.6 3.6" />
                    </svg>
                  </button>
                  {trashOpen &&
                    gone.map((t) => (
                      <SideDoc
                        key={t.name}
                        file={t}
                        on={false}
                        onPick={() => void doRestore(t.name)}
                        gone
                      />
                    ))}
                </div>
              )}
            </div>
          </aside>

          {narrow && (
            <button
              ref={sheet.scrim as React.RefObject<HTMLButtonElement>}
              type="button"
              aria-label="关闭清单"
              tabIndex={-1}
              onClick={() => setSide(false)}
              data-open={side}
              className="dp-scrim absolute inset-0 z-10"
            />
          )}

          {/* ── 右边：当前这一份 ───────────────────────────────────── */}
          <div className="relative flex min-w-0 flex-1 flex-col">
            <div
              ref={scroll}
              className="dp-scroll"
              onScroll={(e) => {
                const s = e.currentTarget;
                setScrolled(s.scrollTop > 2);
                // 纸上的名字跑到条子底下去了没有：条子的高度是量的，不是写死的（上面还有安全区）。
                const head = pageTitle.current;
                const barH = bar.current?.offsetHeight ?? 48;
                setPast(!!head && head.getBoundingClientRect().top < barH);
              }}
            >
              <div className="dp-column">
                {!cur && !listed && <p className="pt-1 text-[13px] text-muted/70">载入中…</p>}

                {!cur && empty && (
                  <div className="dp-empty">
                    <p>
                      {gone.length > 0
                        ? "正文档都进最近删除了 —— 在左列底下恢复，或者重新写一份。"
                        : "这个文件夹还是空的 —— 一份文档都没有。想有，就从第一句开始。"}
                    </p>
                    <Pill onClick={startAdd}>新建一份</Pill>
                  </div>
                )}

                {!cur && listed && !empty && (
                  <p className="pt-1 text-[13px] text-muted/70">从左列挑一份。</p>
                )}

                {doc.phase === "missing" && (
                  <div className="dp-empty">
                    <p>
                      <span className="font-mono text-text">{cur}</span> 不在磁盘上了 ——
                      可能刚被别处删掉。
                    </p>
                    {live.length > 0 && <Pill onClick={() => open(live[0].name)}>打开 {live[0].name}</Pill>}
                  </div>
                )}

                {doc.phase === "error" && (
                  <p className="pt-1 text-[13px] text-muted">
                    读不到这份文件（<span className="text-danger">{doc.msg}</span>）。
                  </p>
                )}

                {doc.phase === "ready" && (
                  <>
                    <p className="dp-eyebrow" title={`content/${board}/map/${id}/${cur}`}>
                      map/{id}/{cur}
                    </p>
                    <div className="dp-title-row" ref={pageTitle}>
                      {/* 标题就是名字。**点一下就地改**，不用绕到菜单里去 —— 名字在哪儿，就在哪儿改。
                          和 Finder / Notes 一个手感：回车或失焦交，Esc 退。AGENTS.md 不能改名，
                          那就不是一颗按钮（不装一个假的可点击）。 */}
                      {titleEdit ? (
                        <input
                          key={cur}
                          autoFocus
                          defaultValue={title}
                          aria-label="文档名"
                          autoCapitalize="off"
                          autoCorrect="off"
                          spellCheck={false}
                          className="dp-title dp-title-input min-w-0"
                          onKeyDown={(e) => {
                            if (e.key === "Enter") {
                              e.preventDefault();
                              commitTitle(e.currentTarget.value);
                            }
                            if (e.key === "Escape") {
                              e.preventDefault();
                              e.stopPropagation();
                              skipBlur.current = true;
                              setTitleEdit(false);
                            }
                          }}
                          onBlur={(e) => {
                            if (skipBlur.current) {
                              skipBlur.current = false;
                              return;
                            }
                            commitTitle(e.currentTarget.value);
                          }}
                        />
                      ) : current?.protected ? (
                        <h1 className="dp-title min-w-0 break-words">{title}</h1>
                      ) : (
                        <button
                          type="button"
                          onClick={() => setTitleEdit(true)}
                          title="点一下改名字"
                          className="dp-title-btn dp-title min-w-0"
                        >
                          <span className="break-words">{title}</span>
                          <svg
                            viewBox="0 0 14 14"
                            className="dp-title-pencil h-4 w-4 shrink-0"
                            aria-hidden="true"
                            fill="none"
                            stroke="currentColor"
                            strokeWidth="1.4"
                            strokeLinecap="round"
                            strokeLinejoin="round"
                          >
                            <path d="M9.4 2.6 11.4 4.6 5.1 10.9 2.6 11.4 3.1 8.9z" />
                            <path d="M8.4 3.6 10.4 5.6" />
                          </svg>
                        </button>
                      )}
                      {current?.protected && <span className="dp-pill">agent 入口</span>}
                    </div>

                    {/* 一行小字：多大、存没存下、它是不是自己跑起来了。都在这一行，不占第二行正文。 */}
                    <div className="dp-meta">
                      <span className="dp-stat">
                        {words(doc.text)} 字 · {lines(doc.text)} 行
                      </span>
                      {STATUS_TEXT[doc.status] && (
                        <>
                          <span className="dp-dot" aria-hidden="true" />
                          <span
                            key={doc.status}
                            className="dp-status"
                            data-s={doc.status}
                            aria-live="polite"
                          >
                            {STATUS_TEXT[doc.status]}
                          </span>
                        </>
                      )}
                      {hasApp && (
                        <a
                          className="dp-link"
                          href={`/p/${encodeURIComponent(board)}/${encodeURIComponent(id)}/`}
                          target="_blank"
                          rel="noopener"
                          title="这一块在自己跑（文件夹里有 scripts.start）—— 在新标签页打开"
                        >
                          打开它的页面 ›
                        </a>
                      )}
                    </div>

                    {doc.theirs && (
                      <div className="dp-glass mb-3 flex flex-wrap items-center gap-2 rounded-xl px-3 py-2 text-[12px]">
                        <span className="min-w-0 flex-1 text-muted">
                          磁盘上这份被别处改过了。两边都还在，你选。
                        </span>
                        <Banner onClick={doc.takeTheirs}>用磁盘的</Banner>
                        <Banner onClick={doc.keepMine} muted>
                          用我的覆盖
                        </Banner>
                      </div>
                    )}

                    {doc.orphan && (
                      <div className="dp-glass mb-3 flex flex-wrap items-center gap-2 rounded-xl px-3 py-2 text-[12px]">
                        <span className="min-w-0 flex-1 text-muted">
                          本地有一份没发出去的草稿，但它基于的是旧版本。两边都还在，你选。
                        </span>
                        <Banner onClick={doc.useOrphan}>用草稿的</Banner>
                        <Banner onClick={doc.dropOrphan} muted>
                          丢掉草稿
                        </Banner>
                      </div>
                    )}

                    {doc.status === "error" && (
                      <p className="mb-3 text-[11.5px] text-danger">
                        没连上。字已经存成本地草稿了，下次进来先看它。
                      </p>
                    )}

                    {/* 同一条 `doc.text`，两种看法。预览看的就是「你会存下去的那一版」——
                        没存的字也已经在里面了，不用先保存再看。 */}
                    {view === "preview" ? (
                      doc.text.trim() ? (
                        <Suspense fallback={<p className="dp-prose-quiet">渲染中…</p>}>
                          <DocMarkdown text={doc.text} />
                        </Suspense>
                      ) : (
                        <p className="dp-prose-quiet">
                          这份还是空的 —— 切回源码写点东西，这里就出东西。
                        </p>
                      )
                    ) : (
                      <textarea
                        ref={body}
                        value={doc.text}
                        onChange={(e) => doc.edit(e.target.value)}
                        autoFocus
                        spellCheck={false}
                        aria-label={`${cur} 正文`}
                        placeholder="写点什么。改了就自己存。"
                        className="dp-text w-full flex-1 resize-none bg-transparent outline-none"
                      />
                    )}
                  </>
                )}
              </div>
            </div>

            {/* 回执：一颗浮起来的胶囊，3.4 秒后自己走。挂在正文这一格上，所以它是「纸」的中心，
                不是「整个窗口」的中心 —— 左边还剩一列文件呢。 */}
            {note && (
              <div className="dp-glass dp-toast" role="status">
                {note}
              </div>
            )}
          </div>

          {confirm && (
            <Confirm
              name={confirm}
              onCancel={() => setConfirm(null)}
              onYes={() => void doTrash(confirm)}
            />
          )}
        </div>
      </div>
    </NodeMenuProvider>
  );
}

/* ------------------------------------------------------------------ *
 * 列表里的一行：一个名字，右键归菜单管。
 *
 * **鼠标端行上没有任何按钮** —— 悬停才浮出来的小图标是「求你别用」的按钮，还会跟右键抢位置。
 * 触屏没有右键，所以那颗 ••• 只在 `(hover: none)` 的设备上才存在（`NodeMenuTrigger` 自己判断）。
 * ------------------------------------------------------------------ */
function SideDoc({
  file,
  on,
  onPick,
  gone,
}: {
  file: DocFile;
  on: boolean;
  onPick: () => void;
  /** 最近删除里那一行：名字划掉，左键点它就恢复。 */
  gone?: boolean;
}) {
  const [stem, ext] = splitName(file.name);
  return (
    <div className="dp-row nm-host nm-host--row" data-on={on} data-doc-id={file.name}>
      <button type="button" onClick={onPick} className="dp-row-pick" title={file.name}>
        <span className={`dp-row-stem ${gone ? "dp-row-stem--gone" : ""}`}>{stem}</span>
        <span className="dp-row-ext">{ext}</span>
        {file.protected && <span className="dp-row-tag">入口</span>}
      </button>
      <NodeMenuTrigger nodeId={file.name} kind="row" />
      {file.snippet && <p className="dp-row-snip">{file.snippet}</p>}
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * 删除确认。destructive 在左、默认焦点在「取消」—— 手比眼睛快的时候，
 * 落在回车上的那一颗得是安全的那个。
 * ------------------------------------------------------------------ */
function Confirm({
  name,
  onCancel,
  onYes,
}: {
  name: string;
  onCancel: () => void;
  onYes: () => void;
}) {
  const no = useRef<HTMLButtonElement | null>(null);

  useEffect(() => {
    no.current?.focus();
  }, []);

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center p-5" role="presentation">
      <div className="absolute inset-0 bg-black/30" onPointerDown={onCancel} aria-hidden="true" />
      <div
        className="dp-glass dp-pop w-[272px] overflow-hidden rounded-[20px]"
        role="alertdialog"
        aria-label="移到最近删除"
        aria-describedby="dp-confirm-body"
      >
        <p className="px-4 pb-1 pt-4 text-center text-[13.5px] font-semibold tracking-[-0.01em]">
          移到最近删除？
        </p>
        <p
          id="dp-confirm-body"
          className="px-4 pb-4 pt-1 text-center text-[11.5px] leading-relaxed text-muted"
        >
          <b className="font-mono font-medium text-text">{name}</b> 会挪进{" "}
          <b className="font-mono font-medium text-text">map/&lt;id&gt;/.trash/</b>，
          <br />
          没有真的删掉 —— 左列的「最近删除」里能恢复。
        </p>
        <div className="flex border-t border-border">
          <button
            type="button"
            onClick={onYes}
            className="min-h-[46px] flex-1 text-[13px] font-medium text-danger transition-[background,scale] duration-100 hover:bg-danger/10 active:scale-[0.985] sm:min-h-[42px]"
          >
            移到最近删除
          </button>
          <button
            ref={no}
            type="button"
            onClick={onCancel}
            className="min-h-[46px] flex-1 border-l border-border text-[13px] text-accent transition-[background,scale] duration-100 hover:bg-surface active:scale-[0.985] focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent sm:min-h-[42px]"
          >
            取消
          </button>
        </div>
      </div>
    </div>
  );
}

function Banner({
  children,
  onClick,
  muted,
}: {
  children: React.ReactNode;
  onClick: () => void;
  muted?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`h-7 shrink-0 rounded-lg px-2.5 text-[12px] transition-colors duration-150 ${
        muted ? "text-muted hover:bg-raised hover:text-text" : "bg-surface text-text/90 hover:bg-raised"
      }`}
    >
      {children}
    </button>
  );
}

/** 头里那颗图标按钮 —— 和盘面那颗同一个尺寸、同一种悬停。 */
function IconBtn({
  children,
  onClick,
  label,
  className = "",
}: {
  children: React.ReactNode;
  onClick: () => void;
  label: string;
  className?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-lg transition-colors duration-150 hover:bg-surface hover:text-text ${className}`}
    >
      {children}
    </button>
  );
}

/** 头里那颗胶囊 —— 和盘面的「＋ 块」「＋ 组」同一副样子。 */
/* ------------------------------------------------------------------ *
 * 正文怎么看：源码 / 预览。
 *
 * 一颗两格的开关，滑块是一块**真的矩形**（不是两套底色换着上）—— 所以切的那一下可以
 * 只动 `transform`，整个过渡只有一个合成层。开着的固定在左边：默认是源码，那一格不该会跑。
 *
 * 没有键盘快捷键：⌘E、⌘⇧P 在浏览器里各自有主，抢过来比省一下鼠标更贵。
 * ------------------------------------------------------------------ */
function ViewSwitch({ view, onChange }: { view: View; onChange: (v: View) => void }) {
  return (
    <div className="dp-view" role="group" aria-label="正文显示方式">
      <span className="dp-view-thumb" data-i={view === "preview" ? 1 : 0} aria-hidden="true" />
      <button
        type="button"
        aria-pressed={view === "source"}
        onClick={() => onChange("source")}
        title="源码模式：直接改 markdown"
        className="dp-view-btn"
      >
        源码
      </button>
      <button
        type="button"
        aria-pressed={view === "preview"}
        onClick={() => onChange("preview")}
        title="预览模式：渲染出来读，像 GitHub 上看一份 .md"
        className="dp-view-btn"
      >
        预览
      </button>
    </div>
  );
}

function Pill({
  children,
  onClick,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      className="h-7 shrink-0 rounded-lg bg-surface px-2.5 text-[12px] text-text/90 transition-[background,scale] duration-150 hover:bg-raised active:scale-[0.97]"
    >
      {children}
    </button>
  );
}

/* 菜单里那几枚图标：一张纸、一支笔、一只垃圾桶、一个回头的箭头、两张叠起来的纸。 */
const stroke = {
  viewBox: "0 0 14 14",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.4,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  "aria-hidden": true,
} as const;

const IconPage = () => (
  <svg {...stroke}>
    <path d="M3.2 1.8h5.2l2.4 2.4v8H3.2z" />
    <path d="M8.4 1.8v2.5h2.4" />
  </svg>
);

const IconPencil = () => (
  <svg {...stroke}>
    <path d="M9.2 2.4 11.6 4.8 5.4 11H3V8.6z" />
  </svg>
);

const IconTrash = () => (
  <svg {...stroke}>
    <path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4" />
    <path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4" />
  </svg>
);

const IconRestore = () => (
  <svg {...stroke}>
    <path d="M3.2 7a3.9 3.9 0 1 1 1.3 2.9" />
    <path d="M3.2 3.6V7h3.4" />
  </svg>
);

const IconCopy = () => (
  <svg {...stroke}>
    <rect x="2.5" y="2.5" width="7" height="7" rx="1.6" />
    <path d="M5.5 11.5h4.4a1.6 1.6 0 0 0 1.6-1.6V5.5" />
  </svg>
);

/** 载入中 / 查无此块时的那一层空壳 —— 和真页面同一个形（一条头 + 版心），不会闪。 */
/* 手机工具条上的三枚：一只眼（看渲染）、两个尖括号（回去改）、一个加号。 */
const IconEye = () => (
  <svg {...stroke}>
    <path d="M1.5 7s2.2-3.9 5.5-3.9S12.5 7 12.5 7s-2.2 3.9-5.5 3.9S1.5 7 1.5 7Z" />
    <circle cx="7" cy="7" r="1.6" />
  </svg>
);

const IconCode = () => (
  <svg {...stroke}>
    <path d="M4.9 4.2 2.2 7l2.7 2.8" />
    <path d="M9.1 4.2 11.8 7l-2.7 2.8" />
  </svg>
);

const IconPlus = () => (
  <svg {...stroke}>
    <path d="M7 2.8v8.4M2.8 7h8.4" />
  </svg>
);

function Shell({
  onClose,
  title,
  children,
}: {
  onClose: () => void;
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="absolute inset-0 z-40 flex flex-col bg-canvas text-text">
      <header className="flex h-[52px] shrink-0 items-center gap-1.5 border-b border-border px-3">
        <IconBtn label="返回盘面" onClick={onClose}>
          <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
            <path
              d="M9.5 3.5 5 8l4.5 4.5"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </IconBtn>
        <span className="truncate px-1 font-mono text-[12px] text-muted">{title}</span>
      </header>
      <div className="dp-scroll">
        <div className="dp-column">{children}</div>
      </div>
    </div>
  );
}
