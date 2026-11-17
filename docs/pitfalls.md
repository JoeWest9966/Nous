# 本库自己踩的坑

**上游的坑不在这里**（那些在 `docs/pitfalls-from-pi.md`，按 changelog 逐条判定）。这里只记
「我们自己的代码/文档里踩出来的、下次还会再踩」的东西。**只追加**，每条写清症状 —— 症状比
原因重要，因为人是在看到症状时来搜这份文件的。

---

## 2026-11-17 · 文档里写了一个真控制字符 ⇒ 这份文档从搜索里消失

**症状**：`rg '500ms' docs/` 明明该命中 `docs/storage.md`，输出里却整个没有这份文件；`grep`
直接跳过；换 `-a` 才看得见。你会以为「我记错了，没写过」，然后**再写一遍**同一件事。

**原因**：`docs/storage.md` 里有一行

```
| 孤立代理对与 `\x00` 往返无损 |
```

那个 `\x00` 是**真正的 0x00 字节**，不是转义文本。`grep` / `rg` 见 NUL 就判定整个文件是
二进制，默认跳过（`rg` 只给一句 "binary file matches" 甚至什么都不给）。367 行的文档因此对
所有搜索隐形。发现它纯属偶然：搜 `500ms` 时那份文件在结果里缺席。

**怎么修**：写成转义文本 `\u0000`（Markdown 里就是在说「这个字符」），别放真字节。

**怎么防**：改完文档跑一遍扫控制字符（这次一次性扫了全库 `.md`/`.rs`/`.toml`/`.sh`/`.tsv`/
`.json`，除 Tab/换行外 0 个）：

```bash
python3 - <<'PY'
import pathlib
for p in pathlib.Path('.').rglob('*'):
    if 'target' in p.parts or '.git' in p.parts or not p.is_file(): continue
    if p.suffix not in {'.md','.rs','.toml','.sh','.tsv','.json'}: continue
    for i, ch in enumerate(p.read_bytes()):
        if ch < 0x09 or ch in (0x0b, 0x0c) or 0x0e <= ch < 0x20 or ch == 0x7f:
            print(p, i, hex(ch)); break
PY
```

**推广**：任何「我写过但搜不到」的文档，先怀疑它是二进制 —— 一个字节就够。
