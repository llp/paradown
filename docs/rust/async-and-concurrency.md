# Rust 语法：async、await、tokio 与并发

代表代码：

- `src/main.rs`
- `src/rate_limiter.rs`
- `src/worker/runtime.rs`
- `src/job/mod.rs`
- `src/coordinator/mod.rs`

相关文档：

- `docs/rust/concurrency-primitives.md`

## 1. `async fn`

`async fn` 不会立刻执行完整函数体。

它会返回一个 Future。

```rust
async fn run() -> Result<(), Error>
```

调用后需要 `.await` 才会推进执行：

```rust
run().await
```

## 2. `.await`

`.await` 表示等待一个 Future 完成。

在 Tokio 中，等待期间当前任务可以让出执行权，让运行时调度其他任务。

它不是简单阻塞线程。

## 3. `#[tokio::main]`

```rust
#[tokio::main]
async fn main() -> ExitCode
```

普通 Rust `main` 不能直接异步。

`#[tokio::main]` 是属性宏，会生成启动 Tokio runtime 的同步入口，再在 runtime 里执行 async main。

## 4. `tokio::spawn`

```rust
tokio::spawn(async move {
    ...
})
```

创建一个异步任务。

`async move` 会把闭包捕获的变量移动进任务中，这样任务可以独立运行。

## 5. `Arc<T>`

`Arc` 是原子引用计数指针，用来在多线程或多异步任务之间共享所有权。

```rust
Arc<Task>
```

克隆 `Arc` 不会复制 `Task`，只会增加引用计数。

## 6. 异步锁

Tokio 提供异步版本的锁：

```rust
tokio::sync::Mutex<T>
tokio::sync::RwLock<T>
```

获取锁需要 `.await`：

```rust
let mut value = mutex.lock().await;
```

等待锁时，任务让出执行权，而不是阻塞整个线程。

## 7. 原子类型

```rust
AtomicU64
Ordering::Relaxed
```

原子类型适合简单计数器或状态位。

`Ordering::Relaxed` 只保证单次原子读写本身不会撕裂，不提供额外的跨线程顺序保证。

## 8. `tokio::select!`

```rust
tokio::select! {
    _ = tokio::time::sleep_until(wait_until) => return,
    changed = updates.changed() => { ... }
}
```

`select!` 同时等待多个异步分支，哪个先完成就执行哪个分支。

它适合“等待超时或等待配置变化”这类场景。

## 9. watch channel

```rust
let (tx, rx) = watch::channel(());
```

`watch` 通道保存最新值。

接收端可以等待值变化：

```rust
updates.changed().await
```

项目中的限速器用它通知等待中的任务：限速配置已经变化，可以重新计算等待时间。
