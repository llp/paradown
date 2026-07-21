# Rust 语法：宏与属性

代表代码：

- `src/cli_app/mod.rs`
- `src/error.rs`
- `src/main.rs`
- `integrations/libtorrent-engine/build.rs`

## 1. 属性

属性写成：

```rust
#[something]
#[something(...)]
```

它们给紧随其后的 item、字段或表达式附加编译期信息。

## 2. derive 宏

```rust
#[derive(Debug, Clone)]
```

`derive` 会自动生成 trait 实现。

有些 derive 来自标准库，例如：

- `Debug`
- `Clone`
- `Default`
- `PartialEq`

有些来自外部 crate，例如：

- `Serialize`
- `Deserialize`
- `Parser`
- `thiserror::Error`

## 3. 属性宏

```rust
#[tokio::main]
```

属性宏可以改写或生成代码。

`#[tokio::main]` 会生成启动 Tokio runtime 的入口。

## 4. derive(Parser)

```rust
#[derive(Parser, Debug)]
#[command(name = "paradown")]
struct Cli {
    #[arg(short, long)]
    config: Option<PathBuf>,
}
```

`Parser` 来自 clap。

它会读取结构体字段和 `#[arg(...)]` 属性，生成命令行解析代码。

## 5. 条件编译：`cfg`

```rust
#[cfg(feature = "native-libtorrent")]
```

`cfg` 在编译期决定代码是否参与编译。

如果 feature 没开启，被标记的代码就像不存在一样。

常见形态：

```rust
#[cfg(test)]
#[cfg(feature = "...")]
#[cfg(not(feature = "..."))]
```

## 6. 普通宏调用

带 `!` 的调用通常是宏：

```rust
format!("task-{}.json", id)
write!(f, "{}", value)
matches!(value, Pattern)
println!("cargo:rerun-if-changed=build.rs")
```

宏和普通函数不同，它们在编译期展开，可以接受更灵活的语法。

## 7. Cargo build script 输出

`build.rs` 里常见：

```rust
println!("cargo:rerun-if-changed=build.rs");
println!("cargo:rustc-link-lib=torrent-rasterbar");
```

这些不是普通日志，而是 build script 和 Cargo 通信的协议。

Cargo 读取这些输出后决定：

- 什么时候重新运行 build script。
- 给 rustc 增加哪些链接参数。
