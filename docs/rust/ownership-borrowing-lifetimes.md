# Rust 语法：所有权、借用、生命周期

这篇文档对应项目中的第 3 批学习注释。

代表代码：

- `src/domain/spec.rs`
- `src/domain/source.rs`
- `src/request/task.rs`
- `src/storage/mapping.rs`
- `src/diagnostics.rs`

## 1. 所有权是什么

Rust 中每个值都有一个所有者。

```rust
let name = String::from("file.bin");
```

这里 `name` 拥有这个 `String`。

当所有者离开作用域，值会被释放。

```rust
{
    let name = String::from("file.bin");
}
// name 离开作用域，String 被释放
```

## 2. move：所有权移动

默认情况下，`String`、`Vec<T>`、`PathBuf` 这类拥有堆内存的数据会发生 move。

```rust
let a = String::from("file.bin");
let b = a;
```

此后 `a` 不再可用，因为所有权已经移动给 `b`。

项目里常见：

```rust
pub fn build(self) -> TaskRequest {
    TaskRequest {
        spec: self.spec,
        file_name: self.file_name,
    }
}
```

`build(self)` 取得 builder 的所有权，所以可以把字段直接移动进最终结构体。

## 3. 借用：`&T`

借用表示只临时查看一个值，不取得所有权。

```rust
fn locator(&self) -> &str
```

`&self` 是不可变借用。

调用者把对象借给函数，函数不能移动或销毁对象。

项目代表代码：

```rust
pub fn primary(&self) -> Option<&SourceDescriptor>
```

这个函数返回来源的引用，不复制整个 `SourceDescriptor`。

## 4. 可变借用：`&mut T`

可变借用允许修改被借用的值。

```rust
pub fn push_unique(&mut self, source: SourceDescriptor)
```

`&mut self` 表示这个方法可以修改 `self.sources`。

Rust 同一时间只允许：

```text
一个可变引用
或者
多个不可变引用
```

这样可以在编译期避免数据竞争。

## 5. `self`、`&self`、`&mut self`

| 接收器 | 含义 | 常见场景 |
| --- | --- | --- |
| `self` | 取得所有权 | 消费对象、builder 的 `build` |
| `mut self` | 取得所有权且可修改 | builder 链式设置 |
| `&self` | 不可变借用 | 只读查询 |
| `&mut self` | 可变借用 | 修改对象内部字段 |

项目代表代码：

```rust
pub fn file_name(mut self, name: impl Into<String>) -> Self
```

这种 builder 方法取得 `self`，修改字段，再返回 `Self`。

## 6. `String` 与 `&str`

`String` 拥有字符串数据。

`&str` 是字符串切片，只借用数据。

```rust
pub fn locator(&self) -> &str
```

这个返回值通常借用 `self` 内部的字符串。

```rust
pub fn identity_key(&self) -> String
```

这个返回值是新生成或克隆出来的独立字符串。

## 7. `&'static str`

```rust
pub fn scheme(&self) -> &'static str
```

`'static` 表示这个引用在整个程序运行期间有效。

字符串字面量天然是 `&'static str`：

```rust
"http"
"https"
```

所以 `scheme()` 可以返回 `&'static str`。

但 `locator()` 不能随便返回 `&'static str`，因为它可能返回 `self` 内部的 `String` 字段。

## 8. `clone()` 与 `to_string()`

`clone()` 复制已有拥有值：

```rust
url.clone()
```

`to_string()` 通常从可显示的值或 `&str` 创建新的 `String`：

```rust
segment.to_string()
```

选择它们的核心问题是：

```text
这里能否借用？
这里是否需要返回一个独立拥有的数据？
```

## 9. `as_ref()` 与 `as_deref()`

`as_ref()` 把 `Option<T>` 变成 `Option<&T>`。

```rust
let maybe_ref: Option<&String> = maybe_string.as_ref();
```

它常用于避免把值从结构体里 move 出来。

`as_deref()` 会进一步把 `Option<String>` 变成 `Option<&str>`。

```rust
let maybe_str: Option<&str> = maybe_string.as_deref();
```

项目代表代码：

```rust
info_hash.as_deref().or(display_name.as_deref())
```

## 10. 生命周期不是延长生命

生命周期标注只是描述引用之间的有效范围关系。

它不会让一个值活得更久。

错误例子：

```rust
fn bad() -> &str {
    let s = String::from("hello");
    &s
}
```

`s` 在函数结束时释放，所以不能返回指向它的引用。

正确做法通常是返回拥有值：

```rust
fn good() -> String {
    String::from("hello")
}
```
