// Template 那一页真点一遍 —— CDP 驱动一个已经把调试端口开起来的 chromium。
//
// 为什么不是 jsdom：这一页的要害全在「点了以后真的换了屏、真的开了会话、真的滚到了那一组」，
// jsdom 里这些全是假的。所以宁可起一个真浏览器。
//
//   chromium --headless=new --no-sandbox --remote-debugging-port=9231 --user-data-dir=/tmp/cdp about:blank &
//   node test/smoke-templates.mjs
//
// 注意：第 4 步会**真开一条会话**（点一行 = 建会话）。这是故意的 —— 不真开就验不到
// 「认引擎回的生效值」和「开完跳进会话页」。引擎没有删会话的路，跑完会留一条在列表里。
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

const targets = await json(`http://127.0.0.1:${PORT}/json/list`);
const page = targets.find((t) => t.type === "page");
if (!page) { console.log("FAIL  找不到调试目标（chromium 起了吗？）"); process.exit(1); }
const tab = new Tab(new WebSocket(page.webSocketDebuggerUrl));
await new Promise((r) => tab.ws.addEventListener("open", r));
await tab.send("Page.enable");
await tab.send("Runtime.enable");

// 带一个 query 进去：只改 hash 的 navigate 不会真重载，测不到冷启动。
await tab.send("Page.navigate", { url: `${BASE}?r=${Date.now()}` });
let booted = false;
for (let i = 0; i < 60; i++) {
  booted = await tab.eval("location.href.startsWith('" + BASE + "') && document.querySelectorAll('.tab').length === 4");
  if (booted) break;
  await sleep(300);
}
if (!booted) { console.log("FAIL  页面没起来"); process.exit(1); }
await sleep(1200);

// 1. 点 Template tab
await tab.eval(`document.querySelector('.tab[data-tab="templates"]').click()`);
await sleep(1800);
check("点 tab → 视图", await tab.eval("document.body.dataset.view"), "templates");
check("点 tab → 只有模板页可见", await tab.eval(
  `[...document.querySelectorAll('.page')].map(n => n.id + '=' + !n.hidden).join(',')`,
), ["page-list=false", "page-templates=true", "page-thread=false"].join(","));
check("hash", await tab.eval("location.hash"), "#/templates");
check("tab 高亮", await tab.eval(
  `[...document.querySelectorAll('.tab')].map(t => t.dataset.tab + ':' + t.classList.contains('on')).join(',')`,
), "nous:false,templates:true,undefined:false,undefined:false");
check("行数", await tab.eval("document.querySelectorAll('#templates .row.tpl').length"), 4);
check("分组", await tab.eval("[...document.querySelectorAll('#templates .group')].map(g => g.dataset.letter)"), ["D", "R", "S", "W"]);
check("标尺", await tab.eval("[...document.querySelectorAll('#tpl-index span')].map(s => s.textContent)"), ["D", "R", "S", "W"]);
check("行名", await tab.eval("[...document.querySelectorAll('#templates .row-name span:first-child')].map(n => n.textContent)"), ["deep-dive", "read-only-review", "scratch", "weekly-report"]);
check("底行", await tab.eval("document.querySelector('.tpl-foot').textContent"), "4 / 4 条模板");

// 1.5 顶部是「标题 + 右上角两图标」，搜索条不是常驻的。
check("默认无搜索条", await tab.eval("document.querySelector('#tpl-searchbar').hidden"), true);
check("默认不处于搜索态", await tab.eval("document.querySelector('#tpl-nav').classList.contains('searching')"), false);
check("右上角两个图标", await tab.eval("document.querySelectorAll('#tpl-nav .nav-side.end .nav-icon').length"), 2);
check("放大镜图标存在", await tab.eval("!!document.querySelector('#tpl-open-search svg')"), true);

// 2. 搜索（点放大镜才出搜索条；内存里筛，不再发请求）
const reqs = await tab.eval("performance.getEntriesByType('resource').filter(r => r.name.includes('/api/presets')).length");
await tab.eval("document.querySelector('#tpl-open-search').click()");
await sleep(300);
check("点放大镜 → 搜索态", await tab.eval("document.querySelector('#tpl-nav').classList.contains('searching')"), true);
check("点放大镜 → 搜索条可见", await tab.eval("!document.querySelector('#tpl-searchbar').hidden"), true);
check("点放大镜 → 框被聚焦", await tab.eval("document.activeElement === document.querySelector('#tpl-q')"), true);
await tab.eval("(q => { const n = document.querySelector('#tpl-q'); n.value = q; n.dispatchEvent(new Event('input')); })('周报')");
await sleep(300);
check("搜「周报」→ 1 行", await tab.eval("document.querySelectorAll('#templates .row.tpl').length"), 1);
check("搜「周报」→ 组", await tab.eval("[...document.querySelectorAll('#templates .group')].map(g => g.dataset.letter)"), ["W"]);
check("搜「周报」→ 底行", await tab.eval("document.querySelector('.tpl-foot').textContent"), "1 / 4 条模板");
const reqs2 = await tab.eval("performance.getEntriesByType('resource').filter(r => r.name.includes('/api/presets')).length");
check("筛选不发请求", reqs2 - reqs, 0);
await tab.eval("(q => { const n = document.querySelector('#tpl-q'); n.value = q; n.dispatchEvent(new Event('input')); })('不存在的名字')");
await sleep(300);
check("没命中", await tab.eval("document.querySelector('#templates .empty')?.textContent"), "没有「不存在的名字」");
await tab.eval("document.querySelector('#tpl-cancel-search').click()");
await sleep(300);
check("点取消 → 退出搜索态", await tab.eval("document.querySelector('#tpl-nav').classList.contains('searching')"), false);
check("点取消 → 搜索条藏起", await tab.eval("document.querySelector('#tpl-searchbar').hidden"), true);
check("点取消 → 列表回满", await tab.eval("document.querySelectorAll('#templates .row.tpl').length"), 4);
check("点取消 → 框清空", await tab.eval("document.querySelector('#tpl-q').value"), "");

// 3. 标尺跳位
await tab.eval(`document.querySelector('#templates').scrollTop = 0`);

await tab.eval("(() => { const s = [...document.querySelectorAll('#tpl-index span')].find(x => x.dataset.letter === 'W'); const r = s.getBoundingClientRect(); s.dispatchEvent(new PointerEvent('pointerdown', { clientY: r.top + r.height / 2, clientX: r.left, bubbles: true })); })()");
await sleep(300);
check("按 W 滚下去了", await tab.eval("document.querySelector('#templates').scrollTop > 0"), true);
check("W 高亮", await tab.eval("[...document.querySelectorAll('#tpl-index span')].find(x => x.dataset.letter === 'W').classList.contains('on')"), true);

// 4. 点一行 → 真开一条会话 → 进会话页
const rowsBefore = await tab.eval("document.querySelectorAll('#templates .row.tpl').length");
await tab.eval(`[...document.querySelectorAll('#templates .row.tpl')].find(r => r.dataset.name === 'scratch').click()`);
await sleep(2500);
check("进了会话页", await tab.eval("document.body.dataset.view"), "thread");
check("会话页藏底栏", await tab.eval("getComputedStyle(document.querySelector('.tabbar')).display"), "none");
check("会话页两页都藏", await tab.eval("[document.querySelector('#page-list').hidden, document.querySelector('#page-templates').hidden]"), [true, true]);
check("标题带上时间", await tab.eval("/^scratch \\d\\d-\\d\\d \\d\\d:\\d\\d$/.test(document.querySelector('#thread-title').textContent)"), true);
check("hash 是会话", await tab.eval("/^#\\/c\\/\\d+$/.test(location.hash)"), true);
check("输入条回来了", await tab.eval("!!document.querySelector('#composer')"), true);

// 5. 后退 → 列表；列表里应该有刚才那条
await tab.eval(`location.hash = '#/'`);
await sleep(2000);
check("回列表", await tab.eval("document.body.dataset.view"), "list");
check("底栏回来了", await tab.eval("getComputedStyle(document.querySelector('.tabbar')).display"), "flex");
check("列表里有新建的", await tab.eval("[...document.querySelectorAll('#sessions .row-name span')].some(s => s.textContent.startsWith('scratch '))"), true);
check("nous tab 高亮", await tab.eval("[...document.querySelectorAll('.tab')].find(t => t.dataset.tab === 'nous').classList.contains('on')"), true);

// 6. 模板页回来时不该再闪白（数据在内存里）
await tab.eval(`document.querySelector('.tab[data-tab="templates"]').click()`);
await sleep(1200);
check("模板还在", await tab.eval("document.querySelectorAll('#templates .row.tpl').length"), rowsBefore);
const reqs3 = await tab.eval("performance.getEntriesByType('resource').filter(r => r.name.includes('/api/presets')).length");
check("重进这一页会重拉一次（就一次）", reqs3 - reqs2, 1);

// 7. 冷启动直接进 #/templates
await tab.send("Page.navigate", { url: `${BASE}?r=${Date.now()}#/templates` });
await sleep(2500);
check("冷启动进模板页", await tab.eval("document.body.dataset.view"), "templates");
check("冷启动有行", await tab.eval("document.querySelectorAll('#templates .row.tpl').length"), 4);

const failed = cases.filter((c) => !c.ok);
console.log(`\n${cases.length - failed.length}/${cases.length} 通过`);
process.exit(failed.length ? 1 : 0);
