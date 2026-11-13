// 归档入口真点一遍 —— CDP 驱动一个已经把调试端口开起来的 chromium。
//
// 跟 smoke-templates.mjs 同一套骨架：真浏览器、真宿主（4790）、真引擎。
// 测的都是「点了以后真的发生了什么」：长按出面板、按住高亮、滑出取消、
// 右键开同一个面板、归档库页四态、主列表永远没有「彻底删除」。
//
//   chromium --headless=new --no-sandbox --remote-debugging-port=9231 --user-data-dir=/tmp/cdp about:blank &
//   node test/smoke-archive.mjs
//
// 归档/撕/捞都是真打引擎的 —— 引擎是同一个，动作可逆（撕之前先归档，
// 撕完打捞回来），跑完不留脏数据。
const BASE = process.env.BASE ?? "http://127.0.0.1:4790/p/new1/bjes6b/";
const PORT = Number(process.env.CDP_PORT ?? 9231);

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const json = async (url, init) => (await fetch(url, init)).json();

class Tab {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.waiters = new Map();
    this.events = [];
    ws.addEventListener("message", (e) => {
      const msg = JSON.parse(e.data);
      if (msg.id && this.waiters.has(msg.id)) {
        const { resolve } = this.waiters.get(msg.id);
        this.waiters.delete(msg.id);
        resolve(msg);
      } else if (msg.method) this.events.push(msg.params);
    });
  }
  send(method, params = {}) {
    const id = ++this.id;
    return new Promise((resolve) => {
      this.waiters.set(id, { resolve });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  async eval(expression) {
    const res = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    const detail = res.result?.result;
    if (detail?.exceptionDetails || res.result?.exceptionDetails) {
      throw new Error(JSON.stringify(res.result?.exceptionDetails ?? detail));
    }
    return detail?.value;
  }
}

const cases = [];
const check = (name, got, want) => {
  const ok = JSON.stringify(got) === JSON.stringify(want);
  cases.push({ name, ok, got, want });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}  ${ok ? "" : `got=${JSON.stringify(got)} want=${JSON.stringify(want)}`}`);
};

let targets;
try {
  targets = await json(`http://127.0.0.1:${PORT}/json/list`);
} catch (error) {
  console.log(`SKIP  CDP 端口 ${PORT} 连不上（chromium 没起？）：${error.message}`);
  process.exit(0);
}
const page = targets.find((t) => t.type === "page");
if (!page) { console.log("FAIL  找不到调试目标（chromium 起了吗？）"); process.exit(1); }
const tab = new Tab(new WebSocket(page.webSocketDebuggerUrl));
await new Promise((r) => tab.ws.addEventListener("open", r));
await tab.send("Page.enable");
await tab.send("Runtime.enable");

await tab.send("Page.navigate", { url: `${BASE}?r=${Date.now()}` });
let booted = false;
for (let i = 0; i < 60; i++) {
  booted = await tab.eval("document.querySelectorAll('#sessions .row[data-id]').length > 0");
  if (booted) break;
  await sleep(300);
}
if (!booted) { console.log("FAIL  列表没起来"); process.exit(1); }
await sleep(600);

// 只碰测试探针（名字以 zz- 开头）。没有探针就 SKIP —— 绝不拿列表最后一条真会话
// 做归档 / 撕 / 捞。探针由跑测试的人先建好（见 AGENTS.md 的冒烟小节）。
const pick = `(() => {
  const rows = [...document.querySelectorAll('#sessions .row[data-id]')];
  return rows.find(r => r.querySelector('.row-name span:last-child')?.textContent.startsWith('zz-')) ?? null;
})()`;
const probeId = await tab.eval(`${pick}?.dataset.id ?? null`);
if (probeId == null) {
  console.log("SKIP  列表里没有名字以 zz- 开头的探针会话，不碰真数据");
  process.exit(0);
}

// 记下页面发出的每条请求：撕原件的 bug 是路径里混进了 [object HTMLButtonElement]，
// 只有看真的 fetch URL 才抓得到。
await tab.eval(`(() => {
  if (window.__nousReqs) return;
  window.__nousReqs = [];
  const original = window.fetch;
  window.fetch = function (input, init) {
    try { window.__nousReqs.push(typeof input === 'string' ? input : input.url ?? String(input)); } catch {}
    return original.call(this, input, init);
  };
})()`);

// ── 1. 长按 → 底部动作面板 ─────────────────────────────────────────
{
  const row = await tab.eval(pick);
  check("选中了小白鼠", Boolean(row), true);

  // 快速点一下不误触发
  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:7, pointerType:'touch', clientX:40, clientY:40}))`);
  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerup', {bubbles:true, pointerId:7, pointerType:'touch', clientX:40, clientY:40}))`);
  await sleep(250);
  check("快速点按不出面板", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), false);

  // 长按 0.5s（票上的手感：600ms 后必须已经出面板）
  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:8, pointerType:'touch', clientX:40, clientY:40}))`);
  await sleep(650);
  check("长按 0.5s → 面板滑出", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), true);
  check("遮罩在", await tab.eval("document.querySelector('.sheet-mask')?.classList.contains('on') ?? false"), true);

  // 面板内容：会话名 + 置顶 + 归档 + 撕原件（红）+ 取消；主列表永远不出现「彻底删除」
  check("面板有会话名（灰字）", await tab.eval("!!document.querySelector('.sheet .sheet-cap')?.textContent.trim()"), true);
  const labels = await tab.eval("[...document.querySelectorAll('.sheet button')].map(b => b.textContent.trim())");
  check("面板项：置顶/归档/撕/取消", labels.includes("归档该聊天…") && labels.includes("撕原件…") && labels.some(l => l.includes("置顶")) && labels.includes("取消"), true);
  check("「彻底删除」不出现在面板里", labels.some((l) => l.includes("彻底删除")), false);

  // 撕原件是红色
  check("撕原件标红", await tab.eval("document.querySelector('.sheet .danger')?.textContent.trim()"), "撕原件…");

  // 点遮罩关面板
  await tab.eval("document.querySelector('.sheet-mask').dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:9}))");
  await tab.eval("document.querySelector('.sheet-mask').click()");
  await sleep(260);
  check("点遮罩关面板", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), false);

  // Esc 关面板（有键盘时）
  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:10, pointerType:'touch', clientX:40, clientY:40}))`);
  await sleep(650);
  await tab.eval("document.dispatchEvent(new KeyboardEvent('keydown', {key:'Escape', bubbles:true}))");
  await sleep(260);
  check("Esc 关面板", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), false);
}

// ── 2. 菜单项 pointer 语义：按住高亮、滑出取消、松手执行 ────────────
{
  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:11, pointerType:'touch', clientX:40, clientY:40}))`);
  await sleep(650);
  const r = await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '取消');
    const r = b.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);

  // 按住归档项：高亮
  const arch = await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…');
    const r = b.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…');
    b.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:12, pointerType:'touch', clientX:${arch.x}, clientY:${arch.y}}));
  })()`);
  check("按住菜单项 → 高亮", await tab.eval("[...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…').classList.contains('pressed')"), true);

  // 滑到「取消」上（出项外）：高亮消失
  await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…');
    b.dispatchEvent(new PointerEvent('pointermove', {bubbles:true, pointerId:12, pointerType:'touch', clientX:${r.x}, clientY:${r.y}}));
  })()`);
  check("滑出项外 → 高亮消失", await tab.eval("[...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…').classList.contains('pressed')"), false);

  // 在取消上松手（pointerdown 在前 —— 菜单项的 run 只认它自己 pointerup）→ 面板关、什么都没执行
  await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '取消');
    b.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:12, pointerType:'touch', clientX:${r.x}, clientY:${r.y}}));
  })()`);
  await sleep(50);
  await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '取消');
    b.dispatchEvent(new PointerEvent('pointerup', {bubbles:true, pointerId:12, pointerType:'touch', clientX:${r.x}, clientY:${r.y}}));
  })()`);
  await sleep(300);
  check("松手在取消上 → 面板关", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), false);
}

// ── 3. 桌面右键开同一个面板 ─────────────────────────────────────────
{
  await tab.eval(`(() => {
    const row = ${pick};
    const r = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, cancelable:true, clientX: r.left + 30, clientY: r.top + 20}));
  })()`);
  await sleep(260);
  check("右键 → 同一个底部面板", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), true);
  check("右键面板也是那份菜单内容", await tab.eval(
    "[...document.querySelectorAll('.sheet button')].some(b => b.textContent.trim() === '归档该聊天…')",
  ), true);
  await tab.eval("[...document.querySelectorAll('.sheet button')].find(b => b.textContent.trim() === '取消').click()");
  await sleep(200);

  // 200ms 的收回定时器只许删自己那一对遮罩 / 面板 —— 窗口内重开不许把新的删掉。
  await tab.eval(`(() => {
    const row = ${pick};
    const r = row.getBoundingClientRect();
    // 三次：第一次开 A；第二次关 A（排下 timer1）开 B；第三次关 B（排下 timer2）开 C。
    // 旧实现里 timer2 会按 querySelector 抓到 C 的遮罩删掉。
    for (let i = 0; i < 3; i += 1) {
      row.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, cancelable:true, clientX: r.left + 30, clientY: r.top + 20}));
    }
  })()`);
  await sleep(300);
  check("200ms 内重开 → 只剩一个新面板", await tab.eval("document.querySelectorAll('.sheet').length"), 1);
  check("200ms 内重开 → 新遮罩还在", await tab.eval("document.querySelectorAll('.sheet-mask').length"), 1);
  check("200ms 内重开 → 新面板是打开态", await tab.eval("document.querySelector('.sheet')?.classList.contains('on') ?? false"), true);
  await tab.eval("document.querySelector('.sheet-mask')?.click()");
  await sleep(260);
}

// ── 4. 归档库页 ─────────────────────────────────────────────────────
{
  // 入口：列表页 header.nav 右侧的归档库图标
  await tab.eval("document.querySelector('#open-archive').click()");
  await sleep(900);
  check("进了归档库页", await tab.eval("document.body.dataset.view"), "archive");
  check("归档库页可见", await tab.eval("!document.querySelector('#page-archive').hidden"), true);

  // 分组头：gen 格式化成日期
  check("有 gen 分组头", await tab.eval("document.querySelectorAll('#archive-list .arch-gen').length > 0"), true);

  // 四态：状态标签 + 动作按钮由 archived/live 推
  const states = await tab.eval("[...document.querySelectorAll('#archive-list .arch-state')].map(s => s.dataset.state)");
  check("状态都在四态里", states.every((s) => ["here", "gone", "back", "orphan"].includes(s)), true);
  check("「原件在」的行有撕按钮", await tab.eval("[...document.querySelectorAll('#archive-list .arch-row')].some(r => r.querySelector('.arch-state').dataset.state === 'here' && r.querySelector('.arch-act.purge'))"), true);
  check("「已销毁」的行有打捞按钮", await tab.eval("[...document.querySelectorAll('#archive-list .arch-row')].some(r => r.querySelector('.arch-state').dataset.state === 'gone' && r.querySelector('.arch-act.restore'))"), true);
  check("「已捞回」没有动作按钮", await tab.eval("[...document.querySelectorAll('#archive-list .arch-row')].filter(r => r.querySelector('.arch-state').dataset.state === 'back').every(r => !r.querySelector('.arch-act'))"), true);

  // 归档库的行不开会话面板：右键弹「置顶」会给已归档 id 写 pins.json，等于第二份名单。
  await tab.eval(`(() => {
    const row = document.querySelector('#archive-list .arch-row');
    const r = row.getBoundingClientRect();
    row.dispatchEvent(new MouseEvent('contextmenu', {bubbles:true, cancelable:true, clientX: r.left + 20, clientY: r.top + 10}));
  })()`);
  await sleep(260);
  check("右键归档库的行 → 不开会话面板", await tab.eval("document.querySelector('.sheet') === null"), true);

  // 返回键回列表
  await tab.eval("document.querySelector('#archive-back').click()");
  await sleep(600);
  check("返回 → 列表页", await tab.eval("document.body.dataset.view"), "list");
}

// ── 5. 主列表面板撕原件：路径必须是数字 id，失败必须吐引擎原文 ─────────
{
  const id = await tab.eval(`${pick}.dataset.id`);
  check("小白鼠 id", Number(id) > 0, true);

  // 探针还没归档：直接撕会被引擎 409 挡住。先拿到引擎原话当断言的第二真相。
  const up = await fetch(new URL(`api/sessions/${id}/purge`, BASE), { method: "POST" });
  const upBody = await up.json();
  check("没归档直接撕 → 引擎 409", up.status, 409);

  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:15, pointerType:'touch', clientX:40, clientY:40}))`);
  await sleep(650);
  await tab.eval("[...document.querySelectorAll('.sheet button')].find(b => b.textContent.trim() === '撕原件…').click()");
  await sleep(150);
  check("撕原件出确认层", await tab.eval("document.querySelector('.confirm h3')?.textContent.includes('撕原件')"), true);
  check("确认正文说复印件还在", await tab.eval("document.querySelector('.confirm p')?.textContent.includes('复印件还在')"), true);
  await tab.eval("document.querySelector('.confirm .danger').click()");
  await sleep(700);

  const purgeReqs = await tab.eval("window.__nousReqs.filter(u => /\\/sessions\\/[^/]+\\/purge$/.test(String(u)))");
  check("列表撕原件打到数字 id", purgeReqs.some((u) => new RegExp(`/sessions/${id}/purge$`).test(String(u))), true);
  check("列表撕原件没打 [object …]", purgeReqs.some((u) => String(u).includes("[object")), false);
  check("失败 toast 是引擎原文", await tab.eval("document.querySelector('#toast').textContent"), upBody.error);
  await sleep(900);
}

// ── 6. 归档往返（真打引擎）：小白鼠行 → 归档 → 归档库里出现 → 打捞回来 ──
{
  const id = await tab.eval(`${pick}.dataset.id`);

  // 归档：长按 → 归档该聊天… →（无确认，直接做）行消失 + toast
  await tab.eval(`${pick}.dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:13, pointerType:'touch', clientX:40, clientY:40}))`);
  await sleep(650);
  await tab.eval("[...document.querySelectorAll('.sheet button')].find(b => b.textContent.trim() === '归档该聊天…').dispatchEvent(new PointerEvent('pointerdown', {bubbles:true, pointerId:14, pointerType:'touch', clientX:100, clientY:100}))");
  const a = await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…');
    const r = b.getBoundingClientRect();
    return { x: r.left + r.width / 2, y: r.top + r.height / 2 };
  })()`);
  await tab.eval(`(() => {
    const b = [...document.querySelectorAll('.sheet button')].find(x => x.textContent.trim() === '归档该聊天…');
    b.dispatchEvent(new PointerEvent('pointerup', {bubbles:true, pointerId:14, pointerType:'touch', clientX:${a.x}, clientY:${a.y}}));
  })()`);
  await sleep(400); // toast 只亮 1.4s，先看一眼再等列表重拉
  check("归档有 toast", await tab.eval("!document.querySelector('#toast').hidden && document.querySelector('#toast').textContent.includes('归档')"), true);
  await sleep(1800);
  check("归档后行从列表消失", await tab.eval(`![...document.querySelectorAll('#sessions .row[data-id]')].some(r => r.dataset.id === '${id}')`), true);

  // 归档库里有它（原件在）
  await tab.eval("document.querySelector('#open-archive').click()");
  await sleep(900);
  check("归档库里有小白鼠（原件在）", await tab.eval(
    `[...document.querySelectorAll('#archive-list .arch-row')].some(r => r.dataset.id === '${id}' && r.querySelector('.arch-state').dataset.state === 'here')`,
  ), true);

  // 撕原件：确认弹层（红），确认后状态变「已销毁」
  await tab.eval(`[...document.querySelectorAll('#archive-list .arch-row')].find(r => r.dataset.id === '${id}').querySelector('.arch-act.purge').click()`);
  await sleep(300);
  check("撕原件出确认层", await tab.eval("!!document.querySelector('.confirm')"), true);
  check("确认层标题带会话名", await tab.eval("document.querySelector('.confirm h3')?.textContent.includes('撕原件')"), true);
  check("确认按钮是红的「撕掉原件」", await tab.eval("document.querySelector('.confirm .danger')?.textContent.trim()"), "撕掉原件");
  check("确认层正文说复印件还在", await tab.eval("document.querySelector('.confirm p')?.textContent.includes('复印件还在')"), true);

  // 先点取消，确认它没撕
  await tab.eval("[...document.querySelectorAll('.confirm button')].find(b => b.textContent.trim() === '取消').click()");
  await sleep(300);
  check("取消 → 没撕", await tab.eval(
    `[...document.querySelectorAll('#archive-list .arch-row')].some(r => r.dataset.id === '${id}' && r.querySelector('.arch-state').dataset.state === 'here')`,
  ), true);

  // 再来一次，真撕
  await tab.eval(`[...document.querySelectorAll('#archive-list .arch-row')].find(r => r.dataset.id === '${id}').querySelector('.arch-act.purge').click()`);
  await sleep(300);
  await tab.eval("document.querySelector('.confirm .danger').click()");
  await sleep(1800);
  check("撕完 → 状态「已销毁」", await tab.eval(
    `[...document.querySelectorAll('#archive-list .arch-row')].some(r => r.dataset.id === '${id}' && r.querySelector('.arch-state').dataset.state === 'gone')`,
  ), true);
  check("撕完动作只剩打捞", await tab.eval(`(() => {
    const r = [...document.querySelectorAll('#archive-list .arch-row')].find(r => r.dataset.id === '${id}');
    return !!r.querySelector('.arch-act.restore') && !r.querySelector('.arch-act.purge');
  })()`), true);

  // 打捞回来：状态「已捞回」，主列表里真的回来了
  await tab.eval(`[...document.querySelectorAll('#archive-list .arch-row')].find(r => r.dataset.id === '${id}').querySelector('.arch-act.restore').click()`);
  await sleep(1800);
  check("打捞后 → 状态「已捞回」", await tab.eval(
    `[...document.querySelectorAll('#archive-list .arch-row')].some(r => r.dataset.id === '${id}' && r.querySelector('.arch-state').dataset.state === 'back')`,
  ), true);
  await tab.eval("document.querySelector('#archive-back').click()");
  await sleep(900);
  check("主列表里它回来了", await tab.eval(`[...document.querySelectorAll('#sessions .row[data-id]')].some(r => r.dataset.id === '${id}')`), true);
}

const failed = cases.filter((c) => !c.ok);
console.log(`\n${cases.length - failed.length}/${cases.length} 通过`);
process.exit(failed.length ? 1 : 0);
