// spike：SshExecutionEnv 冒烟 —— 对 localhost:9551（本机 sshd）跑一遍核心方法。
// 跑法：cd /opt/Nous/content/new1/map/qrd3wh && node ssh-env.spike.mjs
import { SshExecutionEnv } from "./ssh-env.js";

const host = { addr: "localhost", port: 9551, user: "root" };
const env = new SshExecutionEnv({ cwd: "/tmp", host });
const ctx = { abortSignal: undefined };

let pass = 0, fail = 0;
const check = (name, cond, extra = "") => {
  if (cond) { pass++; console.log(`  ✓ ${name}`); }
  else { fail++; console.log(`  ✗ ${name} ${extra}`); }
};

console.log("── exec 流式输出 ──");
{
  const chunks = [];
  const r = await env.exec(`echo one; sleep 0.3; echo two; echo three >&2`, {
    onOutput: (text, _c, info) => chunks.push(`${info.stream}:${JSON.stringify(text)}`),
  }, ctx);
  check("exit 0", r.ok && r.value.exitCode === 0, JSON.stringify(r));
  // 每个 chunk 是 `stream:JSON.stringify(text)`，逐个解回来再拼。
  const pick = (stream) => chunks
    .filter((c) => c.startsWith(stream + ":"))
    .map((c) => JSON.parse(c.slice(stream.length + 1)))
    .join("");
  const stdout = pick("stdout");
  const stderr = pick("stderr");
  check("stdout = one\\ntwo\\n", stdout === "one\ntwo\n", JSON.stringify(stdout));
  check("stderr = three\\n", stderr === "three\n", JSON.stringify(stderr));
  check("流式（不止一个 chunk）", chunks.length >= 2, `chunks=${chunks.length}`);
}

console.log("── exec 退出码 ──");
{
  const okR = await env.exec("true", {}, ctx);
  check("true → 0", okR.ok && okR.value.exitCode === 0);
  const badR = await env.exec("false", {}, ctx);
  check("false → 1", badR.ok && badR.value.exitCode === 1);
  const arrR = await env.exec(["printf", "%s", "arr-ok"], {}, ctx);
  check("数组形式", arrR.ok && arrR.value.exitCode === 0);
  const cdR = await env.exec("pwd", {}, ctx);
  check("cwd = /tmp", cdR.ok && cdR.value.exitCode === 0);
}

console.log("── 读 ──");
{
  const r = await env.readTextFile("/etc/hostname", ctx);
  check("readTextFile /etc/hostname", r.ok && r.value.trim().length > 0, JSON.stringify(r));
  const lines = await env.readTextLines("/etc/hostname", { maxLines: 1 }, ctx);
  check("readTextLines maxLines=1", lines.ok && lines.value.length === 1);
  const missing = await env.readTextFile("/no/such/file", ctx);
  check("读不存在 → not_found", !missing.ok && missing.error.code === "not_found", JSON.stringify(missing.error?.code));
}

console.log("── 写（二进制安全） ──");
{
  const bytes = new Uint8Array([0, 1, 2, 255, 254, 10, 13]);
  const w = await env.writeFile("/tmp/ssh-env-spike.bin", bytes, ctx);
  check("writeFile", w.ok, JSON.stringify(w));
  const r = await env.readBinaryFile("/tmp/ssh-env-spike.bin", ctx);
  check("readBinaryFile 二进制一致", r.ok && Buffer.compare(Buffer.from(r.value), Buffer.from(bytes)) === 0);
  const a = await env.appendFile("/tmp/ssh-env-spike.bin", "suffix", ctx);
  check("appendFile", a.ok, JSON.stringify(a));
  const t = await env.readTextFile("/tmp/ssh-env-spike.bin", ctx);
  check("追加后末尾是 suffix", t.ok && t.value.endsWith("suffix"));
}

console.log("── 元数据 ──");
{
  const info = await env.fileInfo("/etc/hostname", ctx);
  check("fileInfo kind=file", info.ok && info.value.kind === "file" && info.value.size > 0, JSON.stringify(info));
  const dirInfo = await env.fileInfo("/tmp", ctx);
  check("fileInfo kind=directory", dirInfo.ok && dirInfo.value.kind === "directory");
  const list = await env.listDir("/tmp", ctx);
  check("listDir 有 ssh-env-spike.bin", list.ok && list.value.some((f) => f.name === "ssh-env-spike.bin"), JSON.stringify(list.value?.slice(0, 3)));
  const ex = await env.exists("/etc/hostname", ctx);
  check("exists true", ex.ok && ex.value === true);
  const nex = await env.exists("/no/such", ctx);
  check("exists false", nex.ok && nex.value === false);
  const canon = await env.canonicalPath("/tmp/../tmp", ctx);
  check("canonicalPath", canon.ok && canon.value === "/tmp", JSON.stringify(canon));
}

console.log("── 增删 + 临时 ──");
{
  const mk = await env.createDir("/tmp/ssh-env-spike-dir/sub", {}, ctx);
  check("createDir -p", mk.ok, JSON.stringify(mk));
  const rm = await env.remove("/tmp/ssh-env-spike-dir", { recursive: true }, ctx);
  check("remove -r", rm.ok, JSON.stringify(rm));
  const td = await env.createTempDir("spike-", ctx);
  check("createTempDir", td.ok && td.value.startsWith("/tmp/spike-"), JSON.stringify(td));
  const tf = await env.createTempFile({ prefix: "f", suffix: ".txt" }, ctx);
  check("createTempFile", tf.ok && tf.value.endsWith(".txt"), JSON.stringify(tf));
}

console.log("── 流式 reader ──");
{
  const reader = await env.openTextLineReader("/etc/hostname", ctx);
  const line = reader.ok ? await reader.value.readLine(ctx) : null;
  check("openTextLineReader 第一行", reader.ok && line.ok && typeof line.value?.text === "string");
  const bin = await env.openBinaryReader("/etc/hostname", {}, ctx);
  const chunk = bin.ok ? await bin.value.read(0, 4, ctx) : null;
  check("openBinaryReader 读 4 字节", bin.ok && chunk.ok && chunk.value.length === 4);
  const dir = await env.openDirReader("/etc", ctx);
  const page = dir.ok ? await dir.value.next(5, ctx) : null;
  check("openDirReader 第一页 5 条", dir.ok && page.ok && page.value.entries.length === 5 && !page.value.done);
}

console.log("── watch ──");
{
  const w = await env.watch([{ path: "/tmp" }], () => {}, ctx);
  check("watch → not_supported", !w.ok && w.error.code === "not_supported", JSON.stringify(w.error?.code));
}

console.log(`\n${pass} 过，${fail} 挂`);
process.exit(fail === 0 ? 0 : 1);
