/**
 * 左列的图标包，对应 pele playground 那份 frontmatter：
 *
 *   config:
 *     treeView:
 *       showIcons: true
 *       defaultIconPack: material-icon-theme
 *       filenameIcons:
 *         Dockerfile: docker
 *       extensionIcons:
 *         .ts: typescript
 *         .tsx: react-ts
 *         .txt: none
 *
 * 图标是手绘的内联 SVG，颜色取 pele series 色板（和盘面同一套 Flexoki 值），
 * 不引外部字体包 —— 一张图一个来源。
 */
export type IconName =
  | "folder"
  | "folder-src"
  | "typescript"
  | "react-ts"
  | "docker"
  | "markdown"
  | "file"
  | "none";

const FILENAME_ICONS: Record<string, IconName> = {
  Dockerfile: "docker",
};

const EXTENSION_ICONS: Record<string, IconName> = {
  ".ts": "typescript",
  ".tsx": "react-ts",
};

/** 对齐 frontmatter：`.txt: none` 是显式配置 —— notes.txt 就该没有图标。 */
const EXTENSION_NONE = new Set([".txt"]);

export const iconFor = (name: string, isFolder: boolean): IconName => {
  if (isFolder) return name.replace(/\/$/, "").toLowerCase() === "src" ? "folder-src" : "folder";
  if (FILENAME_ICONS[name]) return FILENAME_ICONS[name]!;
  const dot = name.lastIndexOf(".");
  const ext = dot >= 0 ? name.slice(dot).toLowerCase() : "";
  if (ext in EXTENSION_ICONS) return EXTENSION_ICONS[ext]!;
  if (EXTENSION_NONE.has(ext)) return "none";
  if (ext === ".md") return "markdown";
  return "file";
};

const FOLDER = "#d0a215"; // series-3
const TS = "#4385be"; // series-6
const REACT = "#3aa99f"; // series-5
const GREEN = "#879a39"; // series-4
const INK = "#100f0f";

/** 文件夹带折角：关着是合上的盒，开着是掀开盖的盒（material-icon-theme 的姿态）。 */
const Folder = ({ open, braces }: { open: boolean; braces?: boolean }) => (
  <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0" aria-hidden="true">
    <path
      d={
        open
          ? "M2 3h3.6l1.4 1.5H14a1 1 0 0 1 1 1V13a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z"
          : "M2 3h3.6l1.4 1.5H14a1 1 0 0 1 1 1V13a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z"
      }
      fill={FOLDER}
      opacity={0.55}
    />
    {open ? (
      <path d="M1 13.2 2.8 6.6h12.6l-1.8 6.6a1 1 0 0 1-.96.74H1.96A1 1 0 0 1 1 13.2z" fill={FOLDER} />
    ) : (
      <path d="M2 6.5h12.5V13a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z" fill={FOLDER} />
    )}
    {braces && (
      <text
        x="8"
        y={open ? 12 : 11.6}
        textAnchor="middle"
        fontSize="7"
        fontWeight="700"
        fill={INK}
        fontFamily="var(--n-font-mono)"
      >
        {"{}"}
      </text>
    )}
  </svg>
);

/** 角标型图标（TS）：圆角方块 + 字。 */
const Badge = ({ color, label }: { color: string; label: string }) => (
  <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0" aria-hidden="true">
    <rect x="1.5" y="1.5" width="13" height="13" rx="3" fill={color} />
    <text x="8" y="11.3" textAnchor="middle" fontSize="6.5" fontWeight="700" fill={INK}>
      {label}
    </text>
  </svg>
);

/** react-ts：原子轨道。 */
const ReactIcon = () => (
  <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0" aria-hidden="true">
    <circle cx="8" cy="8" r="1.7" fill={REACT} />
    {[0, 60, 120].map((deg) => (
      <ellipse
        key={deg}
        cx="8"
        cy="8"
        rx="6.4"
        ry="2.5"
        fill="none"
        stroke={REACT}
        strokeWidth="1.1"
        transform={`rotate(${deg} 8 8)`}
      />
    ))}
  </svg>
);

/** docker：三层箱子 + 船身，一眼的事。 */
const Docker = () => (
  <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0" aria-hidden="true">
    <g fill={TS}>
      {[4.4, 6.8, 9.2].map((x) => (
        <rect key={`t${x}`} x={x} y="2.6" width="1.9" height="1.9" rx="0.3" />
      ))}
      {[4.4, 6.8, 9.2].map((x) => (
        <rect key={`b${x}`} x={x} y="4.9" width="1.9" height="1.9" rx="0.3" />
      ))}
      <rect x="2" y="4.9" width="1.9" height="1.9" rx="0.3" />
      <path d="M1 7.6h13.4c-.4 3.3-2.7 5.5-6.4 5.5-3.6 0-6.2-1.9-7-4.6a.9.9 0 0 1 0-.9z" />
    </g>
  </svg>
);

/** markdown：M 加一支下箭头。 */
const Markdown = () => (
  <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0" aria-hidden="true">
    <rect
      x="1"
      y="3"
      width="14"
      height="10"
      rx="2"
      fill="none"
      stroke={GREEN}
      strokeWidth="1.3"
    />
    <path d="M4 10.5V5.8l2 2.4 2-2.4v4.7" fill="none" stroke={GREEN} strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
    <path d="M11.4 5.5v4.4m0 0-1.6-1.6m1.6 1.6 1.6-1.6" fill="none" stroke={GREEN} strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" />
  </svg>
);

/** 兜底文件：一张折了角的纸。 */
const FileIcon = () => (
  <svg viewBox="0 0 16 16" className="h-4 w-4 shrink-0" aria-hidden="true">
    <path
      d="M4 1.5h5.5L13 5v8.5a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1z"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.2"
      strokeLinejoin="round"
    />
    <path d="M9.5 1.5V5H13" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" />
  </svg>
);

/**
 * 锁：`open` 就是开了的那把（能解锁），闭着的是真锁着。
 *
 * 两个宿主都用它：菜单里的一条、左列行尾和盘面块上的那枚小记号。
 * 所以尺寸跟着 `className` 走，不要焊死在这里 —— 菜单里 14px，盘面上只有 9px 也容得下。
 */
export const LockIcon = ({
  open = false,
  className = "h-3.5 w-3.5",
}: {
  open?: boolean;
  className?: string;
}) => (
  <svg
    viewBox="0 0 14 14"
    aria-hidden="true"
    className={className}
    fill="none"
    stroke="currentColor"
    strokeWidth={1.5}
    strokeLinecap="round"
    strokeLinejoin="round"
  >
    <rect x="3.3" y="6.2" width="7.4" height="5.4" rx="1.1" />
    <path d={open ? "M5.2 6.2V4.6a1.8 1.8 0 0 1 3.6 0" : "M5.2 6.2V4.6a1.8 1.8 0 0 1 3.6 0v1.6"} />
  </svg>
);

export function TreeIcon({ name, open }: { name: IconName; open: boolean }) {
  switch (name) {
    case "folder":
      return <Folder open={open} />;
    case "folder-src":
      return <Folder open={open} braces />;
    case "typescript":
      return <Badge color={TS} label="TS" />;
    case "react-ts":
      return <ReactIcon />;
    case "docker":
      return <Docker />;
    case "markdown":
      return <Markdown />;
    case "file":
      return <FileIcon />;
    case "none":
      return null; // extensionIcons: .txt: none —— 配置说了算
  }
}
