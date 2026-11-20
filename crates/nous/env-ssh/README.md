# nous-env-ssh

`nous-env` 的远端实现：`ssh` / `scp` 把 `FileSystem` + `Shell` 翻成远端命令。

边界：配置从机器编目（`NOUS_MACHINES_FILE`）读，凭据只从环境读；私钥内存 → 0600 临时文件 → ssh `-i` → 删除；ControlMaster 复用，`NOUS_SSH_MUX=0` 关。不把密钥写进仓库、日志、会话记录或数据库。
