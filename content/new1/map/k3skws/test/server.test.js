// k3skws 后端测试：tmp 目录 + 假 git 仓，零依赖，不碰 /opt/Nous-Elements。
// 推送一律关掉（NOUS_ELEMENTS_PUSH=0），测试里没有远端可推。

process.env.NOUS_ELEMENTS_PUSH = "0";

import { execFile } from "node:child_process";
import { mkdtemp, readFile, writeFile, mkdir, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { promisify } from "node:util";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";

const sh = promisify(execFile);

/** 造一个假仓库：git init + 初始提交，目录跟真的一样有两个。 */
async function makeRepo() {
  const dir = await mkdtemp(path.join(tmpdir(), "elements-"));
  await sh("git", ["init", "-q", "-b", "main"], { cwd: dir });
  await sh("git", ["config", "user.name", "test"], { cwd: dir });
  await sh("git", ["config", "user.email", "test@example.com"], { cwd: dir });
  await mkdir(path.join(dir, "groups"), { recursive: true });
  await mkdir(path.join(dir, "packages"), { recursive: true });
  await writeFile(path.join(dir, "README.md"), "# 假仓库\n", "utf8");
  await sh("git", ["add", "--", "README.md"], { cwd: dir });
  await sh("git", ["commit", "-q", "-m", "init"], { cwd: dir });
  return dir;
}

const { createApp } = await import("../server.js");

describe("k3skws element 后端", () => {
  let repo;
  let server;
  let base;

  before(async () => {
    repo = await makeRepo();
    server = createApp({ repo });
    await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
    base = `http://127.0.0.1:${server.address().port}`;
  });

  after(async () => {
    await new Promise((resolve) => server.close(resolve));
    await rm(repo, { recursive: true, force: true });
  });

  const put = (kind, body, id = body.id) =>
    fetch(`${base}/v1/item/${kind}${id ? `/${id}` : ""}`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  const del = (kind, id) => fetch(`${base}/v1/item/${kind}/${id}`, { method: "DELETE" });
  const state = async () => (await fetch(`${base}/v1/state`)).json();
  const log = async () => (await sh("git", ["log", "--format=%s", "--reverse"], { cwd: repo })).stdout.trim().split("\n");
  const status = async () => (await sh("git", ["status", "--porcelain"], { cwd: repo })).stdout.trim();

  it("新建一份组：落盘 + 一条提交", async () => {
    const res = await put("group", { id: "frontend", name: "前端", desc: "浏览器里的一切", packages: [] });
    assert.equal(res.status, 201);
    const json = await res.json();
    assert.equal(json.commit.committed, true);
    assert.match(json.commit.head, /^[0-9a-f]{7,}$/);

    const file = path.join(repo, "groups", "frontend.json");
    const onDisk = JSON.parse(await readFile(file, "utf8"));
    assert.deepEqual(onDisk, { id: "frontend", name: "前端", desc: "浏览器里的一切", packages: [] });
    // 两空格缩进 + 结尾换行：手改也好看
    assert.match(await readFile(file, "utf8"), /\n$/);

    assert.deepEqual((await log()).slice(-1), ["feat(group): frontend"]);
    assert.equal(await status(), "");
  });

  it("覆盖已有的一份：200 + created:false", async () => {
    const res = await put("group", { id: "frontend", name: "前端（改名了）", packages: [] });
    assert.equal(res.status, 200);
    const json = await res.json();
    assert.equal(json.created, false);
    assert.equal((await log()).slice(-1)[0], "feat(group): frontend");
    const onDisk = JSON.parse(await readFile(path.join(repo, "groups", "frontend.json"), "utf8"));
    assert.equal(onDisk.name, "前端（改名了）");
  });

  it("未知字段原样透传：一个键都不删、不改名", async () => {
    const body = {
      id: "weird",
      name: "怪字段",
      packages: [],
      什么都能放: { nested: [1, 2, 3] },
      weight: 42,
      enabled: true,
      nullish: null,
    };
    const res = await put("group", body);
    assert.equal(res.status, 201);
    const onDisk = JSON.parse(await readFile(path.join(repo, "groups", "weird.json"), "utf8"));
    assert.deepEqual(onDisk, body);
  });

  it("id 与路径不一致 / id 非法 / 缺 name 都被挡下，且不落盘", async () => {
    const mismatch = await put("group", { id: "aa", name: "x" }, "bb");
    assert.equal(mismatch.status, 400);

    const illegal = await fetch(`${base}/v1/item/group/Not_OK`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id: "Not_OK", name: "x" }),
    });
    assert.equal(illegal.status, 400);

    const noName = await put("group", { id: "no-name" });
    assert.equal(noName.status, 422);

    const escape = await fetch(`${base}/v1/item/group/..%2F..%2Fetc`, {
      method: "PUT",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id: "../etc", name: "x" }),
    });
    assert.equal(escape.status, 400);

    const listed = await state();
    assert.ok(!listed.groups.some((g) => ["aa", "no-name", "Not_OK"].includes(g.id)));
  });

  it("包：正文内联，tags 必须是字符串数组", async () => {
    const ok = await put("package", {
      id: "react-rules",
      name: "React 规范",
      kind: "prompt",
      desc: "…",
      body: "第一，Hooks 顺序。\n第二，依赖数组。",
      tags: ["react"],
    });
    assert.equal(ok.status, 201);

    const badTags = await put("package", { id: "bad-tags", name: "x", tags: [1, 2] });
    assert.equal(badTags.status, 422);

    const badBody = await put("package", { id: "bad-body", name: "x", body: { inline: true } });
    assert.equal(badBody.status, 422);

    const got = await (await fetch(`${base}/v1/item/package/react-rules`)).json();
    assert.match(got.body, /Hooks 顺序/);
  });

  it("并发 6 个写入：全都成功、提交一条不少、工作区干净", async () => {
    const ids = ["c1", "c2", "c3", "c4", "c5", "c6"];
    const results = await Promise.all(ids.map((id) => put("package", { id, name: id, tags: [] })));
    assert.deepEqual(results.map((r) => r.status), [201, 201, 201, 201, 201, 201]);

    const subjects = await log();
    for (const id of ids) assert.ok(subjects.includes(`feat(package): ${id}`), `缺提交：${id}`);
    assert.equal(await status(), "");

    // 每份都是完整 JSON（原子写的意义）
    for (const id of ids) {
      const text = await readFile(path.join(repo, "packages", `${id}.json`), "utf8");
      assert.doesNotThrow(() => JSON.parse(text));
    }
  });

  it("删除：文件没了 + chore 提交", async () => {
    const res = await del("package", "c6");
    assert.equal(res.status, 200);
    await assert.rejects(() => readFile(path.join(repo, "packages", "c6.json"), "utf8"));
    assert.equal((await log()).slice(-1)[0], "chore(package): rm c6");

    const again = await del("package", "c6");
    assert.equal(again.status, 404);
  });

  it("坏 JSON 只标红那一份，列表照出", async () => {
    await writeFile(path.join(repo, "groups", "broken.json"), "{ 这是坏的", "utf8");
    const res = await fetch(`${base}/v1/state`);
    assert.equal(res.status, 200);
    const json = await res.json();
    const broken = json.groups.find((g) => g.id === "broken");
    assert.ok(broken.__broken, "坏文件应带 __broken");
    assert.ok(json.groups.some((g) => g.id === "frontend"), "别的好文件还在");
    await sh("git", ["checkout", "--", "groups/broken.json"], { cwd: repo }).catch(() => {});
    await rm(path.join(repo, "groups", "broken.json"), { force: true });
  });

  it("state 里有仓库信息，静态页出得来", async () => {
    const json = await state();
    assert.equal(json.repo.path, repo);
    assert.match(json.repo.head, /^[0-9a-f]{7,}$/);
    assert.equal(json.kinds.group.dir, "groups");

    const page = await fetch(`${base}/`);
    assert.equal(page.status, 200);
    assert.match(page.headers.get("content-type"), /text\/html/);
  });

  it("手动补推：关推送时老实说没推", async () => {
    const res = await fetch(`${base}/v1/push`, { method: "POST" });
    assert.equal(res.status, 200);
    const data = await res.json();
    assert.equal(data.push.pushed, false);
    assert.match(data.push.error, /推送已关闭/);
    assert.equal(typeof data.repo.head, "string");
  });

  it("未知类别 / 未知接口 都是 4xx，不是 500", async () => {
    assert.equal((await fetch(`${base}/v1/item/skill/x`)).status, 404);
    assert.equal((await fetch(`${base}/v1/nope`)).status, 404);
  });
});
