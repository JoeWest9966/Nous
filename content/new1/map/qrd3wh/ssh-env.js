// SshExecutionEnv —— 把 pi-durable 的 ExecutionEnv 接口翻译成 `ssh` 命令。
//
// 为什么用 `ssh` CLI 而不是 ssh2 库：
//   · 尊重宿主的 `~/.ssh/config`（跳板、ControlMaster、known_hosts）—— 密钥本来就是
//     「宿主机自己的事」，vq0f03 编目里明确不存密钥。
//   · 零新依赖。qrd3wh 已经钉死 pi-durable 1.4.0，再拖一个 ssh2 进来只会多一份会漂的东西。
//
// 传输约定（四种原语，别的都从它们搭）：
//   · 执行：`ssh -p port user@addr 'command'`，字符串走远端 shell，数组逐个 shell-quote 后拼。
//   · 读文件：远端 `base64 -w0 < path`，本地解码 —— 二进制安全，也躲开远端 locale 的编码坑。
//   · 写文件：`ssh … 'mkdir -p dir && cat > path'`，字节从 stdin 灌进去 —— 不经过 shell，二进制安全。
//   · 元数据：`stat -c '%F|%s|%Y'` 和 `find -printf '%y\t%f\t%s\t%T@'`（Linux 目标，都是 GNU coreutils）。
//
// 边界（跟 NodeExecutionEnv 的差别，都是刻意为之）：
//   · `watch` 返回 not_supported —— 远端没有可靠的 fs 事件，pi-durable 自己写着「旧远端 daemon
//     就用 not_supported，调用方退回到自己的 walk」。
//   · 流式 reader 在 open 时把整个文件/目录读进内存 —— 每次操作一个新 ssh 连接，不做持久连接。
//     大文件会占内存，但工具调用读的是代码文件，不是 iso。真遇到大文件再加分块。
//
// 凭据（`key`）：只在构造时过一手 —— 写进 `mkdtemp` 建的 0700 临时目录里一个 0600 的文件，
// 交给 `ssh -i`，`cleanup()` / 进程退出时整个目录删掉。**不落块的文件夹、不进日志。**
// `key` 缺省时行为跟以前一模一样（全靠宿主 ~/.ssh）。
//
// 这个文件不 import pi-durable（跟 Nous-extend 扩展同一规矩）：只 import 它的**类型**以外的
// `env/index.js` 里的 ok/err/ExecutionError/FileError/StreamDecoder —— 那些是纯函数，不是宿主能力。
// 等一下 —— 为了不在 qrd3wh 里再实现一遍 Result/FileError，这里直接 import 运行时：
// pi-durable 的 env/index.js 是 ESM，`ok`/`err`/`FileError`/`ExecutionError`/`StreamDecoder`
// 都是导出。qrd3wh 的 server.js 已经 import 了 `@earendil-works/pi-durable/env/node`，
// 所以这条链是通的。

import { spawn } from "node:child_process";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
// 路径一律用 **posix**：这些是**远端**路径，不能借宿主的路径语义。
// 用 `node:path` 的话，宿主一旦是 Windows，`resolve("/a","b")` 就变成 `C:\\a\\b` ——
// 每个文件操作都静默错。块是「只要 Node 就能跑」的，这条不能赌。
import { basename, isAbsolute, join, resolve } from "node:path/posix";
import { ExecutionError, FileError, StreamDecoder, err, ok } from "@earendil-works/pi-durable/env";

// ── 临时密钥文件 ──────────────────────────────────────────────────────────────
// 装着 key 的目录，进程退出时兜底清一遍（cleanup 不一定被调到）。
const KEY_DIRS = new Set();
process.once("exit", () => {
  for (const dir of KEY_DIRS) {
    try { rmSync(dir, { recursive: true, force: true }); } catch {}
  }
});

/** 把明文 PEM 写成 0600 的临时文件，返回路径。目录 0700，进程退出即删。 */
function materializeKey(key) {
  const dir = mkdtempSync(join(tmpdir(), "nous-ssh-"));
  KEY_DIRS.add(dir);
  const file = join(dir, "id");
  // mode 只在**创建**时生效，而 ssh 死磕权限位，所以再 chmod 一次掩住。
  writeFileSync(file, key.endsWith("\n") ? key : `${key}\n`, { mode: 0o600 });
  chmodSync(file, 0o600);
  return file;
}

// ── shell 引用 ────────────────────────────────────────────────────────────────
// POSIX 单引号：`'` → `'\''`。任何字符串都能安全塞进 sh -c 的单引号里。
// 唯一例外：`~` 要留给远端 shell 展开成 home（单引号会把它框死），换成 `$HOME`。
const shq = (s) => {
  const t = String(s);
  if (t === "~") return "$HOME";
  if (t.startsWith("~/")) return `$HOME${shq(t.slice(1))}`;
  return `'${t.replace(/'/g, "'\\''")}'`;
};

// ── 远程命令原语 ──────────────────────────────────────────────────────────────

/** 一台机器的 ssh 目标。addr 用 IP/tailnet 名，user/port 从编目来。
 *  `host.keyPath` 有值时用它当唯一身份（IdentitiesOnly），否则全靠宿主 `~/.ssh`。 */
export const sshHostArgs = (host) => [
  "-p", String(host.port ?? 22),
  "-o", "BatchMode=yes",
  "-o", "ConnectTimeout=10",
  "-o", "StrictHostKeyChecking=accept-new",
  ...(host.keyPath ? ["-i", host.keyPath, "-o", "IdentitiesOnly=yes"] : []),
  `${host.user ?? "root"}@${host.addr}`,
];

/** 跑一条远端命令，stdout/stderr 整块收回来。文件操作用的，不流式。
 *  强制 `LC_ALL=C`：`stat -c %F` 的输出是**按 locale 翻译**的（中文 locale 下是「普通文件」），
 *  错误措辞也会跟着变，`remoteFileError` 就分不出 not_found。C 之下只认一份措辞。
 *  `exec` 不走这里 —— 那是用户/agent 自己的命令，不该改它的 locale。 */
function runRemote(host, command, { stdin } = {}) {
  return new Promise((resolve) => {
    const args = [...sshHostArgs(host), `LC_ALL=C ${command}`];
    let child;
    try {
      child = spawn("ssh", args, {
        stdio: [stdin === undefined ? "ignore" : "pipe", "pipe", "pipe"],
        windowsHide: true,
      });
    } catch (error) {
      resolve({ status: null, stdout: "", stderr: String(error?.message ?? error), signal: null });
      return;
    }
    let stdout = "";
    let stderr = "";
    child.stdout?.setEncoding("utf8");
    child.stderr?.setEncoding("utf8");
    child.stdout?.on("data", (c) => { stdout += c; });
    child.stderr?.on("data", (c) => { stderr += c; });
    if (stdin !== undefined) {
      child.stdin?.on("error", () => {});
      child.stdin?.end(stdin);
    }
    child.on("error", (error) => resolve({ status: null, stdout, stderr: String(error?.message ?? error), signal: null }));
    child.on("close", (status, signal) => resolve({ status, stdout, stderr, signal }));
  });
}

/** 把远端 stderr 里常见的措辞映射成 FileError code。ssh 自己的连接失败是 status 255。 */
function remoteFileError(status, stderr, path) {
  const text = String(stderr ?? "");
  if (status === null) return new FileError("unknown", `ssh 连不上：${text}`, path);
  if (/No such file|cannot stat|不存在/i.test(text)) return new FileError("not_found", text.trim(), path);
  if (/Permission denied|not permitted|EACCES/i.test(text)) return new FileError("permission_denied", text.trim(), path);
  if (/Not a directory|ENOTDIR/i.test(text)) return new FileError("not_directory", text.trim(), path);
  if (/Is a directory|EISDIR/i.test(text)) return new FileError("is_directory", text.trim(), path);
  return new FileError("unknown", text.trim() || `远端退出码 ${status}`, path);
}

// ── 路径 ──────────────────────────────────────────────────────────────────────
// 远端路径解析：相对路径相对 cwd 拼成绝对路径，交给远端 shell 时它再按 `/` 展开。
// 这里跟 NodeExecutionEnv 一样，~ 和 file:// 留到远端处理（远端有自己的 $HOME）。
const resolveRemotePath = (cwd, p) => {
  const s = String(p ?? "");
  if (s === "~" || s.startsWith("~/")) return s;
  if (isAbsolute(s)) return resolve(s); // 纯归一化，不掺本地 cwd
  // cwd 是 `~` 时**不能**进 resolve：`resolve("~","x")` 会拼出「本地进程 cwd + /~/x」，
  // 远端根本不存在那个路径。`~` 只能在远端展开，所以直接拼成 `~/x` 交出去。
  const base = String(cwd ?? "~");
  if (base === "~" || base.startsWith("~/")) return join(base, s);
  return resolve(base, s);
};

// ── 环境本体 ──────────────────────────────────────────────────────────────────

export class SshExecutionEnv {
  /** 每台机器一个 id —— pi-durable 用它区分「两边的同名路径不是同一个文件」。 */
  id;
  cwd;
  host;
  activeChildren = new Set();

  constructor({ cwd, host, key }) {
    this.cwd = cwd ?? "~";
    // key 折进 host.keyPath —— sshHostArgs 只收一个参数，不用改十几处调用点。
    // 密钥内容不进 this，只留临时文件路径。
    this.host = key ? { ...host, keyPath: materializeKey(key) } : host;
    this.id = `ssh:${host.addr}:${host.port ?? 22}`;
  }

  // ── Shell ──────────────────────────────────────────────────────────────────

  async exec(command, options, context) {
    const signal = context.abortSignal;
    if (signal?.aborted) return err(new ExecutionError("aborted", "aborted"));

    const cwd = options?.cwd ? resolveRemotePath(this.cwd, options.cwd) : this.cwd;
    // 字符串走远端 shell（cd 到 cwd 再跑）；数组逐个 shq 后拼，不经过 shell 解析。
    const body = typeof command === "string" ? command : command.map(shq).join(" ");
    const remote = `cd ${shq(cwd)} 2>/dev/null || exit 126; ${body}`;

    return await new Promise((resolvePromise) => {
      let settled = false;
      let child;
      let timeoutId;
      const stdoutDecoder = new StreamDecoder();
      const stderrDecoder = new StreamDecoder();

      const emit = (text, stream) => {
        if (settled || text === "" || !options?.onOutput) return;
        try {
          options.onOutput(text, context, { stream });
        } catch {
          // onOutput 抛错不打断执行 —— NodeExecutionEnv 会把它当 callback_error，这里从简。
        }
      };
      const kill = () => {
        if (child?.pid) child.kill("SIGKILL");
      };
      const onAbort = () => kill();
      const settle = (result) => {
        if (settled) return;
        settled = true;
        if (timeoutId) clearTimeout(timeoutId);
        if (signal) signal.removeEventListener("abort", onAbort);
        if (child?.pid) this.activeChildren.delete(child.pid);
        resolvePromise(result);
      };

      try {
        child = spawn("ssh", [...sshHostArgs(this.host), remote], {
          stdio: ["ignore", "pipe", "pipe"],
          windowsHide: true,
        });
        if (child.pid) this.activeChildren.add(child.pid);
      } catch (error) {
        settle(err(new ExecutionError("spawn_error", String(error?.message ?? error))));
        return;
      }

      if (options?.timeout !== undefined) {
        timeoutId = setTimeout(() => {
          kill();
        }, options.timeout * 1000);
      }
      if (signal) {
        if (signal.aborted) onAbort();
        else signal.addEventListener("abort", onAbort, { once: true });
      }

      child.stdout?.on("data", (chunk) => emit(stdoutDecoder.decode(chunk), "stdout"));
      child.stderr?.on("data", (chunk) => emit(stderrDecoder.decode(chunk), "stderr"));

      child.on("error", (error) => settle(err(new ExecutionError("spawn_error", String(error?.message ?? error)))));
      child.on("close", (code, exitSignal) => {
        emit(stdoutDecoder.decode(), "stdout");
        emit(stderrDecoder.decode(), "stderr");
        if (signal?.aborted) return settle(err(new ExecutionError("aborted", "aborted")));
        // 126 是我们自己塞的「cd 失败」，125 留给别的用途。
        if (code === 126) return settle(err(new ExecutionError("spawn_error", `工作目录不存在：${cwd}`)));
        if (code === 255) return settle(err(new ExecutionError("spawn_error", `ssh 连不上 ${this.host.addr}:${this.host.port}`)));
        const exitCode = code ?? (exitSignal ? 128 + 1 : 1);
        settle(ok({ exitCode }));
      });
    });
  }

  // ── 读 ──────────────────────────────────────────────────────────────────────

  async readBinaryBytes(path) {
    const remote = `base64 -w0 < ${shq(path)}`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, path));
    const clean = r.stdout.replace(/\s+/g, "");
    if (!clean) return ok(new Uint8Array(0));
    try {
      return ok(Buffer.from(clean, "base64"));
    } catch {
      return err(new FileError("invalid", "base64 解码失败", path));
    }
  }

  async readTextFile(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    const aborted = context.abortSignal?.aborted ? err(new FileError("aborted", "aborted", resolved)) : undefined;
    if (aborted) return aborted;
    const bytes = await this.readBinaryBytes(resolved);
    if (!bytes.ok) return bytes;
    return ok(new TextDecoder("utf-8", { fatal: false }).decode(bytes.value));
  }

  async readTextLines(path, options, context) {
    const read = await this.readTextFile(path, context);
    if (!read.ok) return read;
    const max = options?.maxLines;
    if (max !== undefined && max <= 0) return ok([]);
    const lines = read.value.split("\n");
    if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
    return ok(max === undefined ? lines : lines.slice(0, max));
  }

  async readBinaryFile(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    const aborted = context.abortSignal?.aborted ? err(new FileError("aborted", "aborted", resolved)) : undefined;
    if (aborted) return aborted;
    return this.readBinaryBytes(resolved);
  }

  // ── 写 ──────────────────────────────────────────────────────────────────────

  async writeFile(path, content, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    const aborted = context.abortSignal?.aborted ? err(new FileError("aborted", "aborted", resolved)) : undefined;
    if (aborted) return aborted;
    const parent = join(resolved, "..");
    const bytes = typeof content === "string" ? Buffer.from(content, "utf8") : Buffer.from(content);
    const remote = `mkdir -p ${shq(parent)} && cat > ${shq(resolved)}`;
    const r = await runRemote(this.host, remote, { stdin: bytes });
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, resolved));
    return ok(undefined);
  }

  async appendFile(path, content, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    const aborted = context.abortSignal?.aborted ? err(new FileError("aborted", "aborted", resolved)) : undefined;
    if (aborted) return aborted;
    const parent = join(resolved, "..");
    const bytes = typeof content === "string" ? Buffer.from(content, "utf8") : Buffer.from(content);
    const remote = `mkdir -p ${shq(parent)} && cat >> ${shq(resolved)}`;
    const r = await runRemote(this.host, remote, { stdin: bytes });
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, resolved));
    return ok(undefined);
  }

  async truncateFile(path, size, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const remote = `truncate -s ${Number(size)} ${shq(resolved)}`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, resolved));
    return ok(undefined);
  }

  async flushFile(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    // 远端没有打开的 fd 要 sync —— cat > 写完就落盘了。存在性校验 + no-op。
    const exists = await this.exists(resolved, context);
    if (!exists.ok) return exists;
    return ok(undefined);
  }

  async renameFile(sourcePath, destinationPath, context) {
    const source = resolveRemotePath(this.cwd, sourcePath);
    const destination = resolveRemotePath(this.cwd, destinationPath);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", source));
    const remote = `mv ${shq(source)} ${shq(destination)}`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, source));
    return ok(undefined);
  }

  // ── 元数据 ──────────────────────────────────────────────────────────────────

  async fileInfo(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const remote = `stat -c '%F|%s|%Y' ${shq(resolved)} 2>&1; printf '\\n\\n__exit__%s' "$?"`;
    const r = await runRemote(this.host, remote);
    // 故意把 exit code 也打进 stdout，因为 stat 非零时 stdout 是空、stderr 才有话说。
    const m = /^(.*?)\n\n__exit__(\d+)$/s.exec(r.stdout.trim());
    const exit = m ? Number(m[2]) : (r.status ?? 1);
    const line = (m ? m[1] : "").trim();
    if (exit !== 0 || !line) return err(remoteFileError(r.status, r.stderr || line, resolved));
    const [kindText, sizeText, mtimeText] = line.split("|");
    const kind = { "regular file": "file", "directory": "directory", "symbolic link": "symlink" }[kindText];
    if (!kind) return err(new FileError("invalid", `不支持的文件类型：${kindText}`, resolved));
    return ok({
      name: basename(resolved),
      path: resolved,
      kind,
      size: Number(sizeText) || 0,
      mtimeMs: Math.round(Number(mtimeText) * 1000),
    });
  }

  async listDir(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const remote = `find ${shq(resolved)} -mindepth 1 -maxdepth 1 -printf '%y\\t%f\\t%s\\t%T@\\n' 2>&1; printf '\\n\\n__exit__%s' "$?"`;
    const r = await runRemote(this.host, remote);
    const m = /^(.*?)\n\n__exit__(\d+)$/s.exec(r.stdout.trim());
    const exit = m ? Number(m[2]) : (r.status ?? 1);
    if (exit !== 0) return err(remoteFileError(r.status, r.stderr || (m ? m[1] : ""), resolved));
    const kinds = { f: "file", d: "directory", l: "symlink" };
    const infos = [];
    for (const line of (m[1] ?? "").split("\n")) {
      const [y, name, sizeText, mtimeText] = line.split("\t");
      const kind = kinds[y];
      if (!kind || name === undefined) continue;
      infos.push({
        name,
        path: join(resolved, name),
        kind,
        size: Number(sizeText) || 0,
        mtimeMs: Math.round(Number(mtimeText) * 1000),
      });
    }
    return ok(infos);
  }

  async exists(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const remote = `test -e ${shq(resolved)} && echo yes || echo no`;
    const r = await runRemote(this.host, remote);
    if (r.status === 255) return err(remoteFileError(r.status, r.stderr, resolved));
    return ok(r.stdout.trim() === "yes");
  }

  async canonicalPath(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const remote = `realpath ${shq(resolved)} 2>&1; printf '\\n\\n__exit__%s' "$?"`;
    const r = await runRemote(this.host, remote);
    const m = /^(.*?)\n\n__exit__(\d+)$/s.exec(r.stdout.trim());
    const exit = m ? Number(m[2]) : (r.status ?? 1);
    if (exit !== 0) return err(remoteFileError(r.status, r.stderr || (m ? m[1] : ""), resolved));
    return ok((m[1] ?? "").trim());
  }

  async absolutePath(path, _context) {
    return ok(resolveRemotePath(this.cwd, path));
  }

  async joinPath(parts, _context) {
    return ok(join(...parts));
  }

  // ── 增删 ────────────────────────────────────────────────────────────────────

  async createDir(path, options, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const flag = options?.recursive === false ? "" : "-p";
    const remote = `mkdir ${flag} ${shq(resolved)}`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, resolved));
    return ok(undefined);
  }

  async remove(path, options, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", resolved));
    const flags = [options?.recursive ? "-r" : "", options?.force ? "-f" : ""].filter(Boolean).join(" ");
    const remote = `rm ${flags} ${shq(resolved)}`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, resolved));
    return ok(undefined);
  }

  async createTempDir(prefix, context) {
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted"));
    const remote = `mktemp -d ${shq(`/tmp/${prefix ?? "tmp-"}`)}.XXXXXX`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, "/tmp"));
    return ok(r.stdout.trim());
  }

  async createTempFile(options, context) {
    const dir = await this.createTempDir("tmp-", context);
    if (!dir.ok) return dir;
    const filePath = `${dir.value}/${options?.prefix ?? ""}${Date.now()}${options?.suffix ?? ""}`;
    const remote = `touch ${shq(filePath)}`;
    const r = await runRemote(this.host, remote);
    if (r.status !== 0) return err(remoteFileError(r.status, r.stderr, filePath));
    return ok(filePath);
  }

  // ── 流式 reader（open 时整块读进内存） ─────────────────────────────────────

  async openTextLineReader(path, context) {
    const read = await this.readTextFile(path, context);
    if (!read.ok) return read;
    return ok(new MemTextLineReader(read.value, resolveRemotePath(this.cwd, path)));
  }

  async openBinaryReader(path, options, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    const read = await this.readBinaryFile(resolved, context);
    if (!read.ok) return read;
    return ok(new MemBinaryReader(read.value, resolved));
  }

  async openDirReader(path, context) {
    const resolved = resolveRemotePath(this.cwd, path);
    const listed = await this.listDir(resolved, context);
    if (!listed.ok) return listed;
    return ok(new MemDirReader(listed.value, resolved));
  }

  // ── watch：远端没有可靠 fs 事件，明确说「不支持」 ───────────────────────────

  async watch(_targets, _onChange, _context) {
    return err(new FileError("not_supported", "远端环境不支持文件监听", ""));
  }

  async cleanup(_context) {
    for (const pid of this.activeChildren) {
      try { process.kill(pid, "SIGKILL"); } catch {}
    }
    this.activeChildren.clear();
    // 临时密钥文件跟着环境一起走。
    const keyPath = this.host?.keyPath;
    if (keyPath) {
      const dir = keyPath.replace(/[\\/][^\\/]+$/, "");
      KEY_DIRS.delete(dir);
      try { rmSync(dir, { recursive: true, force: true }); } catch {}
    }
  }
}

// ── 内存 reader（spike 版） ───────────────────────────────────────────────────

class MemTextLineReader {
  lines;
  path;
  index = 0;
  closed = false;
  constructor(text, path) {
    const split = text.split("\n");
    if (split.length > 0 && split[split.length - 1] === "") split.pop();
    this.lines = split;
    this.path = path;
  }
  async readLine(context) {
    if (this.closed) return err(new FileError("invalid", "Text line reader is closed", this.path));
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", this.path));
    if (this.index >= this.lines.length) return ok(undefined);
    return ok({ text: this.lines[this.index++], terminated: true });
  }
  async close() { this.closed = true; }
}

class MemBinaryReader {
  bytes;
  path;
  closed = false;
  constructor(bytes, path) {
    this.bytes = bytes;
    this.path = path;
  }
  async info() {
    if (this.closed) return err(new FileError("invalid", "Binary reader is closed", this.path));
    return ok({ name: basename(this.path), path: this.path, kind: "file", size: this.bytes.length, mtimeMs: 0 });
  }
  async read(offset, length) {
    if (this.closed) return err(new FileError("invalid", "Binary reader is closed", this.path));
    if (!Number.isSafeInteger(offset) || offset < 0 || !Number.isSafeInteger(length) || length < 0) {
      return err(new FileError("invalid", "Offset and length must be non-negative safe integers", this.path));
    }
    const slice = this.bytes.subarray(offset, Math.min(offset + length, this.bytes.length));
    return ok(slice);
  }
  async scanLines(options) {
    if (this.closed) return err(new FileError("invalid", "Binary reader is closed", this.path));
    // 找到 startLine 的位置，返回一个粗略的 LineScan。
    const text = new TextDecoder().decode(this.bytes);
    const newlinePositions = [];
    for (let i = 0; i < text.length; i++) if (text.charCodeAt(i) === 10) newlinePositions.push(i);
    const start = options.startLine === 0 ? 0 : (newlinePositions[options.startLine - 1] ?? text.length) + 1;
    const end = newlinePositions[options.endLine ?? newlinePositions.length - 1] ?? text.length;
    return ok({
      newlines: newlinePositions.length,
      start: Math.min(start, text.length),
      end: Math.min(end, text.length),
      firstLineEnd: newlinePositions[options.startLine] ?? text.length,
      lastLineStart: start,
      selectedBytes: Math.max(0, Math.min(end, text.length) - start),
      firstLineBytes: Math.max(0, (newlinePositions[options.startLine] ?? text.length) - start),
    });
  }
  async close() { this.closed = true; }
}

class MemDirReader {
  entries;
  path;
  index = 0;
  closed = false;
  constructor(entries, path) {
    this.entries = entries;
    this.path = path;
  }
  async next(maxEntries, context) {
    if (this.closed) return err(new FileError("invalid", "Directory reader is closed", this.path));
    if (context.abortSignal?.aborted) return err(new FileError("aborted", "aborted", this.path));
    const slice = this.entries.slice(this.index, this.index + maxEntries);
    this.index += slice.length;
    return ok({ entries: slice, done: this.index >= this.entries.length });
  }
  async close() { this.closed = true; }
}
