# Rust 语法：模式匹配

代表代码：

- `src/domain/spec.rs`
- `src/domain/source.rs`
- `src/status.rs`
- `src/config.rs`

## 1. `match`

`match` 会把一个值和多个模式逐个比较。

```rust
match self {
    Self::Http { url } => url,
    Self::Magnet { uri } => uri,
}
```

Rust 的 `match` 必须穷尽所有情况。

如果是 enum，所有变体都必须被覆盖，除非使用 `_` 兜底。

## 2. enum 解构

结构体风格 enum 变体：

```rust
Self::Http { url }
```

表示：

```text
匹配 Http 变体，并把 url 字段绑定到局部变量 url。
```

忽略字段：

```rust
Self::Http { .. }
```

表示：

```text
只关心它是不是 Http，不关心里面的字段。
```

元组风格 enum 变体：

```rust
Status::Failed(_)
```

表示：

```text
匹配 Failed 变体，但忽略里面的 Error。
```

## 3. `_`

`_` 是通配模式，表示“匹配任何值，但不绑定变量”。

```rust
_ => Err(())
```

常用于兜底分支。

## 4. `|`

`|` 在模式中表示“或者”。

```rust
Status::Completed | Status::Canceled | Status::Deleted
```

任意一个模式匹配成功即可。

## 5. `matches!`

`matches!` 是标准宏，返回 bool。

```rust
matches!(self, Self::Http { .. } | Self::Https { .. })
```

等价于：

```rust
match self {
    Self::Http { .. } | Self::Https { .. } => true,
    _ => false,
}
```

适合只想判断是否匹配，不需要使用匹配到的值。

## 6. `if let`

`if let` 适合只关心一个模式。

```rust
if let Some(existing) = maybe_source {
    ...
}
```

如果是 `Some`，绑定内部值；如果是 `None`，跳过代码块。

## 7. `let else`

`let else` 适合“必须匹配，否则提前退出”。

```rust
let Some(value) = read_env(key) else {
    return Ok(None);
};
```

`else` 分支必须发散，例如：

- `return`
- `break`
- `continue`
- `panic!`

## 8. match ergonomics

当匹配引用时，Rust 会自动帮你处理一部分 `&` 和解引用。

例如：

```rust
fn map_error_to_exit_code(err: &Error) {
    match err {
        Error::ConfigError(_) => ...
    }
}
```

虽然 `err` 是 `&Error`，分支里仍然可以写 `Error::ConfigError(_)`。

这是 Rust 的 match ergonomics，目的是减少样板代码。
