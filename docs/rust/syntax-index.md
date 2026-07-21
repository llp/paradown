# Rust 语法索引与项目代码对照

这份索引用来回答两个问题：

```text
我想学某个 Rust 语法，应该看项目里的哪段代码？
我在代码里看到某个语法，应该去哪个文档复习？
```

索引原则：

- 每个语法主题只选择最有代表性的代码位置。
- 不把测试代码作为主要学习入口。
- 复杂主题链接到 `docs/rust/` 下的详细文档。
- 代码位置可能随项目演进变化，更新注释时也要同步更新本索引。

## 1. 模块系统与可见性

### 1.1 `mod`

代表代码：

- `src/lib.rs`
- `src/domain/mod.rs`
- `src/p2p/mod.rs`

语法形态：

```rust
mod checksum;
mod config;
pub mod discovery;
pub(crate) mod driver;
```

解释重点：

- `mod name;` 声明一个模块。
- Rust 会按约定查找 `name.rs` 或 `name/mod.rs`。
- 没有 `pub` 的模块默认只在当前模块及子模块内可见。
- `pub mod` 会把模块暴露给外部使用者。
- `pub(crate)` 表示只在当前 crate 内公开。

### 1.2 `pub use`

代表代码：

- `src/lib.rs`
- `src/domain/mod.rs`
- `src/p2p/mod.rs`

语法形态：

```rust
pub use config::{Config, ConfigBuilder};
pub use domain::{SourceDescriptor, SourceKind, SourceSet};
```

解释重点：

- `use` 是把路径引入当前作用域。
- `pub use` 是重新导出。
- 重新导出可以把内部模块中的类型集中暴露到 crate 根。
- 使用者可以写 `paradown::Config`，不必知道它实际定义在 `config.rs`。

### 1.3 路径：`crate::`、`super::`、`self::`

代表代码：

- `src/domain/source.rs`
- `src/status.rs`
- `src/repository/*`

语法形态：

```rust
use crate::domain::DownloadSpec;
use super::SourceCapabilities;
```

解释重点：

- `crate::` 从当前 crate 根开始找。
- `super::` 从父模块开始找。
- `self::` 从当前模块开始找。

## 2. 自定义类型

### 2.1 `struct`

代表代码：

- `src/domain/source.rs`
- `src/domain/manifest.rs`
- `src/job/mod.rs`

语法形态：

```rust
pub struct SourceDescriptor {
    pub id: String,
    pub kind: SourceKind,
}
```

解释重点：

- `struct` 定义一组命名字段。
- 字段默认私有。
- `pub` 字段可以被模块外访问。
- `String`、`Vec<T>`、`PathBuf` 等字段表示结构体拥有这些数据。

### 2.2 `enum`

代表代码：

- `src/domain/spec.rs`
- `src/status.rs`
- `src/events.rs`
- `src/error.rs`

语法形态：

```rust
pub enum Event {
    Start(u32),
    Progress { id: u32, downloaded: u64, total: u64 },
    Error(u32, Error),
}
```

解释重点：

- enum 表示一个值可能是多种变体之一。
- `Start(u32)` 是元组风格变体。
- `Progress { ... }` 是结构体风格变体。
- `Pending` 是无字段变体。
- enum 经常和 `match` 搭配使用。

### 2.3 `#[derive(...)]`

代表代码：

- `src/domain/source.rs`
- `src/error.rs`
- `src/config.rs`

语法形态：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
```

解释重点：

- `derive` 让编译器或过程宏自动生成 trait 实现。
- `Debug` 支持 `{:?}`。
- `Clone` 支持显式复制。
- `PartialEq` / `Eq` 支持相等比较。
- `Serialize` / `Deserialize` 来自 serde，用于序列化和反序列化。

## 3. impl 与 trait

### 3.1 inherent impl

代表代码：

- `src/domain/spec.rs`
- `src/domain/source.rs`
- `src/config.rs`

语法形态：

```rust
impl SourceDescriptor {
    pub fn from_spec(...) -> Self {
        ...
    }
}
```

解释重点：

- `impl Type { ... }` 给类型定义方法和关联函数。
- 带 `self` 参数的是方法。
- 不带 `self` 参数的是关联函数。
- `Self` 在 impl 块中表示当前类型。

### 3.2 trait impl

代表代码：

- `src/error.rs`
- `src/status.rs`
- `src/domain/spec.rs`

语法形态：

```rust
impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        ...
    }
}
```

解释重点：

- `impl Trait for Type` 表示给某个类型实现某个 trait。
- trait 规定必须实现哪些方法。
- `Display` 控制 `{}` 格式化。
- `From` 控制类型转换。
- `FromStr` 控制字符串解析。

### 3.3 关联类型

代表代码：

- `src/status.rs`
- `src/repository/contract.rs`

语法形态：

```rust
impl FromStr for Status {
    type Err = ();
}
```

解释重点：

- `type Err = ();` 是 trait 要求的关联类型。
- 关联类型让 trait 的实现者指定某个占位类型。
- `()` 是单元类型，表示没有有意义的信息。

## 4. 所有权、借用、生命周期

### 4.1 `self`、`&self`、`&mut self`

代表代码：

- `src/domain/source.rs`
- `src/domain/spec.rs`
- `src/job/mod.rs`

语法形态：

```rust
pub fn with_identity(mut self, ...) -> Self
pub fn primary(&self) -> Option<&SourceDescriptor>
pub fn primary_mut(&mut self) -> Option<&mut SourceDescriptor>
```

解释重点：

- `self` 会移动整个对象。
- `mut self` 表示拿到所有权后可以修改对象。
- `&self` 是不可变借用。
- `&mut self` 是可变借用。
- 返回 `Self` 可以形成 builder 风格链式调用。

### 4.2 `String` 与 `&str`

代表代码：

- `src/domain/spec.rs`
- `src/config.rs`

语法形态：

```rust
pub fn locator(&self) -> &str
pub fn identity_key(&self) -> String
```

解释重点：

- `String` 拥有字符串数据。
- `&str` 是借用的字符串切片。
- 返回 `&str` 通常意味着返回值依赖某个已有数据。
- 返回 `String` 表示调用者获得独立拥有权。

### 4.3 生命周期

代表代码：

- `src/domain/spec.rs`
- `src/status.rs`

语法形态：

```rust
pub fn scheme(&self) -> &'static str
fmt::Formatter<'_>
```

解释重点：

- `&'static str` 指向整个程序期间有效的字符串字面量。
- `'_` 是匿名生命周期，让编译器推断。
- 生命周期不是让值活得更久，而是描述引用有效范围。

## 5. Option 与 Result

详细文档：

- `docs/rust/result-option.md`

代表代码：

- `src/domain/spec.rs`
- `src/config.rs`
- `src/diagnostics.rs`

语法形态：

```rust
Option<String>
Result<Self, Error>
url::Url::parse(locator).ok()
name.ok_or("missing file name")
some_result.map_err(|err| ...)
```

解释重点：

- `Option<T>` 表示有或没有。
- `Result<T, E>` 表示成功或失败。
- `.ok()` 会把 `Ok(T)` 转成 `Some(T)`，把 `Err(E)` 转成 `None`。
- `.ok_or(...)` 会把 `None` 转成指定错误。
- `.map_err(...)` 转换错误类型。

## 6. 错误传播

代表代码：

- `src/diagnostics.rs`
- `src/config.rs`
- `src/download.rs`

语法形态：

```rust
tokio::fs::create_dir_all(&diagnostics_dir).await?;
serde_json::to_vec_pretty(&diagnostic)
    .map_err(|err| Error::Other(format!("Failed: {err}")))?;
```

解释重点：

- `?` 遇到 `Ok` 会取出成功值。
- `?` 遇到 `Err` 会提前返回。
- 如果错误类型不同，`?` 会尝试通过 `From` 转换。
- `map_err` 可以手动转换错误。

## 7. 模式匹配

### 7.1 `match`

代表代码：

- `src/domain/spec.rs`
- `src/status.rs`
- `src/main.rs`

语法形态：

```rust
match self {
    Self::Http { url } => url,
    Self::Metadata { display_name, info_hash } => ...
}
```

解释重点：

- `match` 必须覆盖所有可能情况。
- enum 可以在 match 中解构。
- 分支返回值类型必须兼容。
- `_` 表示兜底模式。

### 7.2 `matches!`

代表代码：

- `src/domain/spec.rs`
- `src/status.rs`

语法形态：

```rust
matches!(self, Self::Http { .. } | Self::Https { .. })
```

解释重点：

- `matches!` 是宏，返回 bool。
- `|` 表示多个模式任选其一。
- `{ .. }` 忽略结构体风格变体中的字段。

### 7.3 `if let`

代表代码：

- `src/domain/source.rs`
- `src/config.rs`

语法形态：

```rust
if let Some(existing) = self.sources.iter_mut().find(...) {
    *existing = source;
}
```

解释重点：

- `if let` 适合只关心一个匹配分支。
- 匹配成功时绑定内部值。
- 匹配失败时跳过代码块。

### 7.4 `let else`

代表代码：

- `src/config.rs`

语法形态：

```rust
let Some(value) = read_env(key) else {
    return Ok(None);
};
```

解释重点：

- `let else` 用于“必须匹配，否则提前退出”。
- `else` 分支必须发散，例如 `return`、`break`、`panic!`。

## 8. 迭代器与闭包

代表代码：

- `src/domain/source.rs`
- `src/scheduler/planner.rs`
- `src/storage/mapping.rs`

语法形态：

```rust
self.sources
    .iter()
    .filter(|source| source.can_transfer_payload())
    .collect()
```

解释重点：

- `.iter()` 借用遍历。
- `.iter_mut()` 可变借用遍历。
- `.into_iter()` 消费集合。
- `|source| ...` 是闭包。
- 迭代器是惰性的。
- `collect()` 根据目标类型收集结果。

## 9. 泛型、trait bound、动态分发

代表代码：

- `src/request/segment.rs`
- `src/request/task.rs`
- `src/transfer/driver.rs`
- `src/repository/contract.rs`
- `src/p2p/engine.rs`

语法形态：

```rust
impl Into<String>
Box<dyn Trait>
where T: Send + Sync
```

解释重点：

- 泛型让代码适用于多种类型。
- trait bound 限制泛型必须具备某种能力。
- `impl Trait` 常用于参数或返回值的抽象。
- `dyn Trait` 表示运行时动态分发。
- `Box<dyn Trait>` 把 trait object 放到堆上，通过指针持有。

## 10. async 与并发

详细文档：

- `docs/rust/concurrency-primitives.md`

代表代码：

- `src/main.rs`
- `src/job/mod.rs`
- `src/worker/runtime.rs`
- `src/coordinator/mod.rs`
- `src/rate_limiter.rs`

语法形态：

```rust
#[tokio::main]
async fn main() -> ExitCode
some_future.await
Arc<Mutex<T>>
tokio::spawn(async move { ... })
```

解释重点：

- `async fn` 返回 Future。
- `.await` 暂停当前 future，等待异步结果。
- `async move` 会把捕获变量移动进异步任务。
- `Arc` 共享所有权。
- `Mutex` / `RwLock` 管理共享可变状态。

## 11. 宏与属性

代表代码：

- `src/error.rs`
- `src/config.rs`
- `src/cli_app/mod.rs`

语法形态：

```rust
#[derive(Parser)]
#[command(name = "paradown")]
#[arg(short, long)]
format!("metadata::{stable_key}")
write!(f, "{}", self.locator())
```

解释重点：

- `#[...]` 是属性。
- `derive` 可以自动生成 trait 实现。
- `Parser`、`Serialize`、`Deserialize` 等可能是过程宏。
- 带 `!` 的调用通常是宏调用。
- `format!` 返回 `String`。
- `write!` 写入 formatter 或 writer。

## 12. unsafe 与 FFI

代表代码：

- `integrations/libtorrent-engine/src/ffi.rs`
- `integrations/libtorrent-engine/src/native.rs`
- `integrations/libtorrent-engine/build.rs`

语法形态：

```rust
unsafe
extern "C"
#[repr(C)]
*mut T
*const T
CString
CStr
impl Drop for ...
```

解释重点：

- `unsafe` 表示编译器无法完全验证安全性，需要程序员维护不变量。
- `extern "C"` 使用 C ABI。
- `#[repr(C)]` 让结构体布局符合 C 规则。
- 原始指针不受借用检查保护。
- `Drop` 定义值离开作用域时的清理逻辑。

## 13. 当前已有学习文档

- `docs/rust/learning-annotation-standard.md`
- `docs/rust/syntax-index.md`
- `docs/rust/result-option.md`
- `docs/rust/concurrency-primitives.md`

后续建议新增：

- `docs/rust/modules-and-visibility.md`
- `docs/rust/enums-structs-traits.md`
- `docs/rust/ownership-borrowing-lifetimes.md`
- `docs/rust/pattern-matching.md`
- `docs/rust/generics-and-traits.md`
- `docs/rust/iterators-and-closures.md`
- `docs/rust/async-and-concurrency.md`
- `docs/rust/macros-and-attributes.md`
- `docs/rust/unsafe-and-ffi.md`
