// 仓库工具收 root 参数：写 A 不许碰 B。
//
// 这几条是「参数化」这件事的全部意义 —— 不是文件里多了个形参，是两个仓真的隔离。
// 反过来说，哪天有人给某个函数加了 `root = EXTEND` 的默认值，这里会红：因为测试压根
// 没设那个环境变量，默认值会把写入领到一棵不存在的树里。

import test from "node:test";
import assert from "node:assert/strict";

import { readFile } from "node:fs/promises";

import * as repo from "../repo-tools.js";
import { cleanup, head, makeRepo, run, status, writeTree } from "./helpers.js";

test("isTrackedDir 按 root 分开：同一个名字在两个仓里各有各的答案", async () => {
  // A：x/ 提交过 → tracked。B：x/ 只在工作区 → 未发布。
  const a = await makeRepo({ files: { "x/SKILL.md": "a\n" } });
  const b = await makeRepo({ files: { "other/SKILL.md": "b\n" } });
  try {
    await writeTree(b, "x/SKILL.md", "b\n");
    assert.equal(await repo.isTrackedDir(a, "x"), true);
    assert.equal(await repo.isTrackedDir(b, "x"), false);
    // 反过来问也一样：各问各的仓，不该串。
    assert.equal(await repo.isTrackedDir(a, "other"), false);
    assert.equal(await repo.isTrackedDir(b, "other"), true);
  } finally {
    await cleanup(a);
    await cleanup(b);
  }
});

test("serializeRepoWrite：同仓串行，两仓互不阻塞", async () => {
  const a = await makeRepo();
  const b = await makeRepo();
  const events = [];
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  try {
    const taskA1 = repo.serializeRepoWrite(a, async () => {
      events.push(["a1", "start"]);
      await sleep(60);
      events.push(["a1", "end"]);
    });
    const taskA2 = repo.serializeRepoWrite(a, async () => {
      events.push(["a2", "start"]);
      await sleep(20);
      events.push(["a2", "end"]);
    });
    const taskB = repo.serializeRepoWrite(b, async () => {
      events.push(["b", "start"]);
      await sleep(10);
      events.push(["b", "end"]);
    });
    await Promise.all([taskA1, taskA2, taskB]);

    const order = events.map(([who, what]) => `${who}:${what}`);
    // A 的两条任务都跑了，而且 A1 先于 A2（同仓排队）。
    assert.deepEqual(
      order.filter((line) => line.startsWith("a")),
      ["a1:start", "a1:end", "a2:start", "a2:end"],
    );
    // B 没等 A 做完 —— 它插在 A1 结束之前就跑完了（两仓各排各的队）。
    const a1End = order.indexOf("a1:end");
    const bEnd = order.indexOf("b:end");
    assert.ok(bEnd !== -1, "B 的任务没跑");
    assert.ok(bEnd < a1End, `B 被 A 挡住了：${order.join(" → ")}`);
  } finally {
    await cleanup(a);
    await cleanup(b);
  }
});

test("syncRepo 只提交自己那个仓：A 的 HEAD 前进，B 的 HEAD 一个字节不动", async () => {
  const a = await makeRepo();
  const b = await makeRepo();
  try {
    await writeTree(a, "x/SKILL.md", "# x\n");
    await writeTree(b, "y/SKILL.md", "# y\n");
    const headBeforeB = await head(b);

    const result = await repo.syncRepo(a, "test: 只有 A 变");
    assert.equal(result.committed, true);
    assert.equal(result.pushed, null); // 没有上游 origin：说实话，不装成功

    assert.notEqual(await head(a), await headBeforeB);
    assert.equal(await head(b), headBeforeB);
    // B 的改动还在工作区里，没人碰它。
    assert.match(await status(b), /^\?\? y\/$/m);
  } finally {
    await cleanup(a);
    await cleanup(b);
  }
});

test("gitChanges 按 root 读：A 的脏文件不会出现在 B 的清单里", async () => {
  const a = await makeRepo();
  const b = await makeRepo();
  try {
    await writeTree(a, "dirty.md", "x\n");
    assert.deepEqual((await repo.gitChanges(a)).map((item) => item.path), ["dirty.md"]);
    assert.deepEqual(await repo.gitChanges(b), []);
  } finally {
    await cleanup(a);
    await cleanup(b);
  }
});

test("regenIndex：跑的是 root 自己的 index.mjs（不是写死的那个）", async () => {
  const generator = `import { writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
await writeFile(path.join(path.dirname(fileURLToPath(import.meta.url)), "index.json"), JSON.stringify({ scanned: true }) + "\\n");
`;
  const a = await makeRepo({ files: { "index.mjs": generator } });
  const b = await makeRepo();
  try {
    const out = await repo.regenIndex(a);
    assert.deepEqual(out, { skipped: false });
    assert.deepEqual(JSON.parse(await readFile(`${a}/index.json`, "utf8")), {
      scanned: true,
    });

    // `required: false`：没有 index.mjs 也照样过（技能仓刚建的时候就是这样）。
    const soft = await repo.regenIndex(b, { required: false });
    assert.deepEqual(soft, { skipped: true });

    // 有生成器但跑挂了 —— 那不是「没有」，那是「校验没过」，必须抛。
    await writeTree(b, "index.mjs", 'console.error("坏了"); process.exit(1);\n');
    await assert.rejects(() => repo.regenIndex(b, { required: false }), /index\.mjs 校验失败/);
  } finally {
    await cleanup(a);
    await cleanup(b);
  }
});

test("moveIntoArchive / moveOutOfArchive 也认 root", async () => {
  const a = await makeRepo({ files: { "x/SKILL.md": "x\n" } });
  const b = await makeRepo({ files: { "x/SKILL.md": "x\n" } });
  try {
    await repo.moveIntoArchive(a, "x");
    assert.equal(await repo.isTrackedDir(a, ".archive/x"), true);
    // B 的 x/ 还在原地，B 也没有 .archive/
    assert.equal(await repo.isTrackedDir(b, "x"), true);
    assert.equal(await repo.isTrackedDir(b, ".archive/x"), false);
    assert.deepEqual((await repo.archivedEntries(a)).map((entry) => entry.name), ["x"]);
    assert.deepEqual(await repo.archivedEntries(b), []);
    // 顺着 A 恢复，不说话
    await repo.moveOutOfArchive(a, "x");
    assert.equal(await repo.isTrackedDir(a, "x"), true);
    assert.equal(await run("git", ["-C", a, "status", "--porcelain"]).then((out) => out.stdout.trim()), "");
  } finally {
    await cleanup(a);
    await cleanup(b);
  }
});

test("改名提交是一条完整改名：A（加）和 D（删）都在同一条 commit 里", async () => {
  // 踩过的坑：`git commit -- <pathspec>` 只拿到目标侧时，HEAD 里会留一个
  // 「新目录有了、旧目录没删」的半截 —— 归档之后就一直是这个姿势，下一个人
  // 一 publish 就把它当删除推上去（或者永远推不上去）。
  const root = await makeRepo({ files: { "x/SKILL.md": "x\n" } });
  try {
    await repo.moveIntoArchive(root, "x");
    const result = await repo.syncRepo(root, "test: 归档 x", { includeDeleted: true });
    assert.equal(result.committed, true);
    const shown = (await run("git", ["show", "--name-status", "--format=", "HEAD"], { cwd: root })).stdout.trim();
    assert.match(shown, /^R100\s+x\/SKILL\.md\s+\.archive\/x\/SKILL\.md$/m);
    assert.equal((await status(root)).trim(), "");
  } finally {
    await cleanup(root);
  }
});
