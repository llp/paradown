# Rust 学习注释标尺

这份文档是本项目的 Rust 学习注释规范。

目标不是把项目代码变成逐行教程，而是通过这个真实项目建立一套可长期维护的 Rust 语法学习材料：

- 源码中保留最有代表性的语法注释。
- `docs/rust/` 中保存系统化的语法解释。
- 同一种语法只在最合适的位置深入讲一次。
- 后续遇到相同语法时，可以通过语法索引回到代表代码。

## 1. 注释目标

本项目的学习注释主要服务于一个目标：

```text
通过 paradown 这个真实 Rust 项目学习 Rust 语法。
```

因此注释重点是 Rust 语言本身，而不是下载器业务逻辑。

优先解释：

- 模块系统。
- 可见性。
- 所有权。
- 借用。
- 生命周期。
- enum / struct。
- trait / impl。
- 泛型。
- `Option` / `Result`。
- 模式匹配。
- 迭代器。
- 闭包。
- async / await。
- tokio 并发原语。
- 宏和属性。
- FFI / unsafe。

不优先解释：

- 下载器业务背景。
- 每个函数的业务流程细节。
- 每个字段在产品层面的含义。
- 测试断言为什么这么写。

业务说明可以有，但只能服务于理解语法。

## 2. 注释原则

### 2.1 同一种语法只深入注释一次

例如 `Option<T>` 在项目里会出现很多次。

不要每次都写：

```rust
/// `Option<T>` 表示一个值可能存在，也可能不存在...
```

正确做法：

- 在最典型的位置深入解释。
- 在 `docs/rust/syntax-index.md` 建立索引。
- 其他地方如果需要，只写一句短提示。

例如：

```rust
/// 这里返回 `Option<&SourceDescriptor>`，表示可能找到来源，也可能没有。
/// `Option` 的系统解释见 `docs/rust/result-option.md`。
```

### 2.2 源码注释讲“正在发生的语法”

源码注释应该贴着真实代码解释。

好的注释：

```rust
/// `as_ref()` 会把 `Option<String>` 转成 `Option<&String>`，
/// 这样可以借用 id，而不是把 `String` 从 `self` 里移动出来。
```

不好的注释：

```rust
/// Option 是 Rust 中很重要的类型。
```

前者能直接帮助读当前代码；后者太泛。

### 2.3 深入解释放文档，源码放入口

如果解释超过一小段，应该放到 `docs/rust/*.md`。

源码里保留短入口：

```rust
/// 关于 `Result` 和 `Option` 的详细区别，见 `docs/rust/result-option.md`。
```

这样源码不会被大段教程淹没。

### 2.4 不注释测试代码

不要给以下位置新增学习注释：

- `tests/` 目录。
- `integrations/**/tests/` 目录。
- `#[cfg(test)] mod tests` 模块内部。
- `#[test]` 函数。

原因：

- 测试代码通常为了验证行为，不是最稳定的语法学习入口。
- 测试里常有构造数据、断言、边界场景，容易把语法学习和测试意图混在一起。
- 用户明确要求“不注释 test 类型的”。

可以注释：

```rust
#[cfg(test)]
mod tests {
    ...
}
```

之前如果已经加了测试注释，应逐步移除。

### 2.5 不为显而易见的语法制造噪音

不要给每个变量都写：

```rust
// 创建变量
let value = ...
```

也不要给每个 `return`、每个字段赋值写注释。

只有当它涉及 Rust 学习重点时才注释，例如：

- 所有权移动。
- 借用避免 move。
- 生命周期。
- trait object。
- async 边界。
- `?` 的错误转换。
- 宏生成代码。

### 2.6 注释应偏语法，不偏业务

例如看到：

```rust
pub fn push_unique(&mut self, source: SourceDescriptor) {
    if !self.sources.iter().any(|existing| existing.id == source.id) {
        self.sources.push(source);
    }
}
```

注释重点应是：

- `&mut self` 为什么能修改集合。
- `iter()` 借用遍历。
- `any(...)` 返回 bool。
- 闭包 `|existing| ...`。
- `source` 什么时候被移动进 `push`。

而不是大篇幅讲调度器为什么需要来源去重。

## 3. 注释深度分级

### Level 1：短提示

适用于重复语法。

示例：

```rust
/// 这里使用 `Option` 表示字段可能缺失。
```

### Level 2：源码内解释

适用于第一次在代表代码中出现的语法。

示例：

```rust
/// `mut self` 表示函数拿走当前对象的所有权，并允许在函数体内修改它。
/// 修改完成后再返回 `self`，因此可以链式调用。
```

### Level 3：独立学习文档

适用于复杂语法主题。

例如：

- `Result` / `Option`。
- 所有权和生命周期。
- async / await。
- trait object。
- unsafe / FFI。

源码里只放文档入口。

## 4. 总体策略

采用“语法主题覆盖”策略，而不是“文件全量覆盖”策略。

也就是说：

```text
不是每个 Rust 文件都要写很多注释。
而是每个重要 Rust 语法都要找到一个最典型代码位置深入解释。
```

这样可以避免两个问题：

- 源码注释过多，真实逻辑难以阅读。
- 同一种语法重复解释，后期维护困难。

## 5. 分批注释计划

### 第 1 批：模块系统、可见性、导出

代表文件：

- `src/lib.rs`
- `src/main.rs`
- `src/domain/mod.rs`
- `src/request/mod.rs`
- `src/p2p/mod.rs`

重点语法：

```rust
mod xxx;
pub mod xxx;
pub(crate) mod xxx;
pub use xxx::Yyy;
crate::xxx
super::xxx
```

学习目标：

- 理解 crate 根。
- 理解模块声明。
- 理解公开模块和私有模块。
- 理解 re-export。
- 理解 API 门面。

### 第 2 批：enum、struct、derive、trait 实现

代表文件：

- `src/error.rs`
- `src/status.rs`
- `src/events.rs`
- `src/domain/spec.rs`
- `src/domain/source.rs`

重点语法：

```rust
enum Error { ... }
struct SourceDescriptor { ... }
#[derive(Debug, Clone, Serialize, Deserialize)]
impl fmt::Display for Status
impl From<url::ParseError> for Error
```

学习目标：

- 理解 enum 三种变体形式。
- 理解 struct 字段所有权。
- 理解 derive 自动生成 trait 实现。
- 理解 `impl Trait for Type`。
- 理解关联类型。

### 第 3 批：所有权、借用、生命周期

代表文件：

- `src/domain/spec.rs`
- `src/domain/source.rs`
- `src/job/mod.rs`
- `src/storage/mapping.rs`

重点语法：

```rust
self
&self
&mut self
String
&str
&'static str
Option<&T>
Option<&mut T>
clone()
to_string()
as_ref()
as_deref()
```

学习目标：

- 理解值移动。
- 理解借用。
- 理解可变借用。
- 理解生命周期省略。
- 理解返回引用和返回拥有值的区别。

### 第 4 批：Result、Option、错误传播

代表文件：

- `src/domain/spec.rs`
- `src/config.rs`
- `src/error.rs`
- `src/diagnostics.rs`

已有文档：

- `docs/rust/result-option.md`

重点语法：

```rust
Result<T, E>
Option<T>
?
.ok()
.err()
.ok_or(...)
.ok_or_else(...)
.map(...)
.and_then(...)
.map_err(...)
.unwrap_or(...)
.unwrap_or_else(...)
```

学习目标：

- 理解 `Option` 和 `Result` 的差异。
- 理解错误为什么能通过 `?` 自动返回。
- 理解 `From` 与 `?` 的关系。
- 理解链式转换。

### 第 5 批：模式匹配

代表文件：

- `src/domain/spec.rs`
- `src/domain/source.rs`
- `src/status.rs`
- `src/config.rs`

重点语法：

```rust
match value { ... }
Self::Http { .. }
Status::Failed(_)
pattern | pattern
matches!(...)
if let Some(x) = ...
let Some(value) = ... else { ... };
_ => ...
```

学习目标：

- 理解 pattern matching。
- 理解 `_` 和 `..`。
- 理解 enum 解构。
- 理解 `if let`。
- 理解 `let else`。
- 理解 `matches!` 宏。

### 第 6 批：泛型、trait bound、trait object

代表文件：

- `src/request/segment.rs`
- `src/request/task.rs`
- `src/transfer/driver.rs`
- `src/repository/contract.rs`
- `src/p2p/engine.rs`

重点语法：

```rust
impl Into<String>
impl Trait
Box<dyn Trait>
trait X { ... }
type Error = ...
where ...
```

学习目标：

- 理解泛型参数。
- 理解 trait 约束。
- 理解 `impl Trait`。
- 理解动态分发。
- 理解 `Box<dyn Trait>`。
- 理解关联类型。

### 第 7 批：迭代器、闭包、链式调用

代表文件：

- `src/domain/source.rs`
- `src/scheduler/planner.rs`
- `src/storage/mapping.rs`
- `src/payload/file_map.rs`

重点语法：

```rust
.iter()
.iter_mut()
.into_iter()
.map(|x| ...)
.filter(|x| ...)
.find(...)
.any(...)
.collect()
.fold(...)
```

学习目标：

- 理解借用遍历。
- 理解可变借用遍历。
- 理解消费式遍历。
- 理解闭包捕获。
- 理解迭代器惰性。
- 理解 `collect` 的类型推断。

### 第 8 批：async、await、tokio、并发原语

代表文件：

- `src/main.rs`
- `src/job/mod.rs`
- `src/worker/runtime.rs`
- `src/coordinator/mod.rs`
- `src/rate_limiter.rs`

已有文档：

- `docs/rust/concurrency-primitives.md`

重点语法：

```rust
#[tokio::main]
async fn
.await
tokio::spawn
Arc<T>
Mutex<T>
RwLock<T>
OnceCell<T>
Semaphore
```

学习目标：

- 理解 async 函数返回 Future。
- 理解 `.await`。
- 理解 tokio runtime。
- 理解共享所有权。
- 理解异步锁。
- 理解一次性初始化。

### 第 9 批：宏、属性、条件编译

代表文件：

- `src/config.rs`
- `src/error.rs`
- `src/cli_app/mod.rs`
- `integrations/libtorrent-engine/build.rs`

重点语法：

```rust
#[derive(...)]
#[derive(Parser)]
#[command(...)]
#[arg(...)]
#[cfg(...)]
format!
write!
matches!
```

学习目标：

- 理解属性。
- 理解 derive 宏。
- 理解过程宏。
- 理解普通宏调用。
- 理解条件编译。

### 第 10 批：unsafe、FFI、C/C++ 边界

代表文件：

- `integrations/libtorrent-engine/src/ffi.rs`
- `integrations/libtorrent-engine/src/native.rs`
- `integrations/libtorrent-engine/build.rs`

重点语法：

```rust
unsafe
extern "C"
#[repr(C)]
*mut T
*const T
CString
CStr
Drop
```

学习目标：

- 理解 unsafe 的边界。
- 理解 FFI ABI。
- 理解 C 布局。
- 理解原始指针。
- 理解 Rust 和 C/C++ 的所有权边界。
- 理解 `Drop` 析构。

## 6. 代码注释模板

### 6.1 简短源码注释模板

```rust
/// 这里使用 `as_ref()` 是为了把 `Option<String>` 借用成 `Option<&String>`，
/// 避免把 `String` 从 `self` 中移动出来。
```

### 6.2 长解释入口模板

```rust
/// 这里涉及 `Result` 和 `Option` 的转换。
/// 系统解释见 `docs/rust/result-option.md`。
```

### 6.3 不推荐模板

不要写这种没有学习价值的注释：

```rust
// 调用函数
foo();

// 返回结果
result
```

## 7. 每批完成标准

每一批注释完成后都要检查：

```text
1. 没有给 tests 目录或 #[cfg(test)] mod tests 添加学习注释。
2. 同一种语法没有重复长篇解释。
3. 源码注释能直接对应当前代码。
4. 复杂主题已经落到 docs/rust/*.md。
5. cargo fmt --check 通过。
6. 能跑的相关测试已通过。
7. 每批单独提交，提交信息说明语法主题。
```

## 8. 推荐提交粒度

每批一个提交。

提交信息示例：

```text
docs: annotate rust module and visibility syntax
docs: annotate rust enums structs and traits
docs: annotate rust ownership and lifetimes
docs: annotate rust pattern matching syntax
```

这样以后回看学习过程时，可以按语法主题查看历史。
