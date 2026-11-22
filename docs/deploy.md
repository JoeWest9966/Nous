# 部署 nous-app（issue #52）

一句版：在 asus 上 `scripts/deploy.sh`，一条命令装好；j1900 上 `scripts/deploy-check.sh --wait=300` 验收。

**编译可以不在 asus 做**：静态产物在别的机器上编好后，`NOUS_APP_BIN=/path/to/nous-app scripts/deploy.sh`
直接部署（跳过 `cargo build`，仍会校验「ELF + 静态链接」再搬）。这条给两种场合用：asus 上没装
`musl-tools`，或者为了一个静态二进制不想把整仓编一遍。产物 sha 与 `--version` 仍写进 manifest，
所以「部署的是什么」照样能对账。

开发机磁盘分根分区（256G）与工作区盘（512G）：`CARGO_TARGET_DIR` 一律指到工作区，
否则一个 release 构建就能把根分区吃掉一大半。
绑 `0.0.0.0:4789` + `NOUS_TOKEN_FILE`（成对；缺 token 进程拒绝启动、退出 78）。
旧面板 `nous-panel`（4790）已于 2026-11-18 停用并 disable，`content/` 已归档到 j1900 的
`/var/backups/nous-panel/` 并从仓里移除。

**四条私库路径不进 unit**（#63）：`NOUS_SKILLS` / `NOUS_PROMPTS` / `NOUS_PRESETS` /
`NOUS_EXTEND` 的默认值分别是 `/opt/Nous-skills` / `/opt/Nous-prompts` /
`/opt/Nous-Template` / `/opt/Nous-extend`，在 `crates/nous/env/src/content.rs` 一处解析。
unit 里写这四个变量就会变成两份真相 —— 要换路径只在环境里改一处；缺目录是正常状态，
`/v1/skills` / `/v1/prompts` 仍回 `root` 字符串 + `present=false`，不会回 `null`。

## 0 · 一次性准备（人做，脚本不代劳）

j1900（`root` 登录一次）：

```sh
useradd --system --home /srv/nous/deploy --shell /usr/sbin/nologin deploy
install -d -m 0755 /etc/sudoers.d
printf '%s\n' 'deploy ALL=(root) NOPASSWD: ALL' | tee /etc/sudoers.d/nous-deploy >/dev/null
chmod 0440 /etc/sudoers.d/nous-deploy
```

asus：

```sh
ssh-keygen -t ed25519 -N ''
ssh-copy-id deploy@j1900
```

这三行也写在 `deploy.sh` 的“SSH/sudo 不通”分支里，直接复制即可。现在 deploy 是
`NOPASSWD: ALL`（D10 的反悔点：j1900 上多一个人用这台机器时再收窄到枚举命令）。
本机还要 `rustup target add x86_64-unknown-linux-musl` 与 `musl-tools`。

## 1 · 首次部署

```sh
cd /opt/Nous
scripts/deploy.sh
```

脚本按顺序做：前置检查（SSH/sudo、命令、磁盘）→ 本地编 musl 产物 →
**按数据分叉**（看 `/var/lib/nous/nous.db` 在不在，不看产物）→
建 `/srv/nous/work`、`/var/lib/nous`，`deploy.sh` 生成 `/etc/nous/token`（root:nous 0640）→
rsync 到版本目录 → 写 manifest → `ln -sfn current` → unit 快照 + 安装 →
`daemon-reload && enable --now` → 6 次 × 5 秒打 `/v1/device`，断言 `.build == sha8`。

首次路径**只多出 `nous.db`（+ `-wal`/`-shm`）**；不建 `backups/`、不备份、不查活跃任务。
库由应用自己建表；脚本不提前造一个空库。

> 上线当天第一个 `bash` 工具会是 `EACCES`：`/srv/nous/work` 首版是空目录，`nous` 还没有
> 可写的对象。那不是装坏了。脚本**不会** `chown`/`chgrp` 任何 `/opt/*` 仓。

验收：

```sh
scripts/deploy-check.sh --wait=300   # j1900
nous-app --version
ssh j1900 sudo -n curl -s -H "authorization: Bearer $(sudo cat /etc/nous/token)" \
  http://127.0.0.1:4789/v1/device | jq '{version,build,target}'
```

`--wait=300` 是判据 2c 的真等待：改坏库版本后服务必须 `failed`/`exit-code`，300 秒后
`NRestarts` 不涨。回读 `/v1/device` **必须带 token**：无/错 token 都是 401（`/health` 不存在，别拿它当探针）。

## 2 · 升级 / 幂等

同 sha 再跑一次是**零副作用**：`/opt/nous-app` 树和 `current` 指向都不变（判据 3）。
不同 sha 走升级：

1. `stop nous-app`
2. `--active-tasks` 查**在飞**的任务（`pending` / `running` / `waiting` / `completing`）；
   `sleeping`（触发器排队）与 `terminal`（已收尾）不算 —— 否则触发器一上线就永远升不了级（#91）；
   库里只有 `sleeping` 时闸门打印 `0`
3. `--backup` 单库到 `backups/nous-<ts>.db`，`--check` 打出两行
   `quick_check=ok` / `schema=<n>`；manifest 里的 `schema_version` 必须等于那一行（判据 6）
4. 新版本目录 → `ln -sfn current` → unit 快照 → `start`
5. 健康检查里一旦看到 `refuse:`（journal 或响应），**立即**回退，不等 6 次

版本目录与备份各留 2 份（判据 13）。回退 = `current` 指回上一版 + 恢复快照 + 恢复对应
unit + `restart`；只换 binary 不换库会 `SchemaTooNew` → 78。回退也起不来就**停在 failed**，
把 `journalctl -u nous-app -n 200` 落到
`/var/lib/nous/backups/failed-<ts>.log`，不再回退第二层。

人工回退演练（判据 4，第一次部署时没有上一版，要第二次升级后才有对象）：

```sh
ssh j1900 sudo -n ls -1d /opt/nous-app/nous-app-*/nous-app
ssh j1900 sudo -n ls -1t /var/lib/nous/backups/*.json
# 按上一版目录改 current 软链，按 nous-<ts>.json 的 predecessor 恢复对应 nous-<ts>.db 与 unit，再 restart
```

## 3 · 已完成的切换：`127.0.0.1` → `0.0.0.0:4789`（2026-11-18 做完，留作记录）

首版绑回环是因为 #50 的认证与 NousDesk `#8` 的 header 注入当时都还没实现：
`0.0.0.0` + 一个带 bash 工具、零认证的服务 = 局域网内任意设备可执行命令。
**判据 9b 的口径也跟着换了**：不再要求「只听回环」，而是「绑了非回环就必须有钥匙」——
从局域网打 `192.168.1.7:4789`，未认证必须 401、token 文件必须在；只听回环时跳过裸奔检查。

**前置**：#50 合入（非回环无 token → `refuse: no-token-on-non-loopback` / 78），
且 NousDesk `#8` 已合入（`desktop.toml` 有 token、`client.rs` 统一注入 header）。

**动作**（一次显式改动，不混在别处）：

```sh
ssh j1900 sudo -n systemctl edit nous-app
# [Service]
# Environment=NOUS_LISTEN=0.0.0.0:4789
ssh j1900 sudo -n systemctl daemon-reload
ssh j1900 sudo -n systemctl restart nous-app
scripts/deploy-check.sh --wait=300   # 跑 401 判据：无/错/空 token → 401
```

不要在 deploy.sh 里偷偷改这一行；判据 9b 会在改口前一直红。

## 4 · token 轮换 / 卸载

```sh
scripts/deploy.sh --rotate-token     # 重建 32B，root:nous 0640，只回显路径
scripts/deploy.sh --uninstall        # 停服务、摘 unit、删 /opt/nous-app
scripts/deploy.sh --purge --yes-i-mean-it   # 才删 /var/lib/nous 与 /etc/nous/token
```

`--uninstall` 后：`/opt/nous-app` 不在，`/var/lib/nous` 与 `/etc/nous/token` 一个字节不动
（判据 5）。`--purge` 必须手打完整确认串，不接受 `-y`。升级与回退不得改变 token 的
inode 与 sha256（判据 7）。

## 5 · 人工哨兵清单

| 判据 | 为什么不能自动化 / 怎么做 |
|---|---|
| 2c | 真 sleep 300 秒看 `NRestarts` 不涨；`deploy-check.sh --wait=300` 已把等待参数化，但跑它会真把服务拉停一次 |
| 4 | 第一次部署时没有“上一版产物”；要等第二次升级后才有回退对象 |
| 11 | 需要一个真流式会话；跑 deploy.sh 后断言 entries 条数不变，且拒绝信息打印 task id + conversation id |
| 5 | `--uninstall` / `--purge` 会真停服务并删数据，不在自检脚本里自动跑 |
| 10 | 临时删 unit 的 `WorkingDirectory=` 再重启真服务，验 `refuse: workdir-missing` + 78 + failed；验完恢复 |

## 6 · 三条硬边界

- 不碰 4790 / `nous-panel.service` / `content/` 下任何块目录 —— 旧面板已下线（2026-11-18），
  这几条现在只剩历史意义；判据 14 仍在守「部署不碰别人的东西」（判据 14：部署前后
  `systemctl is-active nous-panel` 与 `git -C /opt/Nous status --porcelain` 都不变）。
- 不 `chown`/`chgrp` 任何 `/opt/*`。
- 脚本里没有任何凭据：`git grep -inE 'sshpass|password|secret|crabport' scripts/` 必须 0 命中
  （判据 15）。能改宿主状态文件的人仍能解锁/改配置；这里给的是“没有顺手删掉这回事”，不是物理防线。
