# Nous

## 来历（读代码前先看这条）

Nous 起步时把 [Treemap](https://github.com/JoeWest9966/Treemap) 整个搬过来当底座，基线是
`Treemap@8e208e3`。**这不是 fork，是 vendoring**：不合并上游，需要的东西手动挑过来。
理由和边界见 `docs/adr/0001-vendor-treemap-as-substrate.md`。

所以在 `src/game.ts`、`src/Tree.tsx`、`src/store.ts` 里读到「一盘棋」「块」「组」「这盘棋」
这些词，不是 Nous 的领域语言 —— 那是遗产。Nous 自己的词进 `GLOSSARY.md`。

## Agent skills

### Issue tracker

Issues are tracked in GitHub Issues (via `gh`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five canonical triage labels. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.
