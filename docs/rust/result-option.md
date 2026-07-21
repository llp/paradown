# Rust 学习笔记：Result、Option 以及它们之间的转换

这篇文档是单独的 Rust 技术学习笔记，不属于项目业务文档。

它主要解释这句代码背后的概念：

```rust
url::Url::parse(locator).ok()
```

这句代码的意思是：

```text
把 Result<Url, _> 转成 Option<Url>
```

如果你对 `Result<Url, _>` 和 `Option<Url>` 的区别还不熟，这篇文档可以从头看。

## 1. 先理解 Option<T>

`Option<T>` 表示“一个值可能存在，也可能不存在”。

它只有两个可能：

```rust
enum Option<T> {
    Some(T),
    None,
}
```

比如：

```rust
let name: Option<String> = Some(String::from("file.bin"));
let missing_name: Option<String> = None;
```

含义是：

```text
Some(value) = 有值
None        = 没有值
```

所以：

```rust
Option<Url>
```

意思是：

```text
可能有一个 Url，也可能没有 Url
```

它不关心“为什么没有”。

例如：

```rust
fn last_path_segment(url: &url::Url) -> Option<&str> {
    url.path_segments()?.next_back()
}
```

如果 URL 有最后一段路径，就返回：

```rust
Some("file.bin")
```

如果没有，就返回：

```rust
None
```

这里的重点是：`Option` 只表达“有或没有”，不携带错误原因。

## 2. 再理解 Result<T, E>

`Result<T, E>` 表示“一个操作可能成功，也可能失败”。

它也只有两个可能：

```rust
enum Result<T, E> {
    Ok(T),
    Err(E),
}
```

比如：

```rust
let parsed: Result<url::Url, url::ParseError> =
    url::Url::parse("https://example.com/file.bin");
```

含义是：

```text
Ok(value) = 成功，并且里面有成功结果
Err(err)  = 失败，并且里面有失败原因
```

所以：

```rust
Result<Url, url::ParseError>
```

意思是：

```text
可能成功得到一个 Url，也可能失败得到一个 url::ParseError
```

和 `Option<Url>` 不同的是，`Result` 会保留失败原因。

例如：

```rust
let parsed = url::Url::parse("https://example.com/file.bin");
```

成功时大概是：

```rust
Ok(Url { ... })
```

失败时大概是：

```rust
Err(url::ParseError::RelativeUrlWithoutBase)
```

这里的重点是：`Result` 不只是表达“没有值”，还解释“为什么失败”。

## 3. Option 和 Result 的核心区别

最短的区别：

```text
Option<T>    = 有 T 或没有 T
Result<T, E> = 有 T 或有错误 E
```

也可以这样记：

```text
Option 适合“不存在也正常”的场景。
Result 适合“失败需要解释原因”的场景。
```

比如：

```rust
fn file_name_hint_from_locator(locator: &str) -> Option<String>
```

这个函数返回 `Option<String>` 很合理。

因为“从 URL 里推断文件名”本来就是一个尽力而为的操作：

```text
能推断出来就返回 Some(filename)
推断不出来就返回 None
```

调用者通常不需要知道到底是：

```text
URL 解析失败
path 为空
path 最后一段为空字符串
```

这些细节都可以被统一看成“没有文件名提示”。

但是：

```rust
pub fn parse(locator: impl Into<String>) -> Result<Self, Error>
```

这个函数返回 `Result<Self, Error>` 很合理。

因为“解析下载规格”是项目的核心流程，如果失败，调用者通常需要知道失败原因：

```text
是不是 URL 格式错了？
是不是协议不支持？
是不是其他解析错误？
```

所以这里不能简单返回 `Option<DownloadSpec>`，否则错误信息会丢失。

## 4. Result<Url, _> 里的下划线是什么意思

你看到的：

```rust
Result<Url, _>
```

这里的 `_` 表示“让 Rust 编译器自己推断这个类型”。

`Result` 有两个泛型参数：

```rust
Result<T, E>
```

其中：

```text
T = 成功时的类型
E = 失败时的错误类型
```

所以：

```rust
Result<Url, _>
```

意思是：

```text
成功时是 Url，失败时是什么错误类型让编译器推断
```

在这段代码里：

```rust
url::Url::parse(locator)
```

真实返回类型是：

```rust
Result<url::Url, url::ParseError>
```

所以：

```rust
Result<Url, _>
```

在这里基本等价于：

```rust
Result<Url, url::ParseError>
```

只是文档里写 `_` 可以把注意力放在“成功时得到 Url”上，不必总是把错误类型完整写出来。

## 5. `.ok()`：把 Result 转成 Option

现在看这句：

```rust
url::Url::parse(locator).ok()
```

`url::Url::parse(locator)` 返回的是：

```rust
Result<url::Url, url::ParseError>
```

调用 `.ok()` 以后，变成：

```rust
Option<url::Url>
```

转换规则非常简单：

```rust
Ok(value).ok()  -> Some(value)
Err(error).ok() -> None
```

也就是：

```text
成功结果保留
错误原因丢掉
```

例子：

```rust
let parsed = url::Url::parse("https://example.com/file.bin");
let maybe_url = parsed.ok();
```

如果解析成功：

```rust
Ok(url)
```

会变成：

```rust
Some(url)
```

如果解析失败：

```rust
Err(parse_error)
```

会变成：

```rust
None
```

注意：`.ok()` 会丢弃错误信息。

所以它适合这种情况：

```text
我只关心成功时的值
失败原因对当前逻辑不重要
```

## 6. 为什么 file_name_hint_from_locator 里适合用 .ok()

项目里的函数：

```rust
pub fn file_name_hint_from_locator(locator: &str) -> Option<String> {
    url::Url::parse(locator).ok().and_then(|url| {
        url.path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|segment| !segment.is_empty())
            .map(|segment| segment.to_string())
    })
}
```

它的目标不是“严肃解析一个下载规格”。

它只是想尝试从 locator 里猜一个文件名。

所以：

```rust
url::Url::parse(locator).ok()
```

这里的含义是：

```text
如果 locator 是合法 URL，就继续提取 path 最后一段
如果 locator 不是合法 URL，就直接当作没有文件名提示
```

这就是为什么它把：

```rust
Result<Url, ParseError>
```

转成：

```rust
Option<Url>
```

因为这个函数最终返回的是：

```rust
Option<String>
```

它只想告诉调用者：

```text
有文件名提示
或者
没有文件名提示
```

不想把 URL 解析失败的具体原因暴露出去。

## 7. 如果不用 .ok()，可以怎么写

这段：

```rust
url::Url::parse(locator).ok().and_then(|url| {
    url.path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
})
```

可以写成更展开的 `match`：

```rust
pub fn file_name_hint_from_locator(locator: &str) -> Option<String> {
    let parsed = url::Url::parse(locator);

    match parsed {
        Ok(url) => {
            match url.path_segments() {
                Some(mut segments) => {
                    match segments.next_back() {
                        Some(segment) if !segment.is_empty() => Some(segment.to_string()),
                        _ => None,
                    }
                }
                None => None,
            }
        }
        Err(_err) => None,
    }
}
```

这和原来的链式写法逻辑一样。

区别只是：

```text
链式写法更短
match 写法更适合刚开始学习时逐步理解
```

其中：

```rust
Err(_err) => None
```

就是 `.ok()` 做的事情：

```text
遇到 Err，就丢掉错误，返回 None
```

## 8. `.err()`：把 Result 的错误取出来

除了 `.ok()`，`Result` 还有一个相反方向的方法：

```rust
.err()
```

它会把：

```rust
Result<T, E>
```

转成：

```rust
Option<E>
```

转换规则：

```rust
Ok(value).err() -> None
Err(error).err() -> Some(error)
```

也就是：

```text
成功值丢掉
错误值保留
```

例子：

```rust
let err = url::Url::parse("/tmp/archive.torrent").err();
```

如果解析失败，就得到：

```rust
Some(url::ParseError::RelativeUrlWithoutBase)
```

如果解析成功，就得到：

```rust
None
```

## 9. `ok_or`：把 Option 转成 Result

反过来，`Option<T>` 也可以转成 `Result<T, E>`。

最常用的是：

```rust
.ok_or(error)
```

转换规则：

```rust
Some(value).ok_or(error) -> Ok(value)
None.ok_or(error)        -> Err(error)
```

例子：

```rust
fn require_file_name(name: Option<String>) -> Result<String, &'static str> {
    name.ok_or("missing file name")
}
```

如果 `name` 是：

```rust
Some(String::from("file.bin"))
```

结果就是：

```rust
Ok(String::from("file.bin"))
```

如果 `name` 是：

```rust
None
```

结果就是：

```rust
Err("missing file name")
```

这里的意思是：

```text
本来只是“可能没有值”
现在我要把“没有值”解释成一个具体错误
```

## 10. `ok_or_else`：懒生成错误

还有一个常用变体：

```rust
.ok_or_else(|| error)
```

它和 `ok_or` 的区别是：

```text
ok_or(error)         会先创建 error
ok_or_else(|| error) 只有真的遇到 None 时才创建 error
```

如果错误值很简单，例如字符串字面量：

```rust
name.ok_or("missing file name")
```

可以用 `ok_or`。

如果错误值需要分配内存或计算，例如：

```rust
name.ok_or_else(|| format!("missing file name for locator: {locator}"))
```

通常用 `ok_or_else` 更合适。

## 11. 用表格记住转换关系

| 从 | 方法 | 到 | 成功/有值时 | 失败/无值时 |
| --- | --- | --- | --- | --- |
| `Result<T, E>` | `.ok()` | `Option<T>` | `Ok(value)` -> `Some(value)` | `Err(error)` -> `None`，错误丢弃 |
| `Result<T, E>` | `.err()` | `Option<E>` | `Ok(value)` -> `None`，成功值丢弃 | `Err(error)` -> `Some(error)` |
| `Option<T>` | `.ok_or(error)` | `Result<T, E>` | `Some(value)` -> `Ok(value)` | `None` -> `Err(error)` |
| `Option<T>` | `.ok_or_else(|| error)` | `Result<T, E>` | `Some(value)` -> `Ok(value)` | `None` -> `Err(error)`，错误懒生成 |

## 12. 什么时候用 Option，什么时候用 Result

优先问自己一个问题：

```text
调用者需要知道失败原因吗？
```

如果不需要，使用 `Option<T>`：

```rust
fn file_name_hint_from_locator(locator: &str) -> Option<String>
```

因为它只是一个提示，不一定有，失败原因没那么重要。

如果需要，使用 `Result<T, E>`：

```rust
pub fn parse(locator: impl Into<String>) -> Result<Self, Error>
```

因为解析下载规格失败时，调用者应该知道为什么失败。

## 13. 和 spec.rs 里的代码对应起来

`parse` 是核心解析函数，所以它保留错误：

```rust
pub fn parse(locator: impl Into<String>) -> Result<Self, Error>
```

它会根据不同错误返回：

```rust
Err(Error::UnsupportedProtocol(...))
Err(err.into())
```

`file_name_hint_from_locator` 是辅助推断函数，所以它丢弃错误：

```rust
pub fn file_name_hint_from_locator(locator: &str) -> Option<String>
```

它用 `.ok()` 把 URL 解析错误统一折叠成 `None`：

```rust
url::Url::parse(locator).ok()
```

所以这两个函数的设计不是随便选的：

```text
parse                     -> 需要错误原因 -> Result
file_name_hint_from_locator -> 只需要有没有 -> Option
```

## 14. 一个记忆口诀

```text
Option 问：有没有？
Result 问：成没成？没成为什么？
```

再短一点：

```text
Option = Some / None
Result = Ok / Err
```

`.ok()` 的作用：

```text
Ok 变 Some
Err 变 None
错误信息丢掉
```

`.ok_or(...)` 的作用：

```text
Some 变 Ok
None 变 Err
给“没有值”补一个错误原因
```
