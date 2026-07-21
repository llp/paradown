use crate::Error;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// 下载任务状态。
///
/// 这个 enum 展示了两类常见变体：
/// - `Pending`、`Running` 这类无字段变体，只表示一种状态。
/// - `Failed(Error)` 这种元组风格变体，除了表示“失败”，还携带一个 `Error` 值。
///
/// `#[derive(Clone, Debug, Serialize, Deserialize)]` 自动生成常用 trait 实现。
/// 这里没有 derive `Display`，因为 `Display` 通常需要人工决定用户看到的字符串，
/// 所以下面手写了 `impl fmt::Display for Status`。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Status {
    Pending,
    Preparing,
    Running,
    Paused,
    Completed,
    Canceled,
    Failed(Error),
    Deleted,
}

/// 实现 `Display`，让 `Status` 可以用 `{}` 格式化。
///
/// trait 实现的语法是：
///
/// ```ignore
/// impl TraitName for TypeName {
///     ...
/// }
/// ```
///
/// `fmt` 的签名由 `Display` trait 规定，不能随意改。
/// `fmt::Formatter<'_>` 中的 `'_` 是匿名生命周期，表示 formatter 引用的生命周期由编译器推断。
impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `match self` 对 enum 的所有可能状态做穷尽匹配。
        // 每个分支都返回 `&'static str` 字符串字面量，所以整个 match 表达式的结果是 `&str`。
        let s = match self {
            Status::Pending => "Pending",
            Status::Preparing => "Preparing",
            Status::Running => "Running",
            Status::Paused => "Paused",
            Status::Completed => "Completed",
            Status::Failed(_) => "Failed",
            Status::Canceled => "Canceled",
            Status::Deleted => "Deleted",
        };
        // `write!` 是宏调用，宏名后面有 `!`。
        // 它把格式化后的内容写入 formatter，并返回 `fmt::Result`。
        write!(f, "{}", s)
    }
}

impl Status {
    /// 判断状态是否已经结束。
    ///
    /// `matches!` 是宏，用来把“某个值是否匹配某些模式”转换成 bool。
    /// `Status::Failed(_)` 中的 `_` 表示忽略失败状态里携带的 `Error`。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Status::Completed | Status::Canceled | Status::Failed(_) | Status::Deleted
        )
    }
}

/// 实现 `FromStr`，让字符串可以尝试解析成 `Status`。
///
/// 有了这个实现后，可以使用：
///
/// ```ignore
/// let status: Status = "Running".parse()?;
/// ```
///
/// `FromStr` 有一个关联类型 `Err`，用于指定解析失败时的错误类型。
impl FromStr for Status {
    // `()` 是单元类型，表示这里的解析错误不携带额外信息。
    // 如果需要更详细的错误，可以把它换成自定义错误类型。
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // 返回 `Result<Self, Self::Err>`：
        // - `Ok(Status::...)` 表示解析成功。
        // - `Err(())` 表示解析失败。
        // 这里的 `Self` 等价于 `Status`，`Self::Err` 等价于上面定义的 `()`。
        match s {
            "Pending" => Ok(Status::Pending),
            "Running" => Ok(Status::Running),
            "Preparing" => Ok(Status::Preparing),
            "Paused" => Ok(Status::Paused),
            "Completed" => Ok(Status::Completed),
            "Failed" => Ok(Status::Failed(crate::error::Error::Other(String::from("")))),
            "Canceled" => Ok(Status::Canceled),
            "Deleted" => Ok(Status::Deleted),
            _ => Err(()),
        }
    }
}
