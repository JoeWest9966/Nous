import {
  Fragment,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import "./menu.css";

/**
 * 07 号 · 右键 → 浮层菜单。
 *
 * 三个以上动作就不该排一排图标 —— 尤其盘面上的块，30px 见方塞不下四颗按钮。
 * 所以是「右键，弹出一张菜单」，左列的行和盘面上的块共用同一张。
 *
 * **一只鼠标只走右键**：那颗只在 hover 才浮出来的 ••• 是「求你别用」的按钮，
 * 拿掉了。菜单的位置跟鼠标光标走，不跟按钮走。
 *
 * 触屏没有右键 —— 所以 `(hover: none)` 的设备上那颗 ••• 还在（见 NodeMenuTrigger），
 * 它是唯一的入口，不是彩蛋。键盘走 Shift+F10 / 菜单键，落在同一个 handler 里。
 *
 * 菜单是 `position: fixed` 的 —— 块上那个 `overflow: hidden` 切不掉它，
 * 这是菜单能上块的前提。只动 transform / translate / scale / opacity。
 */

export type MenuItem = {
  key: string;
  label: string;
  icon: ReactNode;
  danger?: boolean;
  /** 这一条上面画一条分隔线 */
  separatorBefore?: boolean;
  /**
   * 摆得出来但按不动 —— 不是「藏起来」。
   * 用在锁上：块被上层的组锁住时，「删除」消失得莫名其妙，不如把理由写在那一行。
   */
  disabled?: boolean;
  run: () => void;
};

type Layout = { left: number; top: number; up: boolean; origin: string };

/** 视口坐标。右键给的就是这一个点。 */
type Point = { x: number; y: number };

type Opened = {
  nodeId: string;
  /** 那颗 ••• —— 只有触屏才有。位置跟着它走；没有它就跟 `point` 走 */
  trigger: HTMLElement | null;
  /** 右键落下的位置 */
  point: Point | null;
  /** 行（li）或块（group/leaf）—— 菜单要关掉时得知道该问谁 */
  host: HTMLElement;
  /** Esc 收起来时焦点回哪儿：触屏是那颗 •••，右键是被点的那一块本身 */
  back: HTMLElement | null;
};

type Api = {
  open: (nodeId: string, trigger: HTMLElement, host: HTMLElement) => void;
  openAt: (nodeId: string, host: HTMLElement, at: Point, back: HTMLElement | null) => void;
  close: (back?: boolean) => void;
  activeId: string | null;
};

const Ctx = createContext<Api | null>(null);

export function useNodeMenu(): Api {
  const api = useContext(Ctx);
  if (!api) throw new Error("useNodeMenu 得放在 <NodeMenuProvider> 里");
  return api;
}

/** 和 menu.css 里的 width 对齐 —— 摆位置时得先知道它多宽，不能等它自己说出来。 */
const WIDTH = 188;
const GAP = 6;
const EDGE = 8;
/** 退场时长（menu.css 的 --nm-exit 是 140ms）：动画走完再把节点摘掉。 */
const EXIT = 140;

/** 触屏那颗 •••：右缘对齐按钮，贴着它下面/上面。 */
const layoutOn = (r: DOMRect, height: number): Layout => {
  const maxLeft = Math.max(EDGE, window.innerWidth - WIDTH - EDGE);
  const left = Math.min(Math.max(r.right - WIDTH, EDGE), maxLeft);
  const below = r.bottom + GAP;
  const above = r.top - GAP - height;
  // 下面放不下就翻上去；上面也放不下（太矮的窗口）就哪儿亮就在哪儿待着。
  const up = below + height > window.innerHeight - EDGE && above > EDGE;
  return { left, top: Math.max(EDGE, up ? above : below), up, origin: `100% ${up ? "100%" : "0"}` };
};

/** 右键：右上角落在光标上（原生菜单就是这么摆的），贴边时翻边。 */
const layoutAt = ({ x, y }: Point, height: number): Layout => {
  const maxLeft = Math.max(EDGE, window.innerWidth - WIDTH - EDGE);
  const maxTop = Math.max(EDGE, window.innerHeight - height - EDGE);
  const below = y + GAP;
  const above = y - GAP - height;
  const up = below + height > window.innerHeight - EDGE && above >= EDGE;
  const left = Math.min(Math.max(x, EDGE), maxLeft);
  return {
    left,
    top: Math.min(Math.max(up ? above : below, EDGE), maxTop),
    up,
    // 从光标那一点长出来：横向按光标落在菜单的哪半边定锚点。
    origin: `${Math.min(Math.max(x - left, 0), WIDTH)}px ${up ? "100%" : "0"}`,
  };
};

const placeOf = (cur: Opened, height: number): Layout =>
  cur.trigger ? layoutOn(cur.trigger.getBoundingClientRect(), height) : layoutAt(cur.point!, height);

export function NodeMenuProvider({
  items,
  attr = "data-node-id",
  children,
}: {
  /** 菜单里有哪些条 —— 由调用方决定，它才知道这些动作真去干嘛。 */
  items: (nodeId: string, host: HTMLElement) => MenuItem[];
  /**
   * 菜单认哪个标记。**文档页里嵌了第二层 provider** —— 两层都认 `data-node-id` 的话，
   * 一行文件右键会同时弹出盘面的那张菜单和文档的那张。所以属性名是参数，一个 id 空间一个。
   */
  attr?: string;
  children: ReactNode;
}) {
  const [opened, setOpened] = useState<Opened | null>(null);
  const [shown, setShown] = useState(false);
  const [pos, setPos] = useState<Layout | null>(null);
  const panel = useRef<HTMLDivElement | null>(null);

  // 事件监听是在 effect 里挂的，拿不到最新的 state —— 用一个 ref 兜住「现在开着的是谁」。
  const alive = useRef<Opened | null>(null);
  alive.current = opened;
  const build = useRef(items);
  build.current = items;
  const timer = useRef(0);

  const close = useCallback((back = false) => {
    const cur = alive.current;
    if (!cur) return;
    setShown(false);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setOpened(null), EXIT);
    // 键盘关掉的才把焦点还回去；点条目关掉的不能抢 —— 那边可能已经开了个输入框。
    const to = cur.trigger ?? cur.back;
    if (back && to?.isConnected) to.focus({ preventScroll: true });
  }, []);

  const open = useCallback(
    (nodeId: string, trigger: HTMLElement, host: HTMLElement) => {
      window.clearTimeout(timer.current);
      // 再点一下自己 = 收起来
      if (alive.current?.trigger === trigger) return close();
      setShown(false);
      setPos(null);
      setOpened({ nodeId, trigger, point: null, host, back: trigger });
    },
    [close],
  );

  const openAt = useCallback(
    (nodeId: string, host: HTMLElement, at: Point, back: HTMLElement | null) => {
      window.clearTimeout(timer.current);
      const cur = alive.current;
      // 同一个节点再右键一次：只把它挪到新位置，别让整张菜单重新演一遍进场。
      // `setShown(true)` 是给「右键正按在退场中途」兜底的：`pointerdown` 先把菜单关了（那个
      // 140ms 的 timer 清掉了），这里不把它重新点亮，菜单就永远停在不透明 0。
      if (cur && !cur.trigger && cur.nodeId === nodeId) {
        setPos(layoutAt(at, panel.current?.offsetHeight ?? 140));
        setShown(true);
        return;
      }
      setShown(false);
      setPos(null);
      setOpened({ nodeId, trigger: null, point: at, host, back });
    },
    [],
  );

  /**
   * 右键 + 键盘（Shift+F10 / 菜单键）都从这一个 `contextmenu` 里接住。
   * 浏览器这两种输入发的是同一个事件 —— 分两套只会漂。
   *
   * 只认 `[data-node-id]`：空白画布上右键仍然给原生菜单（检查元素、复制都不该被吃掉），
   * 输入框里也留给浏览器 —— 右键粘贴是它的活，不是我们的。
   */
  useEffect(() => {
    const onCtx = (e: MouseEvent) => {
      const t = e.target as HTMLElement | null;
      if (!t?.closest) return;
      if (t.closest("input, textarea, [contenteditable=''], [contenteditable='true']")) return;
      const el = t.closest(`[${attr}]`) as HTMLElement | null;
      const nodeId = el?.getAttribute(attr);
      if (!el || !nodeId) return;
      e.preventDefault();
      const r = el.getBoundingClientRect();
      // 有的浏览器把 Shift+F10 那一下报成 0,0 —— 那就落在元素自己身上。
      const at =
        e.clientX || e.clientY
          ? { x: e.clientX, y: e.clientY }
          : { x: r.left + 16, y: r.top + Math.min(r.height / 2, 24) };
      openAt(nodeId, el.closest(".nm-host") ?? el, at, el);
    };
    document.addEventListener("contextmenu", onCtx);
    return () => document.removeEventListener("contextmenu", onCtx);
  }, [openAt, attr]);

  // 先量高度再摆位置，量完下一帧才允许它亮 —— 同一帧改位置和 data-open 等于没有进场动效。
  useLayoutEffect(() => {
    if (!opened) return;
    setPos(placeOf(opened, panel.current?.offsetHeight ?? 140));
    const raf = requestAnimationFrame(() => setShown(true));
    return () => cancelAnimationFrame(raf);
  }, [opened]);

  // 触屏那颗 ••• 开着的时候，每帧跟着走。
  // 一开始是「滚一下就关」，结果点开菜单本身会让浏览器 focus 到按钮、顺手滚一下，
  // 菜单刚开就被自己的 scroll 监听关掉了。跟着走没这个问题，而且本来也该跟着走。
  // 右键开的那张钉死在光标上，没有要跟的东西。
  useEffect(() => {
    if (!opened?.trigger) return;
    let raf = 0;
    /** 连续多少帧看不见才算走了。窗口一缩，块还在 300ms 的过渡里 —— 一帧就算数会误杀。 */
    let miss = 0;
    const tick = () => {
      const cur = alive.current;
      if (!cur?.trigger) return;
      const r = cur.trigger.getBoundingClientRect();
      if (!cur.trigger.isConnected || (r.width === 0 && r.height === 0)) {
        close();
        return;
      }
      // 只看上下：横向那些是 overflow:hidden 的事，何况缩窗口时它正好会短暂跑到外面去。
      const gone = r.bottom < 0 || r.top > window.innerHeight;
      miss = gone ? miss + 1 : 0;
      if (miss > 12) {
        close();
        return;
      }
      const next = layoutOn(r, panel.current?.offsetHeight ?? 140);
      setPos((prev) =>
        prev && prev.left === next.left && prev.top === next.top && prev.up === next.up ? prev : next,
      );
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [opened, close]);

  useEffect(() => {
    if (!opened) return;

    const onDown = (e: PointerEvent) => {
      const t = e.target as Node | null;
      if (!t) return;
      if (panel.current?.contains(t) || alive.current?.trigger?.contains(t)) return;
      close();
    };

    // 捕获阶段：别让 F2 / Delete / Escape 漏到下面那些窗口级监听上去。
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        close(true);
        return;
      }
      const list = [...(panel.current?.querySelectorAll<HTMLElement>('[role="menuitem"]') ?? [])];
      if (!list.length) return;
      if (e.key === "Tab") {
        close();
        return;
      }
      const step = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
      if (step) {
        e.preventDefault();
        e.stopPropagation();
        const at = list.indexOf(document.activeElement as HTMLElement);
        const next = list[(at + step + list.length) % list.length];
        next?.focus({ preventScroll: true });
        return;
      }
      if (e.key === "Home" || e.key === "End") {
        e.preventDefault();
        (e.key === "Home" ? list[0] : list[list.length - 1])?.focus({ preventScroll: true });
      }
    };

    document.addEventListener("pointerdown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [opened, close]);

  // 亮出来之后焦点进菜单 —— 键盘用户按下去就该能直接上下选。
  useEffect(() => {
    if (!shown) return;
    panel.current?.querySelector<HTMLElement>('[role="menuitem"]')?.focus({ preventScroll: true });
  }, [shown]);

  const pick = (item: MenuItem) => {
    close();
    if (item.disabled) return;
    item.run();
  };

  return (
    <Ctx.Provider value={{ open, openAt, close, activeId: opened?.nodeId ?? null }}>
      {children}
      {opened &&
        createPortal(
          <div
            ref={panel}
            role="menu"
            aria-label="操作"
            data-open={shown ? "true" : "false"}
            className="nm-menu"
            style={
              pos
                ? { left: pos.left, top: pos.top, transformOrigin: pos.origin }
                : { left: 0, top: 0, visibility: "hidden" }
            }
          >
            {build.current(opened.nodeId, opened.host).map((item, i) => (
              <Fragment key={item.key}>
                {item.separatorBefore && <div className="nm-sep" role="separator" />}
                <button
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  disabled={item.disabled}
                  aria-disabled={item.disabled || undefined}
                  className={item.danger ? "nm-danger" : undefined}
                  style={{ "--i": i } as CSSProperties}
                  onClick={() => pick(item)}
                >
                  {item.icon}
                  {item.label}
                </button>
              </Fragment>
            ))}
          </div>,
          document.body,
        )}
    </Ctx.Provider>
  );
}

/** 这台机器有没有 hover。触屏（`hover: none`）才有那颗 •••，鼠标端它压根不该存在于 DOM 里。 */
function useNoHover(): boolean {
  const [noHover, setNoHover] = useState(
    () => typeof window !== "undefined" && window.matchMedia("(hover: none)").matches,
  );
  useEffect(() => {
    const mq = window.matchMedia("(hover: none)");
    const on = () => setNoHover(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return noHover;
}

/**
 * 那颗 •••。**只在没有右键的设备上存在** —— 鼠标端的一切入口都是右键，
 * 一颗 hover 才浮出来的按钮是「求你别用」的按钮，留着只会跟右键抢位置。
 *
 * 挂着的时候它还是老样子：贴在宿主（行 = li，块 = group/leaf）里面，
 * 显不显示由宿主自己冒不冒泡说 —— 贴着它写 CSS 比在 React 里记 hover 稳。
 */
export function NodeMenuTrigger({
  nodeId,
  kind,
  size = "m",
}: {
  nodeId: string;
  kind: "row" | "block";
  size?: "xs" | "s" | "m";
}) {
  const { open, activeId } = useNodeMenu();
  const noHover = useNoHover();
  const ref = useRef<HTMLButtonElement | null>(null);
  const isOpen = activeId === nodeId;

  if (!noHover) return null;

  return (
    <button
      ref={ref}
      type="button"
      data-no-rename
      aria-haspopup="menu"
      aria-expanded={isOpen}
      aria-label="更多操作"
      title="更多操作"
      data-open={isOpen ? "true" : undefined}
      data-size={kind === "block" ? size : undefined}
      className={kind === "block" ? "nm-dots nm-dots--block" : "nm-dots"}
      onClick={(e) => {
        // 行里点了要钻进去、块里点了要选中 —— 都别发生。
        e.stopPropagation();
        e.preventDefault();
        const btn = ref.current;
        const host = btn?.parentElement;
        if (!btn || !host) return;
        open(nodeId, btn, host);
      }}
      onKeyDown={(e) => {
        if (e.key !== "ArrowDown") return;
        e.preventDefault();
        e.stopPropagation();
        const btn = ref.current;
        const host = btn?.parentElement;
        if (btn && host) open(nodeId, btn, host);
      }}
    >
      <Dots />
    </button>
  );
}

const Dots = () => (
  <svg viewBox="0 0 14 14" aria-hidden="true" fill="currentColor">
    <circle cx="3" cy="7" r="1.25" />
    <circle cx="7" cy="7" r="1.25" />
    <circle cx="11" cy="7" r="1.25" />
  </svg>
);
