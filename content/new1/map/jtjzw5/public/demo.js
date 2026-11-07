// 皮用的示例数据 —— **不是真相，只是让这一页在「那一段还装不了东西」时能看清长相**。
//
// `Ultimate-Template/` 的 `ultimate.json` 形状还没定（`sections.json` 里那段 `checker` 是
// `null`），所以引擎这会儿对这一段回 503。页面照实把 503 说出来，然后拿这几条画——顶上有一条
// 黄条写着「示例数据」，关掉它就是空的。
//
// 形状按**提案**写（一条 = 按顺序叠几条会话预设 + 钉一条提示词模板），五条正好把四种卡状态
// 都用上：ok / unpublished / invalid（引用的预设没了）/ failed（自己写坏了）。
// 形状定下来那一天，这个文件删掉，把 `DEMO_*` 换成接口来的数据就行。

export const DEMO_ROW_KIND = "ultimate";

export const DEMO_ULTIMATES = [
  {
    name: "full-stack",
    summary: "一次开工：先挖问题再写周报，中间的档位和工具都钉好",
    category: ["工程"],
    description: "叠两条预设：deep-dive 负责想清楚，weekly-report 负责写出去。顺序有意义 —— 前面那条定下来的模型和档位，后面那条接着用。",
    presets: ["Session-Template/deep-dive", "Session-Template/weekly-report"],
    prompt: "smart-ass",
    readOnly: false,
    tracked: true,
    specError: null,
  },
  {
    name: "review-only",
    summary: "只读评审：能读能跑，一个字节都不许写",
    category: ["工程", "工程/审查"],
    description: "整条只读。readOnly 是真的落到工具面上的（写工具被摘掉），不是一句提示词。",
    presets: ["Session-Template/read-only-review"],
    prompt: "one-shot",
    readOnly: true,
    tracked: true,
    specError: null,
  },
  {
    name: "blank-slate",
    summary: "白板：一条模板都不要、一条技能都不要，从零开始",
    category: [],
    description: "`prompt: null` 是「明确不要」，跟「没说」（跟随仓库默认）是两件事。",
    presets: ["Session-Template/scratch"],
    prompt: null,
    readOnly: false,
    tracked: false,
    specError: null,
  },
  {
    name: "stale-ref",
    summary: "引用的那条预设被归档了 —— 这条现在开不了",
    category: ["工程"],
    description: "这条自己没写坏：是它引用的 `Session-Template/old-baseline` 不在了。修的地方在那边。",
    presets: ["Session-Template/old-baseline", "Session-Template/deep-dive"],
    prompt: "smart-ass",
    readOnly: false,
    tracked: true,
    specError: null,
  },
  {
    name: "half-written",
    summary: "写了一半的一条 —— 用来试红卡长什么样",
    category: ["写作"],
    description: null,
    presets: ["Session-Template/weekly-report"],
    prompt: "smart-ass",
    readOnly: false,
    tracked: true,
    specError: "ultimate.json 里 `presets` 得是数组，收到 \"Session-Template/weekly-report\"",
  },
];

/** 这一段现在「有哪些预设可叠」 —— 示例模式下也给一份，好让引用状态判得出来。 */
export const DEMO_KNOWN_PRESETS = new Set([
  "Session-Template/deep-dive",
  "Session-Template/weekly-report",
  "Session-Template/scratch",
  "Session-Template/read-only-review",
]);

export const DEMO_KNOWN_PROMPTS = new Set(["smart-ass", "one-shot", "demo-weekly"]);
