# 模块关系网

把「谁依赖谁」变成一条只读端点：`GET /v1/modules/graph` 一次给全整张图。

**边的含义**：A 的 `plugin.toml [requires]` 里写着 B 的包名，就有 A → B。这份原料和
`cargo metadata` 双向逐条比对过（少写多写都红），所以是可信的事实，不是猜的。

**不负责**：「A 干活得靠 B」这种语义依赖。内核里没有它的机器可读来源，本插件不替任何人编，
缺的就不在图上；被依赖却没有对应模块的包名进 `unmapped`，不静默丢弃。

**形状**：`{generatedFor, nodes, edges:[{from,to,via}], unmapped}`，编译期内嵌，启动零 IO。