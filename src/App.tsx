import { useEffect, useMemo, useRef, useState } from "react";
import { DocPage } from "./Doc";
import { Find, type Hit } from "./Find";
import { MoveTo } from "./Move";
import { NameInput } from "./Rename";
import { NodeMenuProvider, NodeMenuTrigger, type MenuItem } from "./NodeMenu";
import { Tree } from "./Tree";
import { copyText } from "./clipboard";
import {
  type Box,
  type Node,
  addLeaf,
  addSection,
  childIds,
  findNode,
  treeMap,
  initialState,
  isSection,
  layout,
  moveNode,
  pathTo,
  removeNode,
  renameNode,
  valueOf,
} from "./game";
import {
  type BoardSummary,
  type Draft,
  type Move,
  ConflictError,
  clearDraft,
  createBoard,
  listBoards,
  loadBoard,
  loadLocks,
  readDraft,
  saveBoard,
  setLock,
  writeDraft,
} from "./store";

const EASE = "ease-[cubic-bezier(0.32,0.72,0,1)]";
/** 左栏默认是收起的。只有你手动展开过才记一个 1 —— 没记录就是关着。 */
const SIDE_KEY = "nous-sidebar-open";
/** 上次开着的是哪个项目。这个存在浏览器里无所谓 —— 丢了只是回到默认那一盘。 */
const PROJECT_KEY = "nous-project";

/**
 * 你在哪一层：`content/<id>` 里的项目文件不该记这个 ——
 * 那是「我刚才看到哪」，属于这台设备和这次会话；换个设备未必要看同一层。
 */
const pathKey = (id: string): string => `nous-path:${id}`;

/**
 * 块的页面是**真实路径**，不是 hash：`/p/<board>/<nodeId>/`。
 *
 * hash 得等 SPA 先跑起来才认，而这一页要能被冷启动、粘到别处、被将来那个
 * 独立进程自己 serve —— 那就必须是一条真路径。
 *
 * **只在 `__nous/` 下面才有壳的页面。** `/p/<board>/<id>/` 往下全归那个块的进程 ——
 * 没开发的块就是一张空白页，壳不去替它画。这段正则因此只认笔记页那一条。
 */
const DOC_URL = /^\/p\/([^/]+)\/([^/]+)\/__nous\/doc\/?$/;

function readLocation(): { board?: string; doc?: string } {
  const hit = DOC_URL.exec(window.location.pathname);
  if (!hit) return {};
  try {
    return { board: decodeURIComponent(hit[1]!), doc: decodeURIComponent(hit[2]!) };
  } catch {
    return {};
  }
}

function readPath(id: string): string[] {
  try {
    const raw = localStorage.getItem(pathKey(id));
    const parsed: unknown = raw ? JSON.parse(raw) : null;
    return Array.isArray(parsed) ? parsed.filter((v): v is string => typeof v === "string") : [];
  } catch {
    return [];
  }
}

function writePath(id: string, path: string[]): void {
  try {
    localStorage.setItem(pathKey(id), JSON.stringify(path));
  } catch {
    // 写不了就算了，下次从根开始，不是大事。
  }
}

/** 存下来的路径里可能包含已经被删掉的节点；能进去的只有还在的组。 */
const alivePath = (root: Node, ids: string[]): string[] =>
  ids.filter((id) => {
    const n = findNode(root, id);
    return n !== undefined && isSection(n);
  });
/** 下拉里「新建项目」那一项的值。JSX 属性不认 \u 转义，别拿 NUL 当哨兵。 */
const NEW_BOARD = "__new__";

/** 落盘状态。显示在顶栏，因为「我这一手到底存没存」是这功能唯一的 UI。 */
type Status = "loading" | "saving" | "saved" | "conflict" | "error";

/** 一次写盘的完整快照：语义 + 历史。 */
type Snapshot = { root: Node; moves: Move[] };

const STATUS_TEXT: Record<Status, string> = {
  loading: "载入中",
  saving: "保存中",
  saved: "已保存",
  conflict: "冲突",
  error: "没连上",
};

/** pele 自己的 series 色板（`--pele-series-1..8`，深色主题那一套）。
 *  不用浅色主题的 `#4c78a8` 那组 —— 那是白底用的，放到 `#100f0f` 上全发灰。 */
const SERIES = [
  "#d14d41",
  "#da702c",
  "#d0a215",
  "#879a39",
  "#3aa99f",
  "#4385be",
  "#8b7ec8",
  "#ce5d97",
] as const;

/** pele treemap 的几何：块间 4px 缝（每边内缩 2）、子块左右各缩 6、底部缩 6、顶部让出标题带。 */
const GAP = 2;
const SIDE = 6;

const shrink = (b: Box, d: number): Box => ({
  x: b.x + d,
  y: b.y + d,
  w: Math.max(b.w - d * 2, 1),
  h: Math.max(b.h - d * 2, 1),
});

/** 标题带高度：够高才有，不然名字宁可不显示，也不要压着子块。 */
const hat = (b: Box, depth: number): number => {
  if (depth === 1) return b.h > 76 && b.w > 96 ? 24 : b.h > 40 && b.w > 60 ? 16 : 0;
  return b.h > 60 && b.w > 84 ? 18 : 0;
};

const tint = (hex: string, alpha: number): string => {
  const n = parseInt(hex.slice(1), 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
};

export function App() {
  const [root, setRoot] = useState<Node>(() => initialState());
  const [path, setPath] = useState<string[]>([]);
  /** 开着的是谁的页面。`null` = 在盘面上。和 `path` 是两条轴：一个是钻到哪层，一个是打开了谁的文档。 */
  const [openDoc, setOpenDoc] = useState<string | null>(() => readLocation().doc ?? null);

  /* —— 盘本身，存在 content/<id>/ 的三份 json 里 —— */
  const [projectId, setProjectId] = useState<string | null>(
    () => readLocation().board ?? localStorage.getItem(PROJECT_KEY),
  );
  const [boards, setBoards] = useState<BoardSummary[]>([]);
  const [moves, setMoves] = useState<Move[]>([]);
  const [status, setStatus] = useState<Status>("loading");
  /** 磁盘变了、本地还有没发出去的一手。两边都不自动丢，等人选。 */
  const [orphan, setOrphan] = useState<Draft | null>(null);
  /** 用 ref 存版本号：放 state 里会让保存 effect 自己触发自己，转成死循环。 */
  const version = useRef(0);
  /** 载入完成之前绝不许存 —— 否则拿一首示范盘把用户文件盖了。 */
  const ready = useRef(false);
  /** 刚载入的那一下不算「用户落了子」。 */
  const skipSave = useRef(false);
  /** 撞了版本就停手，等用户决定。继续自动存等于帮另一个标签页删数据。 */
  const blocked = useRef(false);
  const [epoch, setEpoch] = useState(0);
  /** 每一步落子前的一盘棋。撤销就是弹一个出来。 */
  const [past, setPast] = useState<Node[]>([]);
  /** 正在改名的那个节点，以及它是在哪儿被戳开的。
   *  一个节点同时在左列和盘面上画着 —— 不记来源，两边会各挂一个输入框，互相抢焦点。 */
  const [editing, setEditing] = useState<{ id: string; where: "tree" | "board" } | null>(null);
  /** ⌘K 跳过去之后闪一下。块平时不可点，闪一下是唯一告诉人「就是这块」的办法。 */
  const [flash, setFlash] = useState<string | null>(null);
  /** 锁着的块：删不掉，前端连按钮都不画。真拦截在服务端（save 会把被删的锁块接回来）。 */
  const [locks, setLocks] = useState<ReadonlySet<string>>(() => new Set());
  const flashTimer = useRef<number | undefined>(undefined);
  const [findOpen, setFindOpen] = useState(false);
  /** 「移动到…」捧着的是源 id + 触发它的那颗按钮的位置 —— 面板从那儿长出来，也从那儿缩回去。 */
  const [moving, setMoving] = useState<{ id: string; rect: DOMRect } | null>(null);
  const [side, setSide] = useState<boolean>(() => localStorage.getItem(SIDE_KEY) === "1");
  const [size, setSize] = useState({ w: 0, h: 0 });
  const [narrow, setNarrow] = useState(() => window.matchMedia("(max-width: 819px)").matches);
  const board = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = board.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) =>
      setSize({ w: e.contentRect.width, h: e.contentRect.height }),
    );
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  useEffect(() => {
    if (side) localStorage.setItem(SIDE_KEY, "1");
    else localStorage.removeItem(SIDE_KEY);
  }, [side]);

  /* —— 记住你在哪一层。刷新后回到刚才钻进去的那个组，不用重新点。 —— */
  useEffect(() => {
    // 载入完成前不能写：那时 path 还是初始的 []，会把上次存的层级冲掉。
    if (!ready.current || !projectId) return;
    writePath(projectId, path);
  }, [path, projectId]);

  /* —— 项目列表。挂载时扫一次 content/ 目录。 —— */
  useEffect(() => {
    let alive = true;
    void (async () => {
      try {
        const list = await listBoards();
        if (!alive) return;
        setBoards(list);
        if (!list.length) {
          setStatus("error");
          return;
        }
        if (!list.some((b) => b.id === projectId)) setProjectId(list[0]!.id);
      } catch {
        if (alive) setStatus("error");
      }
    })();
    return () => {
      alive = false;
    };
    // 新建项目走 createBoard 自己维护这份列表，这里不再扫。
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  /* —— 载入一盘。换项目、或撞了冲突手动重读，都走这里。 —— */
  useEffect(() => {
    if (!projectId) return;
    let alive = true;
    ready.current = false;
    blocked.current = false;
    setStatus("loading");
    void (async () => {
      try {
        const board = await loadBoard(projectId);
        if (!alive) return;
        // 新 id 是随机发的，跟载入的盘没有先后关系，所以这里没有「对齐」这一步。
        // 草稿之所以要先看：它上面那几手还没落盘，磁盘那份里没有它们。
        const draft = readDraft(projectId);
        const loaded = { root: board.root, moves: board.moves.slice(-200) };
        // 草稿内容跟磁盘一模一样：说明它只是一道影子（比如 pagehide 那一刀已经发出去了）。
        // 没“手”要救，别去打扰用户。
        const stale = draft && JSON.stringify(draft.root) === JSON.stringify(board.root);
        if (stale) clearDraft(projectId);

        // 草稿的底就是磁盘当前这一版：只是没发出去，直接补写，不麻烦用户。
        const adopt = draft && !stale && draft.baseVersion === board.version ? draft : null;
        // 底不一样：磁盘被别人改过。两边都留着，等人选。
        setOrphan(draft && !stale && !adopt ? draft : null);

        version.current = board.version;
        const next = adopt ? { root: adopt.root, moves: adopt.moves } : loaded;
        // 用磁盘那份时它就是磁盘内容，别写回去；用草稿时要存，所以不能跳。
        skipSave.current = !adopt;
        lastWritten.current = adopt ? "" : JSON.stringify(loaded);
        setRoot(next.root);
        setMoves(next.moves);
        setPast([]);
        setPath(alivePath(next.root, readPath(projectId)));
        localStorage.setItem(PROJECT_KEY, projectId);
        void loadLocks(projectId).then((ids) => {
          if (alive) setLocks(new Set(ids));
        });
        ready.current = true;
        setStatus(adopt ? "saving" : "saved");
      } catch {
        if (alive) setStatus("error");
      }
    })();
    return () => {
      alive = false;
    };
  }, [projectId, epoch]);

  /* —— 自动保存。 —— */
  /**
   * 写盘必须串行，而且只认最后一份快照。
   * 两个 PUT 同时在飞，旧的那个后到就会把新落的一手盖掉 —— 用户看到的就是「块凭空消失」。
   * `lastWritten` 是内容指纹：同一份盘不重复写，免得白拗版本号。
   */
  const pending = useRef<Snapshot | null>(null);
  const writing = useRef(false);
  const lastWritten = useRef("");

  const drain = async (id: string) => {
    if (writing.current) return;
    writing.current = true;
    try {
      while (pending.current && !blocked.current) {
        const snap = pending.current;
        pending.current = null;
        const key = JSON.stringify(snap);
        if (key === lastWritten.current) {
          // 磁盘上已经是这份了（上一轮写成功／刚载入的就是它）。草稿多余，清掉。
          clearDraft(id);
          continue;
        }
        try {
          const out = await saveBoard(id, snap.root, snap.moves, version.current);
          version.current = out.version;
          lastWritten.current = key;
          // 服务器接回了被删的锁定块：盘上的树和本地不一样了，认盘上那份。
          // 顺手把接回来的东西闪一下，人就知道它没丢。
          if (out.root) {
            lastWritten.current = JSON.stringify({ root: out.root, moves: snap.moves });
            setRoot(out.root);
            const first = out.restored[0];
            if (first) {
              setFlash(first);
              window.clearTimeout(flashTimer.current);
              flashTimer.current = window.setTimeout(() => setFlash(null), 1100);
            }
          }
          // 发出去了，草稿就没用了。
          clearDraft(id);
          setStatus("saved");
        } catch (err) {
          if (err instanceof ConflictError) {
            version.current = err.version;
            blocked.current = true;
            pending.current = null;
            setStatus("conflict");
          } else {
            // 网络抖一下：快照留着，下一次落子或 flush 再试。
            pending.current = snap;
            setStatus("error");
            break;
          }
        }
      }
    } finally {
      writing.current = false;
    }
  };

  useEffect(() => {
    if (!ready.current || !projectId || blocked.current) return;
    if (skipSave.current) {
      skipSave.current = false;
      return;
    }
    pending.current = { root, moves };
    // 同一步先记本地草稿：服务重启/断网时，快照至少还在磁盘上躺着。
    writeDraft(projectId, root, moves, version.current);
    setStatus("saving");
    const timer = setTimeout(() => void drain(projectId), 400);
    return () => clearTimeout(timer);
  }, [root, moves, projectId]);

  /* —— 关页面 / 切后台时补一刀：beforeunload 在移动端不可靠，pagehide 才是那个能信的。 —— */
  useEffect(() => {
    const flush = () => {
      if (!ready.current || !projectId || blocked.current) return;
      const snap = { root, moves };
      if (JSON.stringify(snap) === lastWritten.current) return;
      writeDraft(projectId, snap.root, snap.moves, version.current);
      void saveBoard(projectId, snap.root, snap.moves, version.current, true).catch(() => {});
    };
    const onHide = () => {
      if (document.hidden) flush();
    };
    window.addEventListener("pagehide", flush);
    document.addEventListener("visibilitychange", onHide);
    return () => {
      window.removeEventListener("pagehide", flush);
      document.removeEventListener("visibilitychange", onHide);
    };
  }, [root, moves, projectId]);

  useEffect(() => {
    const mq = window.matchMedia("(max-width: 819px)");
    const sync = () => setNarrow(mq.matches);
    mq.addEventListener("change", sync);
    return () => mq.removeEventListener("change", sync);
  }, []);

  const here = (path.length ? findNode(root, path[path.length - 1]!) : root) ?? root;

  /** 全树 tree。改名、搬家都会变，所以每次从树现算 —— 它不落盘，也不该落盘。 */
  const trees = useMemo(() => treeMap(root), [root]);
  /** ⌘K 的候选。盘面画不下的节点，这里是唯一看得到它们 tree 的地方。 */
  const hits = useMemo<Hit[]>(() => {
    const out: Hit[] = [];
    const walk = (n: Node) => {
      out.push({ node: n, tree: trees.get(n.id) ?? n.name });
      n.children?.forEach(walk);
    };
    walk(root);
    return out;
  }, [root, trees]);

  const goTo = (id: string) => {
    setFindOpen(false);
    setPath(pathTo(root, id));
    setFlash(id);
    window.clearTimeout(flashTimer.current);
    flashTimer.current = window.setTimeout(() => setFlash(null), 1100);
  };

  /**
   * 开一块的页面。**整页跳走**，不是盖一层：那一页是它自己的进程端出来的，
   * 壳管不着它长什么样（没开发的块就停在一张空白页上）。后退键回盘面。
   */
  const openBlock = (id: string) => {
    if (!projectId) return;
    window.location.assign(`/p/${encodeURIComponent(projectId)}/${encodeURIComponent(id)}/`);
  };

  /**
   * 开它的笔记本（`AGENTS.md`）。这个是壳的页面，所以留在 SPA 里、盖在盘面上、
   * 不卸载盘面 —— 否则树的展开状态、滚动位置、撤销栈全丢。
   */
  const openNotes = (id: string) => {
    if (!projectId) return;
    window.history.pushState(
      null,
      "",
      `/p/${encodeURIComponent(projectId)}/${encodeURIComponent(id)}/__nous/doc`,
    );
    setOpenDoc(id);
  };

  const closeDoc = () => {
    window.history.pushState(null, "", "/");
    setOpenDoc(null);
  };

  /** 后退 / 前进：URL 是唯一的真相，状态跟着它走。 */
  useEffect(() => {
    const sync = () => {
      const at = readLocation();
      setOpenDoc(at.doc ?? null);
      if (at.board) setProjectId((cur) => (cur === at.board ? cur : at.board!));
    };
    window.addEventListener("popstate", sync);
    return () => window.removeEventListener("popstate", sync);
  }, []);

  const play = (next: Node, move: Move) => {
    // 空操作不入账：撤销栈里不该塞「我点了一下但什么都没变」。
    if (next === root) return;
    setPast((p) => [...p.slice(-199), root]);
    setRoot(next);
    setMoves((m) => [...m.slice(-199), move]);
  };

  const addBlockTo = (parentId: string) => {
    const next = addLeaf(root, parentId);
    const id = childIds(next, parentId).at(-1);
    if (id) play(next, { t: "addLeaf", parent: parentId, id });
  };

  const addBlock = () => addBlockTo(here.id);

  const addGroup = () => {
    const next = addSection(root, here.id);
    const id = childIds(next, here.id).at(-1);
    if (id) play(next, { t: "addSection", parent: here.id, id });
  };

  /**
   * 落这一手。移动是 declaratively reversible 的 —— 没有任何不可滑回的后果，
   * 所以不弹确认（Apple：确认只留给真不可逆的事，滥用它等于训练人闭眼点过去）。
   * 回报是「闪一下 + 状态行」，不拽视野：挪一块多半还要再挪下一块。
   */
  const doMove = (id: string, to: string, before?: string) => {
    setMoving(null);
    const next = moveNode(root, id, to, before);
    // moveNode 把“其实没动”原样递回来，play 的早退会兜住 —— 不用在这儿再判一遍。
    play(next, { t: "move", id, to, ...(before === undefined ? {} : { before }) });
  };

  const remove = (id: string) => {
    // 按钮不会画出来，这是兜底：万一从别的路径进来了，别把「删了又冒出来」演一遍。
    if (locks.has(id)) return;
    play(removeNode(root, id), { t: "remove", id });
  };

  const toggleLock = async (id: string, next: boolean) => {
    if (!projectId) return;
    try {
      setLocks(new Set(await setLock(projectId, id, next)));
    } catch {
      setStatus("error");
    }
  };

  /** 开改名的那一刻，是谁被双击的 —— 改完把焦点还给它，别让人跟丢。 */
  const editAnchor = useRef<HTMLElement | null>(null);

  const beginRename = (id: string, el: HTMLElement | null, where: "tree" | "board") => {
    editAnchor.current = el;
    setEditing({ id, where });
  };

  /** 改名结束（提交或取消）。要不要落盘，看调用方传不传 next。 */
  const endRename = () => {
    const el = editAnchor.current;
    editAnchor.current = null;
    setEditing(null);
    // 同步还焦点：拖到下一帧去还，会跟下一场改名撞车（旧的那一帧把新输入框顶掉）。
    el?.focus();
  };

  const commitRename = (id: string, name: string) => {
    endRename();
    // 名字没变 / 改成空白时 renameNode 会把原树原样递回来，play 的早退会兜住。
    play(renameNode(root, id, name), { t: "rename", id, name: name.trim() });
  };

  const cancelRename = () => endRename();

  /**
   * 07 号菜单里那几条。删除已经收进菜单了（行上、块上只留一颗 •••），
   * 所以「重命名 / 复制名字 / 复制路径 / 删除」这套动作得在这儿接住。
   */
  const menuItems = (id: string, host: HTMLElement): MenuItem[] => {
    const node = findNode(root, id);
    if (!node) return [];
    // 输入框就长在这一行 / 这一块里面 —— 行和块都有一层 [data-node-id]。
    const target =
      host.querySelector<HTMLElement>("[data-node-id]") ?? host.closest<HTMLElement>("[data-node-id]");
    const items: MenuItem[] = [
      {
        key: "open",
        label: "打开",
        icon: <IconPage />,
        run: () => openBlock(id),
      },
      {
        key: "notes",
        label: "笔记",
        icon: <IconNote />,
        run: () => openNotes(id),
      },
      {
        key: "rename",
        label: "重命名",
        icon: <IconPencil />,
        run: () => beginRename(id, target, target?.closest("aside") ? "tree" : "board"),
      },
      {
        key: "move",
        label: "移动到…",
        icon: <IconMove />,
        run: () => setMoving({ id, rect: (target ?? host).getBoundingClientRect() }),
      },
      {
        key: "copy-name",
        label: "复制名字",
        icon: <IconCopy />,
        run: () => void copyText(node.name),
      },
      {
        key: "copy-path",
        label: "复制路径",
        icon: <IconLink />,
        run: () => void copyText(trees.get(id) ?? node.name),
      },
    ];
    if (isSection(node)) {
      items.push({
        key: "enter",
        label: "进入",
        icon: <IconEnter />,
        run: () => setPath((prev) => [...prev, id]),
      });
    }
    // 只有块能锁。组是容器 —— 锁容器等于间接锁子节点，那是同一件事的第二种写法。
    if (!isSection(node)) {
      const locked = locks.has(id);
      items.push({
        key: "lock",
        label: locked ? "解锁" : "锁定",
        icon: locked ? <IconLockOpen /> : <IconLock />,
        separatorBefore: true,
        run: () => void toggleLock(id, !locked),
      });
      if (locked) return items;
    }
    items.push({
      key: "remove",
      label: "删除",
      icon: <IconTrash />,
      danger: true,
      separatorBefore: !isSection(node),
      run: () => remove(id),
    });
    return items;
  };

  const undo = () => {
    if (!past.length) return;
    const back = past[past.length - 1]!;
    setPast((p) => p.slice(0, -1));
    setRoot(back);
    setMoves((m) => [...m.slice(-199), { t: "undo" }]);
    setPath((ids) =>
      ids.filter((id) => {
        const n = findNode(back, id);
        return n !== undefined && isSection(n);
      }),
    );
  };

  /** 撞了别人写的版本：只给「重读磁盘」这一条路 —— 自动覆盖等于帮另一个标签页删数据。 */
  const reload = () => setEpoch((e) => e + 1);

  /** 本地有一手没发出去，而磁盘已经被改过。拿本地这份盖上去 —— 人明确点了才盖。 */
  const keepDraft = () => {
    const d = orphan;
    if (!d || !projectId) return;
    // 底用磁盘当前的 version：现在写下去不会撞 409，意向就是「以我为准」。
    lastWritten.current = "";
    skipSave.current = false;
    blocked.current = false;
    ready.current = true;
    setRoot(d.root);
    setMoves(d.moves);
    setPast([]);
    setOrphan(null);
    setStatus("saving");
  };

  /** 反过来：认磁盘那份，把本地草稿丢掉。 */
  const dropDraft = () => {
    if (projectId) clearDraft(projectId);
    setOrphan(null);
    setStatus("saved");
  };

  const newBoard = async () => {
    const name = window.prompt("新项目叫什么", "new");
    if (!name?.trim()) return;
    try {
      const board = await createBoard(name.trim());
      setBoards((bs) => [board, ...bs]);
      setProjectId(board.id);
    } catch {
      setStatus("error");
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // 块的页面盖在上面时，盘面不该再抢键盘（它自己处理 Esc）。
      if (openDoc) return;
      // 正在改名时键盘全归输入框：⌘Z 是文本撤销，Backspace 是删字，不是删块。
      if (editing) return;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") {
        e.preventDefault();
        undo();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setFindOpen((o) => !o);
        return;
      }
      // 焦点在哪一格，按键就作用在那一格上 —— 悬停才出现的按钮不该是唯一入口。
      const anchor = (e.target as HTMLElement | null)?.closest?.("[data-node-id]") as
        | HTMLElement
        | undefined;
      const id = anchor?.dataset.nodeId;
      if (!anchor || !id) return;
      if (e.key === "F2") {
        e.preventDefault();
        beginRename(id, anchor, anchor.closest("aside") ? "tree" : "board");
        return;
      }
      if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        play(removeNode(root, id), { t: "remove", id });
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const placed = useMemo(() => {
    if (!size.w || !size.h) return [];

    const build = (parent: Node, area: Box, depth: number, color: string): Placed[] =>
      layout(parent, area, depth).map((kid, i) => {
        // 只有最外一层按分支分色；里面的块都继承自己那一支的颜色（pele 就是这么做的）。
        const kidColor = depth === 0 ? SERIES[i % SERIES.length]! : color;
        const box = shrink(kid.box, GAP);
        if (!kid.node.children) return { node: kid.node, box, depth: kid.depth, color: kidColor, kids: [] };
        const head = hat(box, kid.depth);
        return {
          node: kid.node,
          box,
          depth: kid.depth,
          color: kidColor,
          kids: build(
            kid.node,
            {
              x: box.x + SIDE,
              y: box.y + head,
              w: Math.max(box.w - SIDE * 2, 1),
              h: Math.max(box.h - head - SIDE, 1),
            },
            kid.depth,
            kidColor,
          ),
        };
      });

    return build(here, { x: 0, y: 0, w: size.w, h: size.h }, 0, SERIES[0]!);
  }, [here, size]);

  return (
    <NodeMenuProvider items={menuItems}>
      <main className="relative flex h-dvh w-full overflow-hidden bg-canvas text-text antialiased">
        <aside
          className={`shrink-0 overflow-hidden border-r border-border bg-canvas transition-[width] duration-300 ${EASE} ${
            narrow
              ? `absolute bottom-0 left-0 top-[52px] z-20 shadow-[0_0_60px_rgba(0,0,0,0.6)] ${side ? "w-[248px]" : "w-0 border-r-0"}`
              : `relative ${side ? "w-[248px]" : "w-0 border-r-0"}`
          }`}
          aria-hidden={!side}
          // 块页面盖着的时候，下面的盘面不该还能 Tab 进去 —— 但它也不卸载（状态在它身上）。
          inert={!!openDoc}
        >
          <div
            className={`h-full w-[248px] overflow-y-auto ${side ? "opacity-100" : "opacity-0"} transition-opacity duration-200`}
          >
            <Tree
              root={root}
              path={path}
              editingId={editing?.where === "tree" ? editing.id : null}
              onEnter={setPath}
              onBeginRename={(id, el) => beginRename(id, el, "tree")}
              onCommitRename={commitRename}
              onCancelRename={cancelRename}
            />
          </div>
        </aside>

        {narrow && side && (
          <button
            type="button"
            aria-label="关闭左列浮层"
            onClick={() => setSide(false)}
            className="absolute bottom-0 left-0 top-[52px] z-10 w-full bg-black/40"
          />
        )}

        <div className="flex min-w-0 flex-1 flex-col" inert={!!openDoc}>
          <header className="flex h-[52px] shrink-0 items-center gap-2 border-b border-border px-3">
            <IconButton
              label={side ? "收起左列" : "展开左列"}
              onClick={() => setSide(!side)}
              className={side ? "text-text/80" : "text-muted"}
            >
              <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
                <rect x="1.5" y="2.5" width="13" height="11" rx="2.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
                <path d="M6 2.5v11" stroke="currentColor" strokeWidth="1.2" />
              </svg>
            </IconButton>

            <nav className="flex min-w-0 items-center gap-0.5 text-[13px]" aria-label="项目与路径">
              <select
                value={projectId ?? ""}
                onChange={(e) => (e.target.value === NEW_BOARD ? void newBoard() : setProjectId(e.target.value))}
                title="每个目录是一个独立项目"
                className="mr-1 h-7 max-w-[150px] shrink-0 truncate rounded-lg bg-surface px-1.5 text-[12px] text-text/90 outline-none hover:bg-raised"
              >
                {boards.map((b) => (
                  <option key={b.id} value={b.id}>
                    {b.name}
                  </option>
                ))}
                <option value={NEW_BOARD}>＋ 新项目…</option>
              </select>
              <span className="text-text/25" aria-hidden="true">/</span>
              <button
                type="button"
                onClick={() => setPath([])}
                className={`truncate rounded-md px-1.5 py-1 transition-colors ${path.length ? "text-muted hover:text-text" : "text-text"}`}
              >
                {root.name}
              </button>
              {path.map((id, i) => (
                <span key={id} className="flex min-w-0 items-center gap-0.5">
                  <span className="text-text/25" aria-hidden="true">›</span>
                  <button
                    type="button"
                    onClick={() => setPath(path.slice(0, i + 1))}
                    className={`truncate rounded-md px-1.5 py-1 transition-colors ${
                      i === path.length - 1 ? "text-text" : "text-muted hover:text-text"
                    }`}
                  >
                    {findNode(root, id)?.name ?? "?"}
                  </button>
                </span>
              ))}
            </nav>

            <div className="ml-auto flex items-center gap-1.5">
              <button
                type="button"
                onClick={status === "conflict" ? reload : undefined}
                disabled={status !== "conflict"}
                title={
                  status === "conflict"
                    ? "磁盘上的这一盘被别的窗口改过了。点一下重读磁盘，本地这几手会丢。"
                    : "每次落子自动写到 content/<id>/tree.json"
                }
                className={`mr-1 text-[11px] tabular-nums transition-colors ${
                  status === "conflict" ? "text-danger hover:underline" : "text-muted"
                }`}
              >
                {STATUS_TEXT[status]}
              </button>
              <div className="mr-1 flex items-baseline gap-1.5">
                <span className="text-[11px] text-muted">总分</span>
                <span className="text-[17px] font-semibold tabular-nums" style={{ letterSpacing: "-0.02em" }}>
                  {valueOf(root)}
                </span>
              </div>
              <IconButton label="按 tree 或 id 找一块（⌘K）" onClick={() => setFindOpen(true)}>
                <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
                  <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.2" />
                  <path d="M10.5 10.5 14 14" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" />
                </svg>
              </IconButton>
              <Pill onClick={undo} disabled={!past.length} title="⌘Z / Ctrl+Z">
                撤销
              </Pill>
              <Pill onClick={addBlock} disabled={status === "conflict"}>
                ＋ 块
              </Pill>
              <Pill onClick={addGroup} disabled={status === "conflict"}>
                ＋ 组
              </Pill>
            </div>
          </header>

          <div ref={board} className="relative min-h-0 flex-1 bg-board p-2" aria-label="盘面">
            {orphan && (
              <div className="absolute inset-x-2 top-2 z-30 flex flex-wrap items-center gap-2 rounded-lg border border-border bg-surface px-3 py-2 text-[12px] shadow-lg">
                <span className="min-w-0 flex-1 text-muted">
                  本地有 {orphan.moves.length} 手没发出去，但磁盘上的这一盘已经被改过了。两边都还在，你选。
                </span>
                <Pill onClick={keepDraft}>用本地的</Pill>
                <Pill onClick={dropDraft} muted>
                  丢掉本地的
                </Pill>
              </div>
            )}
            {placed.length === 0 && (
              <div className="absolute inset-0 flex flex-col items-center justify-center gap-3 text-[13px] text-muted">
                <span>盘上什么都没有了</span>
                <Pill onClick={() => addBlockTo(root.id)}>＋ 块</Pill>
              </div>
            )}
            <div className="relative h-full w-full">
              {placed.map((p) => (
                <Piece
                  key={p.node.id}
                  piece={p}
                  origin={{ x: 0, y: 0 }}
                  isSection={isSection}
                  trees={trees}
                  flashId={flash}
                  editingId={editing?.where === "board" ? editing.id : null}
                  onEnter={(id) => setPath([...path, id])}
                  onOpen={openBlock}
                  onBeginRename={(id, el) => beginRename(id, el, "board")}
                  onCommitRename={commitRename}
                  onCancelRename={cancelRename}
                />
              ))}
            </div>
          </div>
        </div>

        {findOpen && <Find hits={hits} onGo={goTo} onClose={() => setFindOpen(false)} />}
        {moving && projectId && (
          <MoveTo
            source={moving.id}
            root={root}
            anchor={moving.rect}
            onMove={(to, before) => doMove(moving.id, to, before)}
            onClose={() => setMoving(null)}
          />
        )}

        {/* 块自己的页面：真实 URL、盖在盘面上面、盘面不卸载。 */}
        {openDoc && projectId && (
          <DocPage
            key={openDoc}
            board={projectId}
            id={openDoc}
            root={root}
            ready={status !== "loading"}
            onClose={closeDoc}
            onOpenGroup={(ids) => {
              setPath(ids);
              closeDoc();
            }}
          />
        )}
      </main>
    </NodeMenuProvider>
  );
}

type Placed = {
  node: Node;
  box: Box;
  depth: number;
  color: string;
  kids: Placed[];
};

/**
 * 一个格子。section 的子树渲染在它自己里面（不是平铺成兄弟节点）——
 * 这样鼠标停在里面的块上，section 的 `group-hover` 才成立，标题带上的按钮才出得来。
 */
function Piece({
  piece,
  origin,
  isSection,
  trees,
  flashId,
  editingId,
  onEnter,
  onOpen,
  onBeginRename,
  onCommitRename,
  onCancelRename,
}: {
  piece: Placed;
  origin: { x: number; y: number };
  isSection: (n: Node) => boolean;
  trees: Map<string, string>;
  flashId: string | null;
  editingId: string | null;
  onEnter: (id: string) => void;
  /** 开这一块的页面。组和块都有 —— 正文不长在树上，长在 map/<id>/AGENTS.md 里。 */
  onOpen: (id: string) => void;
  onBeginRename: (id: string, el: HTMLElement | null) => void;
  onCommitRename: (id: string, name: string) => void;
  onCancelRename: () => void;
}) {
  const { node, box, depth, color, kids } = piece;
  const place = { left: box.x - origin.x, top: box.y - origin.y, width: box.w, height: box.h };
  const section = isSection(node);
  const value = valueOf(node);
  const editing = editingId === node.id;
  const tree = trees.get(node.id) ?? node.name;
  /** ⌘K 跳过来的时候套一圈 accent。只在被点名那一块上，一秒就撤。 */
  const ring = flashId === node.id ? ", 0 0 0 2px var(--n-color-accent)" : "";

  // 双击改名的规矩：名字画得出来的地方才能双击它。名字没地方站，就回左列去改。
  const noRename = (e: React.MouseEvent) =>
    (e.target as HTMLElement).closest("[data-no-rename]") !== null;
  if (section) {
    const strip = hat(box, depth);
    const dots = Math.min(box.w, box.h) < 40 ? "xs" : Math.min(box.w, box.h) < 64 ? "s" : "m";
    return (
      <div
        data-node-id={node.id}
        className={`group/sec nm-host nm-host--block absolute overflow-hidden rounded-[4px] transition-[left,top,width,height] duration-300 ${EASE}`}
        style={{
          ...place,
          background: tint(color, 0.12),
          // pele 的组框描边就是 series 原色，1px，不降透明度。
          boxShadow: `inset 0 0 0 1px ${color}${ring}`,
        }}
      >
        {strip > 0 && (
          <div
            tabIndex={-1}
            className="absolute inset-x-0 top-0 flex items-center gap-1.5 pl-2 pr-8 text-[11px] outline-none"
            style={{ height: strip, color: "var(--n-color-text)" }}
            onDoubleClick={(e) => {
              if (noRename(e)) return;
              onBeginRename(node.id, e.currentTarget);
            }}
          >
            {editing ? (
              <NameInput
                value={node.name}
                onCommit={(name) => onCommitRename(node.id, name)}
                onCancel={onCancelRename}
                className="flex-1 px-1 text-[11px]"
              />
            ) : (
              <>
                <span className="truncate" style={{ letterSpacing: "0.01em" }}>
                  {node.name}
                </span>
                {strip >= 24 && (
                  <span className="shrink-0 text-muted">{node.children!.length} 块</span>
                )}
                {/* tree 只在盘面上露脸：左列保持干净，那是给人看结构的。 */}
                {strip >= 18 && (
                  <Tag
                    value={tree}
                    title={`${tree} —— 点一下复制（${node.name} 的 tree）`}
                    muted
                  />
                )}
                {strip >= 18 && box.w > 110 && (
                  <Tag
                    value={node.id}
                    title={`${node.id} —— 点一下复制（${node.name} 的 id）`}
                    className="shrink-0 opacity-45 hover:opacity-80"
                  />
                )}
                <button
                  type="button"
                  data-no-rename
                  onClick={() => onEnter(node.id)}
                  title={`进入 ${node.name}`}
                  className="ml-auto shrink-0 rounded-[3px] px-1.5 py-0.5 text-[11px] opacity-0 transition-opacity duration-150 hover:bg-white/10 group-hover/sec:opacity-100"
                >
                  进入 ›
                </button>
              </>
            )}
          </div>
        )}
        {!editing && <NodeMenuTrigger nodeId={node.id} kind="block" size={dots} />}
        {kids.map((k) => (
          <Piece
            key={k.node.id}
            piece={k}
            origin={box}
            isSection={isSection}
            trees={trees}
            flashId={flashId}
            editingId={editingId}
            onEnter={onEnter}
            onOpen={onOpen}
            onBeginRename={onBeginRename}
            onCommitRename={onCommitRename}
            onCancelRename={onCancelRename}
          />
        ))}
      </div>
    );
  }

  const big = Math.min(box.w, box.h);
  // 编辑中先把名字的位置腾出来，再小的块也得看得见自己在打什么。
  const showName = (box.w > 56 && box.h > 30) || editing;
  const showValue = big > 24;
  // tree 比名字还挑地方：它是给人抄走的，画一半还不如不画。
  const showTree = box.w > 62 && box.h > 48;
  // id 是 tree 下面再加一行，所以门槛比 tree 还严一档。
  const showId = showTree && box.h > 62;
  const valueSize = Math.max(12, Math.min(40, Math.round(big / 3)));
  const roomForDelete = box.w > 34 && box.h > 26;
  /** ••• 比原来那颗 × 略小一号，所以 30px 见方的块也容得下 —— 但 30px 以下就真没了。 */
  const dots = Math.min(box.w, box.h) < 40 ? "xs" : Math.min(box.w, box.h) < 64 ? "s" : "m";

  return (
    <div
      className={`group/leaf nm-host nm-host--block absolute overflow-hidden rounded-[4px] transition-[left,top,width,height] duration-300 ${EASE}`}
      style={{
        ...place,
        background: color,
        color: "var(--n-color-canvas)",
        boxShadow: ring ? "0 0 0 2px var(--n-color-accent)" : undefined,
      }}
    >
      <div
        tabIndex={0}
        aria-label={`${node.name} ${value}`}
        data-node-id={node.id}
        onDoubleClick={(e) => {
          if (noRename(e)) return;
          onBeginRename(node.id, e.currentTarget);
        }}
        className="flex h-full w-full flex-col items-center justify-center gap-1 outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-black/40"
      >
        {editing ? (
          <NameInput
            value={node.name}
            onCommit={(name) => onCommitRename(node.id, name)}
            onCancel={onCancelRename}
            className="w-full px-1 text-center text-[10px]"
          />
        ) : (
          showName && (
            <span className="pointer-events-none max-w-full truncate px-1.5 text-[10px] leading-none opacity-65">
              {node.name}
            </span>
          )
        )}
        {showValue && (
          <span
            className="pointer-events-none font-semibold tabular-nums leading-none"
            style={{ fontSize: valueSize, letterSpacing: "-0.02em" }}
          >
            {value}
          </span>
        )}
        {showTree && !editing && (
          <>
            <Tag
              value={tree}
              title={`${tree} —— 点一下复制（${node.name} 的 tree）`}
              className="max-w-full opacity-55 hover:opacity-90"
            />
            {showId && (
              <Tag
                value={node.id}
                title={`${node.id} —— 点一下复制（${node.name} 的 id）`}
                className="max-w-full opacity-40 hover:opacity-80"
              />
            )}
          </>
        )}
      </div>
      {roomForDelete && !editing && <NodeMenuTrigger nodeId={node.id} kind="block" size={dots} />}
      {roomForDelete && !editing && (
        <button
          type="button"
          data-no-rename
          onClick={() => onOpen(node.id)}
          title={`打开 ${tree}（新页面）`}
          className="absolute bottom-[3px] right-[3px] rounded-[3px] bg-black/35 px-1.5 py-[1px] text-[10px] leading-[1.3] text-white/90 opacity-0 backdrop-blur-sm transition-opacity duration-150 hover:bg-black/55 group-hover/leaf:opacity-100"
        >
          打开 ›
        </button>
      )}
    </div>
  );
}

/**
 * 盘面上那枚可复制的小标签。tree 和 id 共用这一套画法 —— 点一下复制，复制完变「已复制」。
 *
 * 两个值不一样：tree 是名字路径，改名就变；id 是随机六位，永不失效。同一个壳子，两种内容。
 */
function Tag({
  value,
  title,
  className = "",
  muted = false,
}: {
  value: string;
  title: string;
  className?: string;
  muted?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    if (!(await copyText(value))) return;
    setCopied(true);
    setTimeout(() => setCopied(false), 900);
  };
  return (
    <button
      type="button"
      data-no-rename
      onClick={(e) => {
        e.stopPropagation();
        void copy();
      }}
      title={title}
      className={`min-w-0 truncate rounded-[3px] px-1 font-mono text-[9.5px] leading-[1.4] transition-colors duration-150 hover:bg-white/15 ${
        muted ? "text-muted" : ""
      } ${className}`}
    >
      {copied ? "已复制" : value}
    </button>
  );
}

function Pill({
  children,
  onClick,
  disabled,
  muted,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  disabled?: boolean;
  muted?: boolean;
  title?: string;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`h-7 rounded-lg px-2.5 text-[12px] transition-colors duration-150 disabled:opacity-25 ${
        muted
          ? "text-muted hover:bg-surface hover:text-text"
          : "bg-surface text-text/90 hover:bg-raised"
      }`}
    >
      {children}
    </button>
  );
}

function IconButton({
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

/* 菜单里那几个图标。都 ≤14px 的线稿 —— 菜单项里再花就只剩噪点了。 */
const stroke = {
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round",
  strokeLinejoin: "round",
} as const;

const IconPencil = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M9.2 2.4 11.6 4.8 5.4 11H3V8.6z" />
  </svg>
);

const IconCopy = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <rect x="2.5" y="2.5" width="7" height="7" rx="1.6" />
    <path d="M5.5 11.5h4.4a1.6 1.6 0 0 0 1.6-1.6V5.5" />
  </svg>
);

const IconLink = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M5.8 8.2a2.5 2.5 0 0 0 3.5 0l1.9-1.9a2.5 2.5 0 0 0-3.5-3.5l-.5.5" />
    <path d="M8.2 5.8a2.5 2.5 0 0 0-3.5 0L2.8 7.7a2.5 2.5 0 0 0 3.5 3.5l.5-.5" />
  </svg>
);

/** “移动”：一根横向箭头两端带端帽，旁边一枚沿箭头进框的竖 —— 换爹与排序同一枚图标。 */
const IconMove = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M2 10.2h10M4.4 8 2 10.2 4.4 12.4M10.4 2v6.4c0 .6-.5 1.1-1.1 1.1H4" />
    <path d="M6 6.8 3.6 9.3 6 11.8" />
  </svg>
);

const IconEnter = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M3 3v5.2c0 .9.7 1.6 1.6 1.6H11" />
    <path d="M8.8 6.6 11 9.8l-2.2 3.2" transform="translate(0 -3)" />
  </svg>
);

/** “打开”：一张纸右上角折一下。不让它长得像“进入”（那个是下一层）。 */
const IconPage = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M3.2 1.8h5.2l2.4 2.4v8H3.2z" />
    <path d="M8.4 1.8v2.5h2.4" />
  </svg>
);

const IconNote = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M2.4 3.1h9.2M2.4 6.4h9.2M2.4 9.7h5.4" />
  </svg>
);

const IconLock = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <rect x="3.3" y="6.2" width="7.4" height="5.4" rx="1.1" />
    <path d="M5.2 6.2V4.6a1.8 1.8 0 0 1 3.6 0v1.6" />
  </svg>
);

const IconLockOpen = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <rect x="3.3" y="6.2" width="7.4" height="5.4" rx="1.1" />
    <path d="M5.2 6.2V4.6a1.8 1.8 0 0 1 3.6 0" />
  </svg>
);

const IconTrash = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" {...stroke}>
    <path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4" />
    <path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4" />
  </svg>
);
