/**
 * Worktree 守卫 —— 共享检出（主 worktree）写闸
 *
 * 共享检出同时是每个新 session 的 cwd。在它上面直接写 = 并发写手互相踩，
 * 而且谁在改什么，别人看不见 —— 先例：`feat/109-parts-catalog` 的全部工作
 * 曾以未提交状态躺在共享检出里（该分支自身零提交），任何 session 一个
 * `git checkout` 就能把它抹掉。
 *
 * 纪律全文写在 AGENTS.md「Worktree 纪律（一票一 worktree）」。这个扩展是那条
 * 纪律的**执行面** —— 文档三天后会被忽略，闸不会。
 *
 * 判定：`git rev-parse --absolute-git-dir` == `git rev-parse --git-common-dir`
 * ⇒ 当前处于主 worktree（共享检出）；不等 ⇒ 链接 worktree（正经干活的地方）。
 * 链接 worktree 的 git-dir 是 `<common>/worktrees/<名>`，主 worktree 的就是
 * `<common>` 本身。这个判据与仓库、路径、clone 位置全都无关。
 *
 * 拦什么：
 *   - `write` / `edit`：按**目标路径**判（不是 cwd）—— 目标落在主 worktree 里
 *     就拦，哪怕这个 session 自己住在链接 worktree 里。
 *   - `bash`：只看 cwd + 一批写模式。这**不是沙箱，是减速带** —— 解释器一行
 *     （`python3 -c "open('x','w')"`）就能绕，故意的。要防的是「顺手在里面改」，
 *     不是防有心人。
 *   - `subagent` / `workflowScript`：管不着（子会话有自己的 cwd）。要堵得在
 *     派活那侧堵。
 *
 * 放行：`NW_ALLOW_SHARED_WRITE=1` —— 整段关闸（开工时会闹一声提醒）。
 * 正经的出路不是放行，是建树：`ci/nw-work.sh <票号>`。
 */

import { spawnSync } from "node:child_process";
import { realpathSync } from "node:fs";
import { dirname, isAbsolute, resolve } from "node:path";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

export type WorktreeKind = "main" | "linked" | "none";

/** 判定某个路径落在哪种 worktree 里。不进任何仓 ⇒ "none"。 */
export function classifyWorktree(startPath: string): WorktreeKind {
	let dir = resolve(startPath);
	// 目标可能还不存在（write 新文件）：向上找到第一个存在的祖先目录再问 git。
	for (let i = 0; i < 40; i++) {
		const r = spawnSync(
			"git",
			["-C", dir, "rev-parse", "--absolute-git-dir", "--git-common-dir"],
			{ encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
		);
		if (r.status === 0) {
			const [gitDir, commonDir] = r.stdout.trim().split("\n");
			if (!gitDir || !commonDir) return "none";
			try {
				// 主 worktree 的 --git-common-dir 是相对路径（".git"），链接的是绝对路径。
				const common = isAbsolute(commonDir) ? commonDir : resolve(dir, commonDir);
				const a = realpathSync(gitDir);
				const b = realpathSync(common);
				return a === b ? "main" : "linked";
			} catch {
				return "none";
			}
		}
		const parent = dirname(dir);
		if (parent === dir) break;
		dir = parent;
	}
	return "none";
}

/** bash 里的写模式。宁可漏（有心人绕得掉），不可滥杀（把只读操作也拦了）。 */
const BASH_WRITE_PATTERNS: Array<[RegExp, string]> = [
	[/(?<![-=])>>?(?![&=])/, "重定向 (> / >>)"],
	[/\|\s*tee\b/, "tee"],
	[/\bsed\s+-i\b/, "sed -i"],
	[/\bperl\s+-i\b/, "perl -i"],
	[
		/\bgit\s+(add|commit|checkout|switch|stash|reset|restore|clean|merge|rebase|cherry-pick|apply|am|rm|mv|update-ref)\b/,
		"git 写操作",
	],
	[/^\s*(mkdir|touch|rm|mv|cp|install|truncate|dd|ln|chmod|chown)\b/m, "文件系统写"],
	[
		/\b(go\s+mod\s+tidy|npm\s+(i|install|ci)|pnpm\s+(i|install|add)|yarn\s+(add|install)|pip3?\s+install|cargo\s+(add|install|fix))\b/,
		"依赖安装 / 构建",
	],
	[/\bgo\s+build\s+-o\b/, "构建产物落盘"],
	[/\b(vim?|nano|emacs|code)\s/, "编辑器"],
];

export function findBashWritePattern(command: string): string | null {
	for (const [re, label] of BASH_WRITE_PATTERNS) {
		if (re.test(command)) return label;
	}
	return null;
}

const HOW_TO_ESCAPE = [
	"共享检出是只读的（主 worktree）—— 在这里写 = 并发写手互相踩，而且谁在改什么，别人看不见。",
	"",
	"自己的 worktree 才是干活的地方（树位与分支名照惯例）：",
	"  ci/nw-work.sh <票号>              # 查票 + 认领 + 建树 + 直接进去开工（推荐）",
	"  git worktree add /opt/nw-<票号> -b <type>/<票号>-<slug> origin/master   # 手动版",
	"",
	"（真需要在这里写：NW_ALLOW_SHARED_WRITE=1 重开会话，整段关闸。）",
].join("\n");

export default function (pi: ExtensionAPI) {
	const guardOff = process.env.NW_ALLOW_SHARED_WRITE === "1";
	let announced = false;

	const announce = (ctx: ExtensionContext, message: string, level: "info" | "warning") => {
		if (!ctx.hasUI) return;
		ctx.ui.notify(message, level);
	};

	pi.on("session_start", async (_event, ctx) => {
		if (classifyWorktree(ctx.cwd) !== "main") return;
		if (guardOff) {
			announce(ctx, "worktree 守卫：已由 NW_ALLOW_SHARED_WRITE=1 关闭 —— 你现在在共享检出里写。", "warning");
			ctx.ui.setStatus("worktree-guard", "⚠ 共享检出 · 闸已关");
			return;
		}
		announce(ctx, "你现在在共享检出（主 worktree）里，它只读。要开工：ci/nw-work.sh <票号>", "warning");
		ctx.ui.setStatus("worktree-guard", "⚠ 共享检出 · 只读");
	});

	pi.on("tool_call", async (event, ctx) => {
		if (guardOff) return;

		const input = (event.input ?? {}) as { path?: string; command?: string };

		// 写文件类工具：按目标路径判 —— 哪怕本 session 住在链接 worktree 里，
		// 目标指向共享检出也照样拦。
		if (event.toolName === "write" || event.toolName === "edit") {
			const target = input.path;
			if (typeof target !== "string" || target.length === 0) return;
			const abs = isAbsolute(target) ? target : resolve(ctx.cwd, target);
			if (classifyWorktree(dirname(abs)) !== "main") return;
			if (!announced) {
				announced = true;
				announce(ctx, `拦下：${abs} 在共享检出里。`, "warning");
			}
			return { block: true, reason: HOW_TO_ESCAPE };
		}

		// bash：只能看 cwd + 模式。cwd 不在共享检出就放行。
		if (event.toolName === "bash") {
			const command = input.command;
			if (typeof command !== "string" || command.length === 0) return;
			// 明确的只读 git 子命令先放行（`git log` 里带 "add" 之类不该误杀）。
			const label = findBashWritePattern(command);
			if (!label) return;
			if (classifyWorktree(ctx.cwd) !== "main") return;
			return { block: true, reason: `拦下（${label}）：${HOW_TO_ESCAPE}` };
		}
	});
}
