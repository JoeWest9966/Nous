// combos —— 终极模板（Ultimate-Template）里那一条「触发器 + 会话模板」的组合。
//
// 为什么要有这一层：`/v1/triggers` 给的是**触发器种类**（single-shot 这种，没有实例、
// 没有状态）；「开着没开」是**组合**自己的事 —— 调度器的 listEnabledCombos 读的也是它。
// 所以启停写回 `preset.json` 的 `enabled`，不另存一份开关表：两份名单迟早对不上。
//
// 这里只放**纯函数**（投影、翻开关、试跑前的检查）。写盘和起会话在 server.js ——
// 那两件事得碰仓和调度器，放这里就得把半个进程注入进来。

/** 一条组合给外面的样子：不塞算出来的状态，只把「为什么跑不起来」摊平。 */
export const comboView = (item) => {
  const spec = item?.spec ?? null;
  return {
    name: String(item?.name ?? ""),
    trigger: spec?.trigger ?? null,
    params: spec?.params ?? {},
    session: spec?.session ?? null,
    // 缺省是开着 —— 跟 presets.js 的判定（`spec.enabled !== false`）一个字不差。
    enabled: spec?.enabled !== false,
    // 写坏了 / 引用没了：这条**还在名册里**（页面要显示它，好让人去改），只是跑不动。
    problem: item?.specError ?? "",
  };
};

/** 启停：只翻 enabled，别的一个字不动（形状是 validatePresetSpec 的事）。 */
export const withEnabled = (spec, enabled) => ({ ...(spec ?? {}), enabled: enabled === true });

/** 手动试跑前的检查：能跑返回空串，不能跑返回原因（页面照原文显示，不再加工）。 */
export const trialProblem = (item) => {
  if (!item) return "没有这条终极模板";
  if (item.specError) return item.specError;
  const spec = item.spec ?? {};
  if (!spec.trigger) return "这条没钉触发器";
  if (!spec.session) return "这条没钉会话模板";
  return "";
};
