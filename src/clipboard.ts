/**
 * 复制到剪贴板。
 *
 * `navigator.clipboard` 在 http 下整个不存在（局域网 IP 打开就属于这种情况）——
 * 那不是报错，是 undefined。所以留一条 `execCommand` 的老路，它不挑协议。
 */
export async function copyText(value: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(value);
      return true;
    }
  } catch {
    // 权限被拒也会走到这里 —— 继续试老办法，别直接放弃。
  }
  const ta = document.createElement("textarea");
  ta.value = value;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  const ok = document.execCommand?.("copy") ?? false;
  ta.remove();
  return ok;
}
