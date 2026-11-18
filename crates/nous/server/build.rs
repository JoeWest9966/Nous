//! 编译期只注入 target triple；身份的字节哈希在运行时算（见 src/version.rs）。

fn main() {
    println!(
        "cargo:rustc-env=NOUS_BUILD_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string())
    );
}
