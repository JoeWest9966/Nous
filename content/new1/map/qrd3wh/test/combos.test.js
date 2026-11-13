// combos.js 的纯函数：投影、翻开关、试跑前的检查。
//
// 这三件事之所以能裸测：它们不碰仓、不起进程、不认识调度器 —— 写盘和 launch 在 server.js。
import test from "node:test";
import assert from "node:assert/strict";

import { comboView, trialProblem, withEnabled } from "../combos.js";

const okItem = (over = {}) => ({
  name: "morning",
  spec: { trigger: "single-shot", params: { epoch: 1763000000 }, session: "weekly-report" },
  specError: undefined,
  ...over,
});

test("comboView：没写 enabled = 开着（跟 presets.js 一个判定）", () => {
  const view = comboView(okItem());
  assert.equal(view.enabled, true);
  assert.equal(view.name, "morning");
  assert.equal(view.trigger, "single-shot");
  assert.equal(view.session, "weekly-report");
  assert.deepEqual(view.params, { epoch: 1763000000 });
  assert.equal(view.problem, "");
});

test("comboView：enabled:false 照实说；specError 摊到 problem 上", () => {
  const off = comboView(okItem({ spec: { ...okItem().spec, enabled: false } }));
  assert.equal(off.enabled, false);

  const broken = comboView(okItem({ specError: "params.epoch 必填" }));
  assert.equal(broken.problem, "params.epoch 必填");
});

test("comboView：空名册不炸（读不到 = 空表的老规矩）", () => {
  assert.deepEqual(comboView(null), {
    name: "",
    trigger: null,
    params: {},
    session: null,
    enabled: true,
    problem: "",
  });
});

test("withEnabled：只翻 enabled，引用和参数一个字不动", () => {
  const before = okItem().spec;
  const on = withEnabled(before, true);
  assert.deepEqual(on, { ...before, enabled: true });
  const off = withEnabled(before, false);
  assert.deepEqual(off, { ...before, enabled: false });
  // 传的不是 true 就按关处理（HTTP 那边已经分好 enable / disable 两条路）
  assert.equal(withEnabled(before, "yes").enabled, false);
  // 原对象不被改
  assert.equal("enabled" in before, false);
});

test("trialProblem：缺触发器 / 缺会话 / 写坏 / 查无此条，各说各的", () => {
  assert.equal(trialProblem(okItem()), "");
  assert.equal(trialProblem(null), "没有这条终极模板");
  assert.equal(trialProblem(okItem({ specError: "JSON 坏了" })), "JSON 坏了");
  assert.equal(trialProblem(okItem({ spec: { session: "s" } })), "这条没钉触发器");
  assert.equal(trialProblem(okItem({ spec: { trigger: "single-shot" } })), "这条没钉会话模板");
});
