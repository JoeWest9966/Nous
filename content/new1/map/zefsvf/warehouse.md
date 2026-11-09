---
name: nous-warehouse
description: "j1900 上的 Nous Warehouse（nw 面板 + REST API）：入口地址、认证、信封形状、端点清单、进程与数据位置、重启命令、踩过的坑。要调它的 API、查它为什么全空、或重启它时读这一份。"
---

# j1900 上的 Nous Warehouse

台账的机器可读版在 `data/registry.json`（页面上的「服务台账」卡片读它，
它的 `doc` 字段就是本文件）。这一份是 agent 和人共用的说明：项目是干嘛的、
API 怎么用、挂了怎么重启。

> 一句话：一台机器上的编目面板，`nw` 把 vps / repo / 模型 / 配方写成 `data` 文件夹，
> `nw-panel` 把它们铺成 UI 和 REST API。调它只需要 tailnet，没有 token。

参考文档（都被测试钉在源码上，改端点不改文档 CI 会红）：
**https://github.com/JoeWest9966/Nous-Warehouse/tree/master/docs**

- `docs/api/README.md` —— 契约
- `docs/api/endpoints.md` —— 85 条路由逐条
- `docs/api/schemas.md` —— `data` 的形状
- `docs/cli/reference.md` —— 19 个命令组

下面这一份是「从这里怎么够到它」—— 入口、认证、坑。它们不在仓库里，因为它们随机器变。

## 入口

| 用途 | 地址 | 认证 |
|---|---|---|
| 面板 UI + REST API（首选） | `https://j1900.tail8d0018.ts.net:8443` | 无 —— tailnet 本身是边界 |
| 备用（裸端口） | `http://100.126.95.105:8080` | `Authorization: Bearer <token>` |
| 局域网 | ~~`http://192.168.1.7:8080`~~ | **已关**，端口不监听 |

`8443` 是 `tailscale serve` 终止 TLS（Let's Encrypt，`j1900.tail8d0018.ts.net`），
反代到 `127.0.0.1:18080`。funnel 全关，没有一条路通天网。

备用入口的 token 在 **j1900:`/etc/panel-gate.env`** 的 `NW_GATE_TOKEN`（chmod 600）。
它只在「客户端不带 TLS / 跑在别人的机器上」时才有意义；能用 8443 就用 8443。

## API

信封形状：`{ schema, nw, ok, errors[], warnings[], data }`。

```bash
U=https://j1900.tail8d0018.ts.net:8443
curl -s $U/api/health          # 免认证握手段
curl -s $U/api/vps             # vps 编目（1 台：xy）
curl -s $U/api/github          # 账号下 repo 总账（50 条）
curl -s $U/api/model           # 配方 / 模型 / 源
curl -s $U/api/recipe          # 装配交付链（pi-test / research-desk）
```

逐端点参考在引擎仓（https://github.com/JoeWest9966/Nous-Warehouse/tree/master/docs）：
`docs/api/README.md`（契约）、`endpoints.md`（85 条路由逐条）、
`schemas.md`（`data` 形状）、`docs/cli/reference.md`（19 个命令组）。
三份都被测试钉在源码上，改端点不改文档 CI 红。

## 进程与数据

```
systemd  nw-panel.service     nw-panel --nw /opt/Nous-Warehouse/nw --root /opt/Data-001 --addr 127.0.0.1:18080
systemd  panel-gate.service   panel-gate --listen 100.126.95.105:8080 --upstream http://127.0.0.1:18080
tailscale serve --https=8443  →  http://127.0.0.1:18080
```

- 两个 unit 都是 `enabled` + `Restart=always`，重启自愈。
- `nw-panel` **拒绝绑非回环地址**（`panel.CheckAddr`），所以对外永远经过 serve 或闸。
- 引擎仓 `j1900:/opt/Nous-Warehouse`（master）；数据仓 `j1900:/opt/Data-001`。
- 日志：`/var/log/nw-panel.log`、`/var/log/panel-gate.log`。

重启：

```bash
ssh root@100.126.95.105 'systemctl restart nw-panel panel-gate'
```

## 踩过的坑

1. **`--root` 指错 = 全站安静地回空。** 曾经指向不存在的
   `/opt/Nous-Warehouse/Data-001`（真身是 `/opt/Data-001`）。`nw` 对「编目不存在」的语义是
   **空数组 + rc 0**，所以面板返回 200 + 空列表，不是 404 —— 查「面板一片空」先看 root。
2. **`nw --version` 恒为 `0.2.0`。** 版本号不告诉你二进制新旧。上次就是 binary 落后源码 9 天，
   github / cnb-key / cloudflare-key / recipe / runtime 六批端点物理上不存在。判新旧只能探端点。
3. **回环面板零认证且 `AllowSecret` 恒 true。** 它一旦被桥到非回环网络，
   `/api/github-key/{name}/secret` 就是明文密钥出口。桥之前先想清楚门在哪。
