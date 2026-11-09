// 真机回归：SshExecutionEnv 对**编目里的真机器**跑一遍，凭据来自 vq0f03 的 /secret。
//
// 跟 ssh-env.spike.mjs 的区别：那个对 localhost:9551 跑（那台 sshd 已经不在了），
// 这个从编目取真目标 + 真密钥，走的是与会话完全相同的那条路。
//
// 跑法：cd /opt/Nous/content/new1/map/qrd3wh && node ssh-env.real.mjs [机器名]
// 默认机器 9996696（key 型）。密码型的机器这里会明确报「没有密钥」—— 那是预期的。
//
// **密钥与凭据一律不打印。** 只在失败时打印错误码和状态。
import { SshExecutionEnv } from "./ssh-env.js";

const MACHINE = process.argv[2] ?? "9996696";
const QRD = `http://127.0.0.1:${process.env.PORT ?? 4780}${process.env.NOUS_SELF ?? "/"}`;

let pass = 0;
let fail = 0;
const check = (name, cond, extra = "") => {
  if (cond) {
    pass++;
    console.log(`  ✓ ${name}`);
  } else {
    fail++;
    console.log(`  ✗ ${name} ${extra}`);
  }
};

// vq0f03 的地址：跟 server.js 一样，从宿主 __nous/ctx 现找。
const vpsBase = async () => {
  if (process.env.NOUS_VPS_BASE) return process.env.NOUS_VPS_BASE.replace(/\/+$/, "");
  const origin = process.env.NOUS_ORIGIN ?? "http://127.0.0.1";
  const self = process.env.NOUS_SELF ?? "/";
  const res = await fetch(`${origin}${self.endsWith("/") ? self : self + "/"}__nous/ctx`);
  const doc = await res.json();
  const blocks = Array.isArray(doc.blocks) ? doc.blocks : Object.entries(doc.blocks ?? {}).map(([id, b]) => ({ id, ...(b ?? {}) }));
  const vps = blocks.find((b) => b?.id === "vq0f03");
  if (!vps?.url) throw new Error("__nous/ctx 里没有 vq0f03");
  return vps.url.replace(/\/+$/, "");
};

const base = await vpsBase();
const catalog = await (await fetch(`${base}/api/catalog`)).json();
const machine = (catalog?.data?.machines ?? []).find((m) => m.name === MACHINE);
if (!machine) {
  console.error(`编目里没有 ${MACHINE}`);
  process.exit(2);
}
const host = { addr: machine.ips?.[0]?.addr, user: machine.user ?? "root", port: Number(machine.port) || 22 };

const secretRes = await fetch(`${base}/api/vps/${encodeURIComponent(MACHINE)}/secret`);
const secret = (await secretRes.json().catch(() => null))?.data ?? null;
if (!secret?.key) {
  console.error(`${MACHINE} 没有可用密钥（status=${secretRes.status} kind=${secret?.kind ?? "?"}）—— 密码型要先装公钥`);
  process.exit(2);
}
console.log(`── ${MACHINE} @ ${host.addr}:${host.port}（kind=${secret.kind}） ──`);

const env = new SshExecutionEnv({ cwd: "~", host, key: secret.key });
const ctx = { abortSignal: undefined };
const keyDir = env.host.keyPath?.replace(/[\\/][^\\/]+$/, "");
const { statSync } = await import("node:fs");

console.log("── 临时密钥文件 ──");
{
  const mode = statSync(env.host.keyPath).mode & 0o777;
  check("key 临时文件 0600", mode === 0o600, `mode=${mode.toString(8)}`);
  check("key 目录 0700", (statSync(keyDir).mode & 0o777) === 0o700);
}

console.log("── exec ──");
{
  const chunks = [];
  const r = await env.exec("hostname; echo stderr-line >&2", { onOutput: (t, _c, info) => chunks.push(`${info.stream}`) }, ctx);
  check("exit 0", r.ok && r.value.exitCode === 0, JSON.stringify(r).slice(0, 120));
  check("流式回调拿到 stdout", chunks.includes("stdout"), chunks.join(","));
  const bad = await env.exec("false", {}, ctx);
  check("false → 1", bad.ok && bad.value.exitCode === 1);
  const arr = await env.exec(["printf", "%s", "arr-ok"], {}, ctx);
  check("数组形式", arr.ok && arr.value.exitCode === 0);
  const gone = await env.exec("pwd", { cwd: "/no/such/dir" }, ctx);
  check("cwd 不存在 → spawn_error", !gone.ok && gone.error.code === "spawn_error", JSON.stringify(gone.error?.code));
}

console.log("── 读 / 写（真机上的一次往返） ──");
{
  const path = `/tmp/nous-ssh-real-${process.pid}.txt`;
  const w = await env.writeFile(path, "第一行\n二\n", ctx);
  check("writeFile", w.ok, JSON.stringify(w).slice(0, 120));
  const t = await env.readTextFile(path, ctx);
  check("readTextFile 往返一致（含中文）", t.ok && t.value === "第一行\n二\n", JSON.stringify(t.value ?? t.error));
  const lines = await env.readTextLines(path, { maxLines: 1 }, ctx);
  check("readTextLines maxLines=1", lines.ok && lines.value.length === 1, JSON.stringify(lines.value));
  const bin = new Uint8Array([0, 1, 2, 255, 254]);
  const bp = `/tmp/nous-ssh-real-${process.pid}.bin`;
  await env.writeFile(bp, bin, ctx);
  const br = await env.readBinaryFile(bp, ctx);
  check("二进制往返一致", br.ok && Buffer.compare(Buffer.from(br.value), Buffer.from(bin)) === 0);
  const rm = await env.remove(path, { force: true }, ctx);
  const rm2 = await env.remove(bp, { force: true }, ctx);
  check("清理测试文件", rm.ok && rm2.ok);
}

console.log("── 元数据（locale 地雷就在这一节） ──");
{
  const info = await env.fileInfo("/etc/hostname", ctx);
  check("fileInfo kind=file（stat -c %F 不是本地化的）", info.ok && info.value.kind === "file", JSON.stringify(info.error?.code ?? info.value));
  const dir = await env.fileInfo("/tmp", ctx);
  check("fileInfo kind=directory", dir.ok && dir.value.kind === "directory", JSON.stringify(dir.error?.code ?? dir.value?.kind));
  const missing = await env.fileInfo("/no/such/file", ctx);
  check("不存在的 → not_found", !missing.ok && missing.error.code === "not_found", JSON.stringify(missing.error?.code));
  const canon = await env.canonicalPath("/tmp/../tmp", ctx);
  check("canonicalPath", canon.ok && canon.value === "/tmp", JSON.stringify(canon.value ?? canon.error?.code));
  const ex = await env.exists("/etc/hostname", ctx);
  check("exists true", ex.ok && ex.value === true);
  const nex = await env.exists("/no/such", ctx);
  check("exists false", nex.ok && nex.value === false);
  const listed = await env.listDir("/etc", ctx);
  check("listDir 非空", listed.ok && listed.value.length > 10, String(listed.value?.length));
}

console.log("── watch 明确不支持 ──");
{
  const w = await env.watch([{ path: "/tmp" }], () => {}, ctx);
  check("watch → not_supported", !w.ok && w.error.code === "not_supported");
}

await env.cleanup(ctx);
console.log("── 清理 ──");
{
  const { existsSync } = await import("node:fs");
  check("临时密钥目录已删", keyDir ? !existsSync(keyDir) : true, keyDir ?? "");
}

console.log(`\n${pass} 过，${fail} 挂`);
process.exit(fail === 0 ? 0 : 1);
