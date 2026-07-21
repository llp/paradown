// `src/main.rs` 是 binary crate 的入口文件。
// 这个文件会编译成最终可执行程序；它可以像外部使用者一样，通过 crate 名 `paradown`
// 使用 `src/lib.rs` 中导出的库 API。

// 这里声明当前 binary crate 内部还有一个 `cli_app` 模块。
// 编译器会查找 `src/cli_app.rs` 或 `src/cli_app/mod.rs`；本项目使用后者。
// 这个 `mod cli_app;` 只属于二进制入口，不等同于 `lib.rs` 里的模块声明。
mod cli_app;

// `use paradown::Error;` 从 library crate 的公共 API 中引入 `Error`。
// 虽然 main.rs 和 lib.rs 在同一个 package 里，但它们是两个 crate 目标：
// binary crate 通过包名 `paradown` 引用 library crate。
use paradown::Error;
use std::process::ExitCode;

// `#[tokio::main]` 是属性宏，会在编译时生成启动 Tokio 异步运行时的样板代码。
// 普通 Rust 程序的 `main` 不能直接是 async；这个宏把 async main 包装成同步入口。
#[tokio::main]
async fn main() -> ExitCode {
    // `cli_app::run()` 返回一个 Future；`.await` 表示等待这个异步计算完成。
    // `match` 对 `Result<ExitCode, Error>` 做穷尽匹配：成功返回退出码，失败打印错误并映射退出码。
    match cli_app::run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("paradown: {err}");
            map_error_to_exit_code(&err)
        }
    }
}

// 参数 `err: &Error` 是不可变借用：函数只查看错误，不取得错误所有权。
// 这样调用方仍然保留 `err`，这里也避免复制或移动可能较大的错误值。
fn map_error_to_exit_code(err: &Error) -> ExitCode {
    // `match err` 这里匹配的是 `&Error`。
    // Rust 的模式匹配会通过 match ergonomics 自动处理引用，所以可以直接写 `Error::ConfigError(_)`。
    // `|` 在模式中表示“或者”，多个错误变体映射到同一个退出码。
    // `_` 表示忽略变体内部的数据，因为这里只关心错误类别。
    match err {
        Error::ConfigError(_)
        | Error::InvalidUrl(_)
        | Error::UrlParseError(_)
        | Error::Parse(_)
        | Error::Header(_) => ExitCode::from(64),
        Error::UnsupportedProtocol(_) => ExitCode::from(65),
        Error::TaskNotFound(_) | Error::Canceled(_) => ExitCode::from(2),
        _ => ExitCode::from(1),
    }
}
