//! 分页游标：**不透明**，只由 id 编出来（上游 `storage/scan.ts`，40 行）。
//!
//! 上游的游标是一个 JSON 对象 `{ after: number, order }` —— 语义一样，但它把 `after` 赤裸地
//! 摆在调用方面前，于是「翻页」很容易被写成「跳过多少条」或者「时间戳早于多少」。`wire.md` 里
//! 没有这种东西，本库也就不该先长出来。这里改成一段不透明文本：
//!
//! ```text
//! a1f    升序，从 id 0x1f（base36 "1f"）之后接着翻
//! d1f    降序，同上
//! ```
//!
//! 只编码 id，不带偏移量、不带时间、不带排序键之外的任何东西。解码失败就是
//! [`Refusal::InvalidCursor`] —— 调用方把游标当黑盒原样回传，编造就当场失败。

use super::writes::Refusal;

/// 扫描方向。`ascending` 是最老的在前，`descending` 是最新的在前。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanOrder {
    Ascending,
    Descending,
}

impl ScanOrder {
    pub fn direction(self) -> &'static str {
        match self {
            ScanOrder::Ascending => "ASC",
            ScanOrder::Descending => "DESC",
        }
    }

    fn prefix(self) -> char {
        match self {
            ScanOrder::Ascending => 'a',
            ScanOrder::Descending => 'd',
        }
    }
}

/// 一页的续读位置：从 `after` 之后接着翻，方向沿用创建游标时的方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub after: i64,
    pub order: ScanOrder,
}

impl Cursor {
    pub fn new(after: i64, order: ScanOrder) -> Self {
        Self { after, order }
    }

    /// 编成不透明文本。`after` 不许为负（id 从 1 起）。
    pub fn encode(&self) -> String {
        let mut text = String::new();
        text.push(self.order.prefix());
        to_base36(self.after.max(0) as u64, &mut text);
        text
    }

    pub fn decode(text: &str) -> Result<Self, Refusal> {
        let mut characters = text.chars();
        let order = match characters.next() {
            Some('a') => ScanOrder::Ascending,
            Some('d') => ScanOrder::Descending,
            _ => return Err(Refusal::InvalidCursor),
        };
        let rest = characters.as_str();
        if rest.is_empty() {
            return Err(Refusal::InvalidCursor);
        }
        let after = from_base36(rest).ok_or(Refusal::InvalidCursor)?;
        Ok(Self { after, order })
    }
}

/// 解析「请求的顺序 + 可选游标 + 兜底顺序」，返回实际的顺序与起点。
///
/// 规则照抄上游 `scanStart`：给了游标就沿用游标里的方向；查询又指定了别的方向 = 请求错误
/// （不猜，也不静默改向）。
pub fn scan_start(
    requested: Option<ScanOrder>,
    cursor: Option<&Cursor>,
    fallback: ScanOrder,
) -> Result<(ScanOrder, Option<i64>), Refusal> {
    match cursor {
        None => Ok((requested.unwrap_or(fallback), None)),
        Some(cursor) => {
            if let Some(requested) = requested {
                if requested != cursor.order {
                    return Err(Refusal::CursorOrderMismatch);
                }
            }
            Ok((cursor.order, Some(cursor.after)))
        }
    }
}

/// 一页结果：`next` 只在后面还有东西时出现。
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Cursor>,
}

/// 从「多取一条」的 `limit + 1` 行里切出一页。上游 `page()`。
pub fn cut_page<T>(
    mut values: Vec<T>,
    limit: usize,
    order: ScanOrder,
    id_of: impl Fn(&T) -> i64,
) -> Page<T> {
    if values.len() <= limit {
        return Page {
            items: values,
            next: None,
        };
    }
    values.truncate(limit);
    let next = values.last().map(|last| Cursor::new(id_of(last), order));
    Page {
        items: values,
        next,
    }
}

fn to_base36(mut value: u64, out: &mut String) {
    const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut buffer = [0u8; 13];
    let mut length = 0;
    loop {
        buffer[length] = DIGITS[(value % 36) as usize];
        length += 1;
        value /= 36;
        if value == 0 {
            break;
        }
    }
    for index in (0..length).rev() {
        out.push(buffer[index] as char);
    }
}

fn from_base36(text: &str) -> Option<i64> {
    let mut value: i64 = 0;
    for character in text.chars() {
        let digit = character.to_digit(36)?;
        value = value.checked_mul(36)?.checked_add(i64::from(digit))?;
        if value < 0 {
            return None;
        }
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cursor_is_opaque_and_only_carries_the_id() {
        // 31 = base36 "v"：游标里只有「方向 + id」，没有别的。
        let cursor = Cursor::new(31, ScanOrder::Ascending);
        assert_eq!(cursor.encode(), "av");
        // 不透明：没有分隔符、没有负号、没有被调用方读得懂的字段名。
        for encoded in [
            Cursor::new(2, ScanOrder::Ascending).encode(),
            Cursor::new(1694, ScanOrder::Descending).encode(),
        ] {
            assert!(encoded.chars().all(|c| c.is_ascii_alphanumeric()), "{encoded}");
            assert_eq!(Cursor::decode(&encoded).unwrap().encode(), encoded);
        }
    }

    #[test]
    fn a_cursor_that_was_not_encoded_here_is_refused() {
        for text in ["", "1f", "x1f", "a", "a-1", "a{}", "a 2", "a0123456789012345678"] {
            assert_eq!(Cursor::decode(text), Err(Refusal::InvalidCursor), "{text}");
        }
    }

    #[test]
    fn asking_for_another_order_than_the_cursor_continues_is_a_request_error() {
        let cursor = Cursor::new(7, ScanOrder::Descending);
        assert_eq!(
            scan_start(Some(ScanOrder::Ascending), Some(&cursor), ScanOrder::Ascending),
            Err(Refusal::CursorOrderMismatch)
        );
        assert_eq!(
            scan_start(None, Some(&cursor), ScanOrder::Ascending),
            Ok((ScanOrder::Descending, Some(7)))
        );
        assert_eq!(scan_start(None, None, ScanOrder::Ascending), Ok((ScanOrder::Ascending, None)));
    }

    #[test]
    fn the_page_cuts_one_row_more_than_the_limit_and_carries_the_last_id() {
        let page = cut_page(vec![1, 2, 3], 2, ScanOrder::Ascending, |value| *value);
        assert_eq!(page.items, vec![1, 2]);
        assert_eq!(page.next, Some(Cursor::new(2, ScanOrder::Ascending)));
        // 刚好一页：没有 next（不吐空页）。
        let page = cut_page(vec![1, 2], 2, ScanOrder::Ascending, |value| *value);
        assert_eq!(page.next, None);
    }
}
