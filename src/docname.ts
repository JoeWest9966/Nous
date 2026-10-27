/**
 * 文档名的规矩。**只有这一份** —— 服务端和浏览器都 import 它。
 *
 * 名字就是身份：`map/<id>/<name>` 里的文件名。所以它必须是一个文件名，不是一个路径：
 * 只收字母/数字/中文/`._-`，因此拼不出 `../`，也拼不出 `/etc/passwd`。
 * 必须 `.md` 结尾 —— 这一页只管 markdown。
 *
 * 两份实现会漂。上一版就漂过：客户端会补 `.md`，服务端不补，
 * 于是打个「设计笔记」过去，报「名字不合规矩」。
 */

const BAD = /[^\p{L}\p{N}._-]/gu;

/** 把用户敲的东西变成一个合法的文件名。空字符串 = 什么都不剩（空名字）。 */
export const normalizeDocName = (raw: string): string => {
  let s = raw.trim().toLowerCase();
  s = s.replace(/\s+/g, "-").replace(BAD, "").replace(/-+/g, "-");
  s = s.replace(/^[-.]+/, "").replace(/[-.]+$/, "");
  if (!s) return "";
  if (!s.endsWith(".md")) s += ".md";
  return s;
};

/** 已经是合法文件名了没有。**不补 `.md`、不改大小写** —— 只判断。 */
export const isDocName = (name: string): boolean => /^[\p{L}\p{N}][\p{L}\p{N}._-]{0,63}\.md$/u.test(name);
