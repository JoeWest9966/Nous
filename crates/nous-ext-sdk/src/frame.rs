//! 长度前缀 JSON 帧。
//!
//! 帧格式是 spec ch2 拍板的：**一对管道 + 4 字节大端长度 + JSON**。不开第二个端口、
//! 不引入第二种编码、不做心跳 —— 管道断开即死亡。
//!
//! **单帧上限 1MB，双向各自计数**：`host→ext` 与 `ext→host` 各算各的。拒绝必须发生在
//! **读入之前**：先看长度前缀，超限就不读 payload（EX-58）。

use std::io::{self, Read, Write};

use serde_json::Value;

/// 协议版本。与宿主不符即拒载并报双方版本。
pub const PROTOCOL: u32 = 3;

/// 单帧上限（字节）。与 SSE 未发送上限同数（spec 4.5）。
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// 帧层的失败。
#[derive(Debug)]
pub enum FrameError {
    /// 对端在帧边界处关了管道。不是错误，是「进程没了」。
    Closed,
    /// 声明的长度超过上限。**没有读 payload** —— 拒绝发生在分配之前。
    Oversized { declared: usize, limit: usize },
    /// 4 字节长度前缀不完整。
    ShortPrefix,
    Io(io::Error),
    /// payload 不是合法 JSON。
    Json(serde_json::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Closed => f.write_str("frame channel closed"),
            FrameError::Oversized { declared, limit } => {
                write!(f, "frame too large: declared={declared} limit={limit}")
            }
            FrameError::ShortPrefix => f.write_str("short frame prefix"),
            FrameError::Io(error) => write!(f, "frame io: {error}"),
            FrameError::Json(error) => write!(f, "frame json: {error}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        FrameError::Io(error)
    }
}

/// 读一帧。先读 4 字节长度，**超限直接返回 [`FrameError::Oversized`]**，不读 payload。
pub fn read_frame<R: Read + ?Sized>(reader: &mut R) -> Result<Value, FrameError> {
    let mut prefix = [0u8; 4];
    let mut filled = 0;
    while filled < 4 {
        match reader.read(&mut prefix[filled..]) {
            Ok(0) if filled == 0 => return Err(FrameError::Closed),
            Ok(0) => return Err(FrameError::ShortPrefix),
            Ok(n) => filled += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    let declared = u32::from_be_bytes(prefix) as usize;
    if declared > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized {
            declared,
            limit: MAX_FRAME_BYTES,
        });
    }
    let mut payload = vec![0u8; declared];
    reader.read_exact(&mut payload).map_err(FrameError::Io)?;
    serde_json::from_slice(&payload).map_err(FrameError::Json)
}

/// 写一帧。超限同样在写之前拒绝。
pub fn write_frame<W: Write + ?Sized>(writer: &mut W, value: &Value) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(value).map_err(FrameError::Json)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized {
            declared: payload.len(),
            limit: MAX_FRAME_BYTES,
        });
    }
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

/// 只读长度前缀、不读 payload 时的上限检查形状（宿主用来提前杀进程）。
pub fn exceeds_limit(declared: usize) -> bool {
    declared > MAX_FRAME_BYTES
}
