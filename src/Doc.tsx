import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type Node, findNode, isSection, pathTo, treeMap } from "./game";
import { normalizeDocName } from "./docname";
import {
  type DocFile,
  DocConflictError,
  DocEditError,
  clearDocDraft,
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
 * 这是 `demo/doc-editor/` 的 10 号方案（命令面板）落地后的样子，四条是刻意的：
 *
 * - **列表默认不存在。** 屏幕上只有正文。切文档按 `⌘K`，敲两个字母就到；
 *   正文也能搜 —— 这是唯一一个「不占地方又能找到东西」的列表。
 * - **新建 / 删除 / 恢复在 `⋯` 里。** 三个动词都在那儿，和「找文档」分开：
 *   找是高频、随手；删是低频、要停一下。
 * - **`AGENTS.md` 只能改。** 它是 coding agent 进目录时自动找的文件名，删了整块就没人读得懂。
 *   界面上那个删除入口根本不生成 —— 真拦在服务端（403）。
 * - **它盖在盘面上面，不卸载盘面。** 卸载会丢掉树的展开状态、滚动位置和撤销栈。
 *
 * 每份文档有自己的 etag 和自己的本地草稿：写一篇不该把别的文档顶成 409。
 */
const SAVE_DELAY = 400;
const ENTRY = "AGENTS.md";

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
  const [stubFile, setStubFile] = useState(false);
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
          setStubFile(false);
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
        setStubFile(doc.stub);
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
    stubFile,
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
 * 页面
 * ------------------------------------------------------------------ */
export function DocPage({
  board,
  id,
  root,
  ready,
  onClose,
  onOpenGroup,
}: {
  board: string;
  id: string;
  root: Node;
  /** 树载完了没有。没载完就说「载入中」，别急着喊「查无此块」。 */
  ready: boolean;
  onClose: () => void;
  /** 回到它所在的组 —— 面包屑上那一段。 */
  onOpenGroup: (ids: string[]) => void;
}) {
  const node = findNode(root, id);
  const tree = useMemo(() => treeMap(root).get(id) ?? node?.name ?? id, [root, id, node]);

  const [narrow, setNarrow] = useState(() => window.matchMedia("(max-width: 819px)").matches);

  const [files, setFiles] = useState<DocFile[]>([]);
  const [hasApp, setHasApp] = useState(false);
  const [cur, setCur] = useState<string>(() => readLastDoc(board, id) ?? ENTRY);

  /* 浮层：命令面板 / ⋯ 菜单 / 新建输入 / 删除确认。一次只开一个。 */
  const [pal, setPal] = useState(false);
  const [palQ, setPalQ] = useState("");
  const [palRows, setPalRows] = useState<DocFile[]>([]);
  const [palCur, setPalCur] = useState(0);
  const [menu, setMenu] = useState(false);
  const [adding, setAdding] = useState(false);
  const [addErr, setAddErr] = useState("");
  const [confirm, setConfirm] = useState<string | null>(null);
  /** 正在改名的那一份。`null` = 没在改。 */
  const [renaming, setRenaming] = useState<string | null>(null);
  /** 一句给眼睛看的回执（「建好了」「改名了」）。浮起来，几秒后自己走。 */
  const [note, setNote] = useState("");
  /** 正文滚起来没有。滚起来了工具条才浮起来 —— 用阴影替掉那根硬边框。 */
  const [scrolled, setScrolled] = useState(false);

  const closeAll = useCallback(() => {
    setPal(false);
    setMenu(false);
    setAdding(false);
    setRenaming(null);
    setAddErr("");
    setConfirm(null);
  }, []);

  /* 回执不占地方：3.4 秒，够看完一句话。 */
  useEffect(() => {
    if (!note) return;
    const t = window.setTimeout(() => setNote(""), 3400);
    return () => window.clearTimeout(t);
  }, [note]);

  const doc = useDocFile(board, id, cur, ready && !!node);

  const refresh = useCallback(async () => {
    const list = await listDocs(board, id);
    if (!list) return null;
    setFiles(list.files);
    setHasApp(list.hasApp);
    return list.files;
  }, [board, id]);

  useEffect(() => {
    if (!ready || !node) return;
    void refresh();
  }, [ready, node, refresh]);

  useEffect(() => {
    const mq = window.matchMedia("(max-width: 819px)");
    const on = () => setNarrow(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);

  /* —— 当前在看哪一份 → 记住；那份不在了（别处删了）→ 退回 AGENTS.md —— */
  useEffect(() => {
    if (!ready || !node) return;
    writeLastDoc(board, id, cur);
  }, [board, id, cur, ready, node]);

  const live = useMemo(() => files.filter((f) => !f.trashed), [files]);
  const gone = useMemo(() => files.filter((f) => f.trashed), [files]);

  const current = useMemo(() => files.find((f) => f.name === cur), [files, cur]);

  /* —— 命令面板：空查询给全部，有查询就交给服务端搜（正文不上网） —— */
  useEffect(() => {
    if (!pal) return;
    let alive = true;
    const run = () => {
      if (!palQ.trim()) {
        setPalRows(live);
        setPalCur(0);
        return;
      }
      void listDocs(board, id, palQ).then((out) => {
        if (!alive || !out) return;
        setPalRows(out.files.filter((f) => !f.trashed));
        setPalCur(0);
      });
    };
    // 每敲一个字都问一次太吵：压 140ms，比一格键盘的重复还快。
    const t = window.setTimeout(run, 140);
    return () => {
      alive = false;
      window.clearTimeout(t);
    };
  }, [pal, palQ, live, board, id]);

  /* —— Esc 从最上面那层往下关。⌘K 开关面板，⌘S 立刻写。 —— */
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        if (confirm) return setConfirm(null);
        if (pal) return setPal(false);
        if (menu) return setMenu(false);
        if (adding || renaming) {
          setAdding(false);
          setRenaming(null);
          setAddErr("");
          return;
        }
        onClose();
        return;
      }
      if (mod && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setMenu(false);
        setConfirm(null);
        setPalQ("");
        setPal((p) => !p);
        return;
      }
      if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        doc.saveNow();
      }
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [confirm, pal, menu, adding, renaming, onClose, doc]);

  const open = (name: string) => {
    if (name === cur) {
      closeAll();
      return;
    }
    setCur(name);
    closeAll();
  };

  const newDoc = async (raw: string) => {
    const name = normalizeDocName(raw);
    if (!name) return setAddErr("名字空了 —— 至少给一个字");
    if (name === ENTRY.toLowerCase()) return setAddErr("AGENTS.md 是入口，本来就有一份");
    if (files.some((f) => f.name.toLowerCase() === name)) {
      return setAddErr(`${name} 已经在了，换一个`);
    }
    try {
      const out = await createDoc(board, id, name);
      await refresh();
      setAdding(false);
      setAddErr("");
      setMenu(false);
      setCur(out.name);
      setNote(`${out.name} 建好了 —— 直接写`);
    } catch (err) {
      setAddErr(
        err instanceof DocEditError && err.reason === "exists" ? `${name} 已经在了` : "没建成，名字可能不合规矩",
      );
    }
  };

  const doTrash = async (name: string) => {
    setConfirm(null);
    try {
      await trashDoc(board, id, name);
      const list = await refresh();
      setNote(`${name} 已移到最近删除 —— ⋯ 里能恢复`);
      if (cur === name) {
        const next = list?.find((f) => !f.trashed);
        setCur(next?.name ?? ENTRY);
      }
    } catch (err) {
      setNote(
        err instanceof DocEditError && err.reason === "protected"
          ? "AGENTS.md 只能改，删不掉"
          : "没删成",
      );
    }
  };

  /**
   * 改名。走的是「同一个文件夹里的文件改名」，正文一个字节都不重写 ——
   * 所以 etag 不变，另一个标签页开着它也不会被顶成 409。
   */
  const doRename = async (from: string, raw: string) => {
    const to = normalizeDocName(raw);
    if (!to) return setAddErr("名字空了 —— 至少给一个字");
    if (to === from) {
      closeAll();
      return;
    }
    if (files.some((f) => f.name.toLowerCase() === to)) return setAddErr(`${to} 已经在了，换一个`);
    try {
      const out = await renameDoc(board, id, from, to);
      await refresh();
      if (cur === from) setCur(out.name);
      closeAll();
      setNote(`${from} → ${out.name}`);
    } catch (err) {
      const reason = err instanceof DocEditError ? err.reason : "other";
      setAddErr(
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
      setMenu(false);
      setCur(out.name);
      setNote(`${out.name} 回来了`);
    } catch (err) {
      setNote(err instanceof DocEditError && err.reason === "exists" ? "同名的那份还在，先删它" : "没恢复成");
    }
  };

  if (!ready) {
    return <Shell onClose={onClose} crumbs={[id]} title={id}>{null}</Shell>;
  }

  if (!node) {
    return (
      <Shell onClose={onClose} crumbs={[id]} title={id}>
        <p className="text-[13px] text-muted">
          树里没有这个节点。可能已经被删了 —— 它的文件夹还躺在 map/.gone/ 里，撤销回来就接得上。
        </p>
      </Shell>
    );
  }

  const crumbs = tree.split("/");
  const title = cur.replace(/\.md$/, "");

  return (
    <div
      role="dialog"
      aria-label={`${node.name} 的文档`}
      className="absolute inset-0 z-40 flex flex-col bg-canvas text-text"
    >
      {/* 一条头：返回、位置、当前是哪一份、状态、⌘K、⋯。 */}
      <header className="dp-toolbar" data-lifted={scrolled}>
        <button
          type="button"
          onClick={onClose}
          aria-label="返回盘面"
          title="返回盘面（Esc）"
          className={`dp-icon ${narrow ? "dp-icon--touch" : ""}`}
        >
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
        </button>

        <nav className="dp-crumbs ml-1" aria-label="这个块在树里的位置">
          {crumbs.map((name, i) => {
            const last = i === crumbs.length - 1;
            return (
              <span key={`${name}-${i}`} className="flex min-w-0 items-center">
                {i > 0 && (
                  <span className="dp-crumb-sep" aria-hidden="true">
                    ›
                  </span>
                )}
                {last ? (
                  <span className="dp-crumb dp-crumb--here">{name}</span>
                ) : (
                  <button
                    type="button"
                    onClick={() => onOpenGroup(pathTo(root, id))}
                    className="dp-crumb"
                  >
                    {name}
                  </button>
                )}
              </span>
            );
          })}
        </nav>

        <div className="ml-auto flex shrink-0 items-center gap-1">
          {!narrow && (
            <span
              className="dp-chip mr-0.5"
              title="这一块的 id 同时是它在 map/ 里的文件夹名"
            >
              {isSection(node) ? "组" : "块"} · {id}
            </span>
          )}
          <button
            type="button"
            onClick={() => {
              setMenu(false);
              setPalQ("");
              setPal((p) => !p);
            }}
            aria-label="找文档（⌘K）"
            title="找文档（⌘K）"
            aria-expanded={pal}
            className={`flex items-center gap-1 rounded-lg text-[11.5px] text-muted transition-[background,color,scale] duration-150 hover:bg-surface hover:text-text active:scale-[0.96] ${
              narrow ? "h-11 min-w-11 justify-center" : "h-7 px-2"
            }`}
          >
            <svg viewBox="0 0 14 14" className="h-3.5 w-3.5" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
              <circle cx="6.3" cy="6.3" r="3.5" />
              <path d="M9 9 11.6 11.6" />
            </svg>
            {!narrow && <kbd className="font-mono text-[10.5px] text-muted/70">⌘K</kbd>}
          </button>
          <button
            type="button"
            onClick={() => {
              setPal(false);
              setAddErr("");
              setAdding(false);
              setRenaming(null);
              setMenu((m) => !m);
            }}
            aria-label="更多操作"
            title="更多操作"
            aria-expanded={menu}
            /* 别加 `nm-dots`：那套样式是给浮在块/行上的那些用的 ——
               position:absolute + opacity:0 + pointer-events:none，只在 `.nm-host:hover`
               里复活。这里没有 nm-host，套上去这颗 ••• 在桌面上就是一块点不到的空气。 */
            className={`dp-icon ${narrow ? "dp-icon--touch" : ""}`}
          >
            <svg viewBox="0 0 14 14" className="h-3.5 w-3.5" aria-hidden="true" fill="currentColor">
              <circle cx="3" cy="7" r="1.25" />
              <circle cx="7" cy="7" r="1.25" />
              <circle cx="11" cy="7" r="1.25" />
            </svg>
          </button>
        </div>
      </header>

      <div
        className="dp-scroll"
        onScroll={(e) => {
          const v = e.currentTarget.scrollTop > 4;
          setScrolled((s) => (s === v ? s : v));
        }}
      >
        <div className="dp-column">
          {doc.phase === "loading" && (
            <p className="pt-1 text-[13px] text-muted/70">载入中…</p>
          )}

          {doc.phase === "missing" && (
            <div className="dp-empty">
              <p>
                <span className="font-mono text-text">{cur}</span> 不在磁盘上了 —— 可能刚被别处删掉。
              </p>
              <button
                type="button"
                onClick={() => setCur(ENTRY)}
                className="h-8 rounded-lg bg-surface px-3 text-[12px] text-text/90 transition-[background,scale] hover:bg-raised active:scale-[0.97]"
              >
                回到 AGENTS.md
              </button>
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
              <div className="dp-title-row">
                <h1 className="dp-title min-w-0 break-words">{title}</h1>
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
                    <span key={doc.status} className="dp-status" data-s={doc.status} aria-live="polite">
                      {STATUS_TEXT[doc.status]}
                    </span>
                  </>
                )}
                {hasApp && (
                  <a
                    className="dp-link"
                    href={`/p/${encodeURIComponent(board)}/${encodeURIComponent(id)}/`}
                    title="这一块在自己跑（文件夹里有 scripts.start）"
                  >
                    打开它的页面 ›
                  </a>
                )}
              </div>

              {doc.theirs && (
                <div className="dp-glass mb-3 flex flex-wrap items-center gap-2 rounded-xl px-3 py-2 text-[12px]">
                  <span className="min-w-0 flex-1 text-muted">磁盘上这份被别处改过了。两边都还在，你选。</span>
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

              {doc.stubFile && (
                <p className="mb-2 text-[11.5px] text-muted/70">
                  磁盘上还没有这个文件 —— 开始打字才会建它。
                </p>
              )}

              <textarea
                value={doc.text}
                onChange={(e) => doc.edit(e.target.value)}
                autoFocus
                spellCheck={false}
                aria-label={`${cur} 正文`}
                placeholder="写点什么。改了就自己存。"
                className="dp-text w-full flex-1 resize-none bg-transparent outline-none"
              />
            </>
          )}
        </div>
      </div>

      {/* 回执：一颗浮起来的胶囊，3.4 秒后自己走。 */}
      {note && (
        <div className="dp-glass dp-toast" role="status">
          {note}
        </div>
      )}

      {pal && (
        <Palette
          narrow={narrow}
          q={palQ}
          onQ={(v) => {
            setPalQ(v);
          }}
          rows={palRows}
          cur={palCur}
          onCur={setPalCur}
          onPick={open}
          onClose={() => setPal(false)}
          onDelete={setConfirm}
          total={live.length}
        />
      )}

      {menu && (
        <Menu
          narrow={narrow}
          docs={live}
          trash={gone}
          current={cur}
          mode={renaming ? "rename" : adding ? "add" : "idle"}
          initial={renaming ? renaming.replace(/\.md$/i, "") : ""}
          err={addErr}
          onStartAdd={() => {
            setRenaming(null);
            setAdding(true);
            setAddErr("");
          }}
          onStartRename={(n) => {
            setAdding(false);
            setRenaming(n);
            setAddErr("");
          }}
          onCancelInput={() => {
            setAdding(false);
            setRenaming(null);
            setAddErr("");
          }}
          onSubmit={(v) => (renaming ? void doRename(renaming, v) : void newDoc(v))}
          onDelete={(n) => {
            setMenu(false);
            setConfirm(n);
          }}
          onRestore={(n) => void doRestore(n)}
          onClose={() => setMenu(false)}
        />
      )}

      {confirm && (
        <Confirm
          name={confirm}
          onCancel={() => setConfirm(null)}
          onYes={() => void doTrash(confirm)}
        />
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * 命令面板：桌面是一张小浮层（贴着顶边，像 Spotlight），
 * 手机是一整页（搜索页 —— 那儿没有「浮层」这回事，键盘一起就是全屏）。
 * ------------------------------------------------------------------ */
function Palette({
  narrow,
  q,
  onQ,
  rows,
  cur,
  onCur,
  onPick,
  onClose,
  onDelete,
  total,
}: {
  narrow: boolean;
  q: string;
  onQ: (v: string) => void;
  rows: DocFile[];
  cur: number;
  onCur: (n: number) => void;
  onPick: (name: string) => void;
  onClose: () => void;
  onDelete: (name: string) => void;
  total: number;
}) {
  const input = useRef<HTMLInputElement | null>(null);
  const list = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    input.current?.focus();
  }, []);

  useEffect(() => {
    list.current?.querySelector<HTMLElement>('[data-cur="1"]')?.scrollIntoView({ block: "nearest" });
  }, [cur, rows]);

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (!rows.length) return;
      onCur(Math.max(0, Math.min(rows.length - 1, cur + (e.key === "ArrowDown" ? 1 : -1))));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const hit = rows[cur];
      if (hit) onPick(hit.name);
      return;
    }
    if ((e.metaKey || e.ctrlKey) && e.key === "Backspace") {
      e.preventDefault();
      const hit = rows[cur];
      if (hit && !hit.protected) onDelete(hit.name);
    }
  };

  const panel = (
    <div
      className={`${narrow ? "flex min-h-0 flex-1 flex-col" : "dp-glass dp-pop w-[min(420px,calc(100vw-32px))] overflow-hidden rounded-xl"}`}
      onKeyDown={onKey}
    >
      <div className="flex items-center gap-2 border-b border-border px-3 py-2.5">
        <svg viewBox="0 0 14 14" className="h-3.5 w-3.5 shrink-0 text-muted" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
          <circle cx="6.3" cy="6.3" r="3.5" />
          <path d="M9 9 11.6 11.6" />
        </svg>
        <input
          ref={input}
          value={q}
          onChange={(e) => onQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              onClose();
            }
          }}
          placeholder="搜名字或正文"
          aria-label="搜文档"
          autoCapitalize="off"
          autoCorrect="off"
          spellCheck={false}
          className="min-w-0 flex-1 bg-transparent font-mono text-[16px] text-text outline-none placeholder:text-muted/60 sm:text-[13px]"
        />
        {!narrow && (
          <kbd className="shrink-0 rounded-[3px] bg-surface px-1 py-0.5 font-mono text-[10px] text-muted/70">
            esc
          </kbd>
        )}
      </div>

      <div
        ref={list}
        className={`overflow-y-auto overscroll-contain p-1.5 ${narrow ? "min-h-0 flex-1" : "max-h-[46vh]"}`}
        role="listbox"
        aria-label="文档"
      >
        {rows.length === 0 && (
          <p className="px-2 py-3 text-[12px] leading-relaxed text-muted">
            {q.trim() ? `搜了 ${total} 份，没有命中「${q.trim()}」。` : "这一块还没有文档。"}
          </p>
        )}
        {rows.map((f, i) => (
          <div
            key={f.name}
            data-cur={i === cur ? "1" : undefined}
            className={`flex w-full cursor-pointer items-center gap-2 rounded-md px-2 text-left text-[12.5px] transition-colors duration-100 ${
              narrow ? "min-h-[46px]" : "min-h-[34px]"
            } ${
              i === cur ? "bg-accent text-on-accent" : "text-text/90 hover:bg-surface"
            }`}
            role="option"
            aria-selected={i === cur}
            tabIndex={-1}
            onPointerDown={(e) => {
              // 只防焦点被抢走，不在这里选 —— 选在 onClick。两处都选会打开两次。
              e.preventDefault();
            }}
            onKeyDown={(e) => {
              // 触屏 / 键盘也能走：只有 pointer 的话，Enter 选不中。
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onPick(f.name);
              }
            }}
            onClick={() => onPick(f.name)}
            onMouseEnter={() => onCur(i)}
          >
            {f.protected ? (
              <span
                className={`shrink-0 rounded-[3px] px-1 text-[9.5px] ${
                  i === cur ? "bg-on-accent/20 text-on-accent" : "bg-surface text-muted"
                }`}
              >
                入口
              </span>
            ) : (
              <span
                className={`h-1.5 w-1.5 shrink-0 rounded-full ${i === cur ? "bg-on-accent/60" : "bg-muted/40"}`}
                aria-hidden="true"
              />
            )}
            <span className="min-w-0 flex-1 truncate font-mono text-[12px]">{f.name}</span>
            {f.snippet && (
              <span
                className={`max-w-[45%] shrink-0 truncate text-[10.5px] ${
                  i === cur ? "text-on-accent/75" : "text-muted"
                }`}
              >
                {f.snippet}
              </span>
            )}
          </div>
        ))}
      </div>

      {!narrow && (
        <div className="flex items-center gap-3 border-t border-border px-3 py-1.5 text-[10px] text-muted/60">
          <span>↑↓ 选</span>
          <span>⏎ 打开</span>
          <span>⌘⌫ 删除</span>
          <span className="ml-auto tabular-nums">{rows.length} / {total}</span>
        </div>
      )}
    </div>
  );

  if (narrow) {
    return (
      <div className="dp-fade absolute inset-0 z-30 flex flex-col bg-canvas">
        <div className="flex h-[52px] shrink-0 items-center gap-2 border-b border-border px-3">
          <button
            type="button"
            onClick={onClose}
            className="flex h-7 items-center gap-1 rounded-lg px-1.5 text-[13px] text-accent"
          >
            <svg viewBox="0 0 14 14" className="h-4 w-4" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
              <path d="M8.4 3.4 4.8 7l3.6 3.6" />
            </svg>
            返回
          </button>
          <span className="ml-auto font-mono text-[10.5px] text-muted/60">{total} 份</span>
        </div>
        <div className="flex min-h-0 flex-1 flex-col p-2">{panel}</div>
      </div>
    );
  }

  return (
    <div className="absolute inset-0 z-30" role="presentation">
      <div
        className="absolute inset-0 bg-black/25"
        onPointerDown={onClose}
        aria-hidden="true"
      />
      <div className="pointer-events-none absolute inset-x-0 top-3 flex justify-center px-4">
        <div className="pointer-events-auto">{panel}</div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * ⋯ 菜单：新建 / 改名 / 删 / 恢复。
 *
 * 桌面是贴着那颗按钮弹出来的小面板（`transform-origin` 在按钮上，所以它看起来是
 * 「从 ⋯ 里长出来的」，不是凭空出现在屏幕中间）；手机是从底上升起来的 sheet，
 * 顶部一道抓手——那是在说「能拖、能关」。
 * ------------------------------------------------------------------ */
function Menu({
  narrow,
  docs,
  trash,
  current,
  mode,
  initial,
  err,
  onStartAdd,
  onStartRename,
  onCancelInput,
  onSubmit,
  onDelete,
  onRestore,
  onClose,
}: {
  narrow: boolean;
  docs: DocFile[];
  trash: DocFile[];
  current: string;
  mode: "idle" | "add" | "rename";
  initial: string;
  err: string;
  onStartAdd: () => void;
  onStartRename: (name: string) => void;
  onCancelInput: () => void;
  onSubmit: (v: string) => void;
  onDelete: (name: string) => void;
  onRestore: (name: string) => void;
  onClose: () => void;
}) {
  const input = useRef<HTMLInputElement | null>(null);
  const cur = docs.find((d) => d.name === current);
  const editing = mode !== "idle";

  useEffect(() => {
    if (!editing) return;
    const el = input.current;
    if (!el) return;
    el.focus();
    // 改名时把光标停在名字上、不带扩展名：敲两个字就换完，不用先选中一截。
    el.setSelectionRange(0, el.value.length);
  }, [editing, mode]);

  return (
    <div className="absolute inset-0 z-30" role="presentation">
      <div
        className={`absolute inset-0 ${narrow ? "bg-black/30" : "bg-black/20"} dp-fade`}
        onPointerDown={onClose}
        aria-hidden="true"
      />
      <div
        className={`dp-glass absolute overflow-hidden ${
          narrow
            ? "dp-sheet inset-x-0 bottom-0"
            : "dp-pop right-3 top-[46px] w-[248px] rounded-[14px]"
        }`}
        style={{ transformOrigin: narrow ? "50% 100%" : "100% 0" }}
        role="menu"
        aria-label="文档操作"
      >
        {narrow && <div className="dp-grabber" aria-hidden="true" />}

        <div className={narrow ? "p-2 pb-[max(8px,env(safe-area-inset-bottom))]" : "p-1.5"}>
          {editing ? (
            <div className="p-1">
              <input
                ref={input}
                key={mode}
                defaultValue={mode === "rename" ? initial : ""}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    onSubmit(e.currentTarget.value);
                  }
                  if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    onCancelInput();
                  }
                }}
                placeholder={mode === "rename" ? "新名字，回车确认" : "名字，回车确认"}
                aria-label={mode === "rename" ? "新文档名" : "新文档名"}
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                className="h-8 w-full rounded-md bg-canvas px-2 font-mono text-[16px] text-text outline-none ring-1 ring-border placeholder:text-muted/60 focus-visible:ring-accent sm:text-[12.5px]"
              />
              {err && <p className="px-1 pt-1 text-[11px] text-danger">{err}</p>}
            </div>
          ) : (
            <Row onClick={onStartAdd} icon="plus" label="新建文档" narrow={narrow} />
          )}

          {/* AGENTS.md 这儿两个动词都不给。**照样把它俩列出来、置灰** ——
              「没这功能」和「这份文件不给用」是两件事，界面得说清楚是哪一件。 */}
          {cur?.protected ? (
            <div className={narrow ? "mt-1" : "mt-1 border-t border-border/70 pt-1"}>
              <Row onClick={() => {}} disabled icon="pencil" label="重命名" narrow={narrow} />
              <Row
                onClick={() => {}}
                disabled
                icon="trash"
                label="移到最近删除"
                danger
                narrow={narrow}
              />
              <p className="px-2 pb-1 pt-1.5 text-[10.5px] leading-relaxed text-muted/70">
                AGENTS.md 是 agent 进目录时自动读的那一份：能改内容，改不了名字、删不掉。
              </p>
            </div>
          ) : (
            cur && (
              <div className={narrow ? "mt-1" : "mt-1 border-t border-border/70 pt-1"}>
                <Row
                  onClick={() => onStartRename(cur.name)}
                  icon="pencil"
                  label="重命名"
                  narrow={narrow}
                />
                <Row
                  onClick={() => onDelete(cur.name)}
                  icon="trash"
                  label="移到最近删除"
                  danger
                  narrow={narrow}
                />
              </div>
            )
          )}

          {trash.length > 0 && (
            <div className={narrow ? "mt-1" : "mt-1 border-t border-border/70 pt-1"}>
              <p className="px-2 pb-1 pt-1 text-[10px] tracking-[0.02em] text-muted/60">最近删除</p>
              {trash.map((t) => (
                <Row
                  key={t.name}
                  onClick={() => onRestore(t.name)}
                  icon="restore"
                  label={t.name}
                  mono
                  muted
                  narrow={narrow}
                />
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function Row({
  onClick,
  icon,
  label,
  mono,
  danger,
  muted,
  disabled,
  narrow,
}: {
  onClick: () => void;
  icon: "plus" | "trash" | "restore" | "page" | "pencil";
  label: string;
  mono?: boolean;
  narrow?: boolean;
  danger?: boolean;
  muted?: boolean;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      onClick={onClick}
      disabled={disabled}
      aria-disabled={disabled || undefined}
      className={`flex w-full items-center gap-2 rounded-md px-2 text-left text-[12.5px] transition-[background,color,scale] duration-100 ${
        narrow ? "min-h-[46px]" : "min-h-[34px]"
      } ${
        disabled
          ? "cursor-default text-muted/40"
          : `hover:bg-surface active:scale-[0.985] ${
              danger ? "text-danger" : muted ? "text-muted" : "text-text/90"
            }`
      }`}
    >
      <svg viewBox="0 0 14 14" className="h-3.5 w-3.5 shrink-0" aria-hidden="true" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        {icon === "plus" && <path d="M7 2.8v8.4M2.8 7h8.4" />}
        {icon === "trash" && (
          <>
            <path d="M2.6 4h8.8M5.6 4V2.9c0-.2.2-.4.4-.4h2c.2 0 .4.2.4.4V4" />
            <path d="M4.1 4l.5 7.1c0 .3.3.5.6.5h3.6c.3 0 .6-.2.6-.5L9.9 4" />
          </>
        )}
        {icon === "restore" && (
          <>
            <path d="M3.2 7a3.9 3.9 0 1 1 1.3 2.9" />
            <path d="M3.2 3.6V7h3.4" />
          </>
        )}
        {icon === "pencil" && (
          <>
            <path d="M9.4 2.6 11.4 4.6 5.1 10.9 2.6 11.4 3.1 8.9z" />
            <path d="M8.4 3.6 10.4 5.6" />
          </>
        )}
        {icon === "page" && (
          <>
            <path d="M3.4 2.2h4.6l2.6 2.6v7H3.4z" />
            <path d="M7.8 2.2v2.6h2.6" />
          </>
        )}
      </svg>
      <span className={`min-w-0 flex-1 truncate ${mono ? "font-mono text-[12px]" : ""}`}>{label}</span>
    </button>
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
    <div className="absolute inset-0 z-40 flex items-center justify-center p-5" role="presentation">
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
        <p id="dp-confirm-body" className="px-4 pb-4 pt-1 text-center text-[11.5px] leading-relaxed text-muted">
          <b className="font-mono font-medium text-text">{name}</b> 会挪进{" "}
          <b className="font-mono font-medium text-text">map/&lt;id&gt;/.trash/</b>，
          <br />
          没有真的删掉 —— ⋯ 里能恢复。
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

/** 没载完 / 查无此块时的那一层空壳 —— 和真页面同一个形，不会闪。 */
function Shell({
  onClose,
  crumbs,
  title,
  children,
}: {
  onClose: () => void;
  crumbs: string[];
  title: string;
  children: React.ReactNode;
}) {
  return (
    <div className="absolute inset-0 z-40 flex flex-col bg-canvas text-text">
      <header className="dp-toolbar" data-lifted="false">
        <button type="button" onClick={onClose} aria-label="返回盘面" className="dp-icon">
          <svg viewBox="0 0 16 16" className="h-4 w-4" aria-hidden="true">
            <path d="M9.5 3.5 5 8l4.5 4.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
          </svg>
        </button>
        <span className="dp-crumb ml-1 truncate">{crumbs.join(" › ")}</span>
        <span className="dp-chip ml-auto mr-1">{title}</span>
      </header>
      <div className="dp-scroll">
        <div className="dp-column">{children}</div>
      </div>
    </div>
  );
}
