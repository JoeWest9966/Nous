# 执行环境

执行环境的契约层：文件系统与命令执行两张 trait，外加四条私库路径的解析。

**什么时候用**：任何要读写技能库、模板库、扩展库或提示词库的代码，都从这里拿路径和能力。

**不负责**：不决定内容长什么样、不做 git 发布、不做缓存；本地与远端实现都在它之下。

## 它自带一条端点

`GET /v1/fs/dirs`（本机 #68 / 远端 #78）住在 `routes.rs` —— Magento 2 那种「模块自带
controller」。放这儿的理由是这条路径的语义全在本 crate：`NOUS_START_DIRS` 的读取点、
`~` 不展开、符号链接不算目录、`BrowseError` 的分类。搬到别处就得把这些规则再抄一遍。

远端那条分支**不在这里实现**：`nous-env-ssh` 的 `requires` 里就有本 crate（它要实现
这里的 `ExecutionEnv`），直接调会成环，所以它是一条宿主能力 trait
（`routes::Host::remote_subdirs`），由 `server` 实现并注入。
