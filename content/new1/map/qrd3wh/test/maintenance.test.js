// maintenance.js 的测试：阈值判定（纯） + 在真库上量的数（临时库，不碰 data/）。
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { DatabaseSync } from "node:sqlite";

import { inspect, vacuum, vacuumPlan, MIN_FREELIST_PAGES } from "../maintenance.js";

const tempDb = () => path.join(mkdtempSync(path.join(tmpdir(), "maint-")), "agent.sqlite");

/** 造一个真有空洞的库：写一堆行再删掉，页就进了 freelist。 */
const dbWithHoles = (dbPath, rows = 4000) => {
  const db = new DatabaseSync(dbPath);
  db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY, payload TEXT)");
  const insert = db.prepare("INSERT INTO t (payload) VALUES (?)");
  db.exec("BEGIN");
  for (let i = 0; i < rows; i++) insert.run(`row-${i}-`.padEnd(200, "x"));
  db.exec("COMMIT");
  db.exec("DELETE FROM t");
  db.close();
  return dbPath;
};

test("vacuumPlan：页数不够就不做（8MB 以下不值得重写整个库）", () => {
  const plan = vacuumPlan({ pageCount: 1000, freelistCount: MIN_FREELIST_PAGES - 1, pageSize: 4096 });
  assert.equal(plan.should, false);
  assert.equal(plan.ratio, (MIN_FREELIST_PAGES - 1) / 1000);
});

test("vacuumPlan：页数够了但占比低也不做（大库的小空洞）", () => {
  const plan = vacuumPlan({ pageCount: 100000, freelistCount: 3000, pageSize: 4096 });
  assert.equal(plan.should, false);
  // 3000 页空洞 ≈ 11.7MB —— 但只占 3%，不值得锁库
  assert.equal(plan.wasteBytes, 3000 * 4096);
});

test("vacuumPlan：页数和占比都够才做", () => {
  const plan = vacuumPlan({ pageCount: 5000, freelistCount: 3000, pageSize: 4096 });
  assert.equal(plan.should, true);
  assert.equal(plan.ratio, 0.6);
});

test("vacuumPlan：读不到页大小就别编字节数（0 不是「没空洞」）", () => {
  const plan = vacuumPlan({ pageCount: 5000, freelistCount: 3000 });
  assert.equal(plan.pageSize, 0);
  assert.equal(plan.wasteBytes, 0);
  assert.equal(plan.should, true);
});

test("inspect：有表的库读得到页数，没有空洞时不该 VACUUM", () => {
  const dbPath = tempDb();
  const db = new DatabaseSync(dbPath);
  db.exec("CREATE TABLE t (id INTEGER PRIMARY KEY)");
  db.close();
  const plan = inspect({ dbPath });
  assert.ok(plan.pageCount > 0, "建过表就该有页");
  assert.equal(plan.freelistCount, 0);
  assert.equal(plan.should, false);
});

test("inspect → vacuum：空洞清零，freedBytes 说得清回收了多少", () => {
  const dbPath = dbWithHoles(tempDb());
  const before = inspect({ dbPath });
  assert.ok(before.freelistCount > 0, "应该有空洞");
  assert.ok(before.wasteBytes > 0);

  const done = vacuum({ dbPath });
  assert.equal(done.after.freelistCount, 0);
  assert.ok(done.after.pageCount > 0);
  assert.ok(done.freedBytes > 0, "回收量应该 > 0");
  // 跑完再量一次：没有空洞 = 再跑也白跑（幂等）
  assert.equal(inspect({ dbPath }).should, false);
});

test("vacuum：库不存在就抛（调用方决定当错误还是当「还没建」）", () => {
  assert.throws(() => inspect({ dbPath: path.join(tmpdir(), "definitely-not-here.sqlite") }));
});
