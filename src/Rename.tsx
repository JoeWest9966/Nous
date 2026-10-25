import { useRef } from "react";

/**
 * 原地改一个名字。左列和盘面共用这一个 —— 两条入口，一套行为。
 *
 * 提交 = Enter 或失焦，取消 = Escape 或改成空的（空的由 `renameNode` 兜住）。
 * 中文输入法选词时的回车（`isComposing`）不算提交，不然每打一个词就掉出去一次。
 */
export function NameInput({
  value,
  onCommit,
  onCancel,
  className = "",
}: {
  value: string;
  onCommit: (name: string) => void;
  onCancel: () => void;
  className?: string;
}) {
  // Escape 之后输入框会被卸掉，可能还带一次 blur；用个 ref 记下「这次是自己走的」。
  const escaped = useRef(false);

  return (
    <input
      type="text"
      defaultValue={value}
      autoFocus
      spellCheck={false}
      autoComplete="off"
      aria-label="名字"
      className={`min-w-0 rounded-[3px] bg-transparent py-0 leading-none outline-none ${className}`}
      // currentColor 永远有对比度：块上是深色，行里是浅色，不用为每处另配一个环。
      style={{ boxShadow: "inset 0 0 0 1px currentColor" }}
      onFocus={(e) => e.currentTarget.select()}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        // 编辑中一切按键都归输入框：别让 F2 / Delete / ⌘Z 漏到窗口上去。
        e.stopPropagation();
        if (e.key === "Escape") {
          escaped.current = true;
          onCancel();
          return;
        }
        if (e.key !== "Enter" || e.nativeEvent.isComposing) return;
        e.preventDefault();
        e.currentTarget.blur(); // 只有一条提交路径：失焦
      }}
      onBlur={(e) => (escaped.current ? onCancel() : onCommit(e.currentTarget.value))}
    />
  );
}
