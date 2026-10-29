import { type Components } from "react-markdown";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";

/**
 * 一份 markdown 的**渲染** —— 文档页预览模式里那张纸。
 *
 * 它只做一件事：把 `doc.text` 变成看得懂的版面（标题、列表、表格、代码、引用、勾选框），
 * 和 GitHub 上看一份 `.md` 差不多。规矩三条：
 *
 * - **不认识的东西一律不执行。** 不开 `rehype-raw`，还显式 `skipHtml`：
 *   文档里写一段 `<script>`，在预览里就是个不存在的东西 —— 它是文档，不是页面。
 * - **不让内容绑架版面。** 表格、代码块各自横向滚，页面本身不跟着横着跑。
 * - **外链是新标签页。** 点一个链接不该把这一页（连同左列、草稿、滚动位置）端掉。
 *
 * 它是**懒加载**的一份（`React.lazy`，见 `Doc.tsx`）：默认是源码模式，
 * 一辈子只看源码的人不该为这个渲染器付首包的体积。
 */
export default function DocMarkdown({ text }: { text: string }) {
  return (
    <div className="dp-prose dp-fade">
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={PARTS} skipHtml>
        {text}
      </ReactMarkdown>
    </div>
  );
}

/**
 * 少数几处要自己接手的标签。剩下的（标题、段落、列表、引用、代码）
 * 由 `.dp-prose` 的元素选择器管 —— 那些标签不需要在 React 这一层被改写成别的什么东西。
 */
const PARTS: Components = {
  a({ node: _n, href, children, ...rest }) {
    const out = !!href && /^(https?:)?\/\//i.test(href);
    return (
      <a
        href={href}
        {...rest}
        {...(out ? { target: "_blank", rel: "noreferrer noopener" } : {})}
        title={out ? href : undefined}
      >
        {children}
      </a>
    );
  },

  // GFM 的表格自己不带滚动容器：一张宽表会把整页撑开，所以给它套一格能横滚的。
  table({ node: _n, children, ...rest }) {
    return (
      <div className="dp-prose-scroll">
        <table {...rest}>{children}</table>
      </div>
    );
  },

  // 图片：能缩就缩，不撑破版心；懒加载 —— 一份长文档里的第二十张图不该卡住第一屏。
  img({ node: _n, alt, ...rest }) {
    return <img {...rest} alt={alt ?? ""} loading="lazy" />;
  },

  // 任务列表的勾选框：**看得见、点不动**。这里是预览，改它等于改文件，
  // 而改文件的入口只有一个（源码模式）。React 也不喜欢一个没人接 onChange 的 checked。
  //
  // 不用 `disabled`：禁用态会把 accent 色洗成一块死灰，那看起来像「坏了」。
  // `readOnly` + 指针穿透 —— 照样点不动，但颜色还是正文里那个颜色。
  input({ node: _n, disabled: _d, ...rest }) {
    return <input {...rest} readOnly />;
  },
};
