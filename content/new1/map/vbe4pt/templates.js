// vbe4pt 车间的四套模板。code(name) 返回文件正文 —— 名字嵌进去，装出去就是那个名字。
// 模板里的注释写给改它的人：标明「改这里」，别写成教科书。

const COMMON_HEAD = (name, what) => `/**
 * ${name} —— ${what}
 *
 * 车间造于 vbe4pt（new1/extend）。装到 ~/.pi/agent/extensions/ 或项目 .pi/extensions/
 * 之后，在 pi 里 /reload 生效。
 */

`;

export const TEMPLATES = {
  guard: {
    label: "守门员",
    desc: "hook tool_call，拦下不想让发生的工具调用（rm -rf、写敏感路径……）",
    note: "守门员模板：tool_call hook。改该拦什么。",
    code: (name) => `${COMMON_HEAD(name, "工具调用守门员")}
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

// 拦什么：正则命中就拦。改这里 —— 这不是沙箱，是减速带。
const BLOCKED: RegExp[] = [
  /rm\\s+-rf\\b/,
  /git\\s+push\\b.*--force/,
  />\\s*\\.env\\b/,
];

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event, ctx) => {
    if (event.toolName !== "bash") return;
    const command = String(event.input?.command ?? "");

    for (const re of BLOCKED) {
      if (re.test(command)) {
        // 想改成「问一下再拦」：ctx.hasUI 时用 ctx.ui.confirm()
        return { block: true, reason: "${name}: 命中红线 " + re.source };
      }
    }
  });
}
`,
  },

  tool: {
    label: "工具",
    desc: "registerTool，LLM 能调用的自定义工具，带 typebox 参数",
    note: "工具模板：registerTool。改参数和 execute。",
    code: (name) => `${COMMON_HEAD(name, "自定义工具")}
import { Type } from "@earendil-works/pi-ai";
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";

const tool = defineTool({
  name: "${name}",
  label: "${name}",
  description: "一句话说清这个工具干什么 —— LLM 靠这个决定用不用它",
  parameters: Type.Object({
    // 改这里：参数。写好 description，LLM 才会填对。
    input: Type.String({ description: "……" }),
  }),

  async execute(_toolCallId, params, _signal, _onUpdate, _ctx) {
    // 改这里：干活的地方。返回的 text 就是给 LLM 看的结果。
    return {
      content: [{ type: "text", text: "收到: " + params.input }],
      details: {},
    };
  },
});

export default function (pi: ExtensionAPI) {
  pi.registerTool(tool);
}
`,
  },

  command: {
    label: "命令",
    desc: "registerCommand，人在输入框里敲 /xxx 触发",
    note: "命令模板：registerCommand。改交互逻辑。",
    code: (name) => `${COMMON_HEAD(name, "斜杠命令 /" + name)}
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  pi.registerCommand("${name}", {
    description: "一句话说清这条命令干什么",
    handler: async (args, ctx) => {
      // 改这里。args 是命令后面的字符串；ctx.ui 有 confirm / select / input / notify。
      ctx.ui.notify("/${name}: " + (args || "没带参数"), "info");
    },
  });
}
`,
  },

  blank: {
    label: "空白",
    desc: "只有入口和 import 行，其余自己写",
    note: "空白模板：自己写。",
    code: (name) => `${COMMON_HEAD(name, "扩展")}
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
  // 改这里。
}
`,
  },
};
