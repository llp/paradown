# Rust 语法：泛型、trait、trait object

代表代码：

- `src/request/segment.rs`
- `src/request/task.rs`
- `src/transfer/driver.rs`
- `src/repository/contract.rs`
- `src/p2p/engine.rs`

## 1. 泛型

泛型让代码可以作用于多种类型。

```rust
Option<T>
Result<T, E>
Vec<T>
```

这里的 `T`、`E` 是类型参数。

## 2. `impl Into<String>`

```rust
pub fn source_id(mut self, source_id: impl Into<String>) -> Self
```

`impl Into<String>` 表示：

```text
调用者可以传入任何能转换成 String 的类型。
```

常见输入：

- `String`
- `&str`

函数内部调用：

```rust
source_id.into()
```

取得真正的 `String`。

## 3. trait

trait 定义一组类型必须提供的能力。

```rust
trait Repository {
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, Error>;
}
```

实现这个 trait 的类型必须提供这些方法。

## 4. trait bound

```rust
pub trait Repository: Send + Sync
```

这里的 `: Send + Sync` 是 trait bound。

它表示：

```text
任何实现 Repository 的类型，也必须满足 Send 和 Sync。
```

`Send` 表示值可以安全地移动到另一个线程。

`Sync` 表示多个线程可以安全地共享这个类型的引用。

## 5. async trait

Rust 原生 trait 中的 async fn 支持仍有使用限制，项目使用 `async_trait` 宏降低复杂度。

```rust
#[async_trait]
pub trait Repository {
    async fn load_tasks(&self) -> Result<Vec<DBDownloadTask>, Error>;
}
```

`#[async_trait]` 会生成额外代码，把 async trait 方法转换成可编译的形式。

## 6. trait object：`dyn Trait`

```rust
&'static dyn TransferDriver
```

`dyn TransferDriver` 表示：

```text
某个实现了 TransferDriver 的具体类型，但具体是谁在编译期不写死。
```

这是动态分发。

## 7. 为什么常和引用或 Box 一起出现

trait object 的大小在编译期不固定，所以通常不能裸用：

```rust
dyn TransferDriver
```

常见写法：

```rust
&dyn TransferDriver
Box<dyn TransferDriver>
Arc<dyn TransferDriver>
```

它们都通过固定大小的指针间接持有 trait object。

## 8. 静态分发与动态分发

泛型通常是静态分发：

```rust
fn parse(value: impl Into<String>)
```

编译器会为具体类型生成代码。

trait object 是动态分发：

```rust
fn driver_for_source(...) -> &'static dyn TransferDriver
```

运行时通过 vtable 调用实际类型的方法。
