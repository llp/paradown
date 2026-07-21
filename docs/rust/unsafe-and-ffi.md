# Rust 语法：unsafe 与 FFI

代表代码：

- `integrations/libtorrent-engine/src/ffi.rs`
- `integrations/libtorrent-engine/src/native.rs`
- `integrations/libtorrent-engine/build.rs`

## 1. FFI 是什么

FFI 是 Foreign Function Interface。

它让 Rust 调用其他语言的代码，或让其他语言调用 Rust。

本项目通过 `cxx` crate 连接 Rust 和 C++ libtorrent 集成层。

## 2. `cxx::bridge`

```rust
#[cxx::bridge(namespace = "paradown_libtorrent")]
mod ffi {
    ...
}
```

`cxx::bridge` 是过程宏。

它读取模块中的 Rust/C++ 边界声明，然后生成桥接代码。

## 3. `unsafe extern "C++"`

```rust
unsafe extern "C++" {
    type NativeEngine;
    fn new_native_engine(...) -> Result<UniquePtr<NativeEngine>>;
}
```

`extern "C++"` 表示这些项来自 C++ ABI 边界。

`unsafe` 表示 Rust 编译器无法完全验证边界另一侧是否满足 Rust 安全规则。

## 4. opaque type

```rust
type NativeEngine;
```

这里只声明类型存在，但不暴露它的字段布局。

Rust 侧不能直接构造或读取它，只能通过桥接函数使用。

## 5. `Pin<&mut T>`

```rust
Pin<&mut NativeEngine>
```

`Pin` 表示这个值不能被随意移动。

FFI 中有些 C++ 对象可能依赖稳定地址，因此桥接接口会要求 pinned mutable reference。

## 6. `UniquePtr<T>`

```rust
UniquePtr<NativeEngine>
```

`UniquePtr` 对应 C++ 的 `std::unique_ptr` 所有权模型。

它表示唯一拥有某个 C++ 对象。

## 7. 为什么 unsafe 不是“随便写”

`unsafe` 的意思不是关闭安全规则。

它的意思是：

```text
有些安全条件编译器无法证明，需要程序员保证。
```

例如：

- C++ 返回的指针必须有效。
- 字符串编码和生命周期必须符合桥接要求。
- C++ 对象不能在 Rust 仍使用时被释放。

## 8. build.rs 与 FFI

`build.rs` 会在编译 Rust crate 之前运行。

本项目用它：

- 调用 `cxx_build::bridge("src/ffi.rs")` 生成桥接代码。
- 编译 C++ 文件。
- 查找 libtorrent 和 boost 头文件。
- 输出 Cargo 链接指令。
