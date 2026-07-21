# Rust 并发原语：Arc, Mutex, RwLock 及其他

在 Rust 中编写并发代码时，理解如何安全地共享和修改数据至关重要。Rust 的所有权系统和类型安全机制在编译时就帮助我们避免了许多并发问题（如数据竞争），但对于需要共享可变状态的场景，我们仍然需要使用特定的并发原语。

以下是几个核心的并发原语及其区别：

---

## 1. `Mutex<T>` (互斥锁)

- **作用**: `Mutex` (Mutual Exclusion) 互斥锁用于**保护共享数据，确保在任何给定时间点，只有一个线程（或异步任务）能够访问被它保护的数据**。
- **特点**:
    - **独占访问**: 无论你是要读取数据还是修改数据，都必须先获取锁。一旦锁被一个线程持有，其他尝试获取锁的线程都会被阻塞，直到锁被释放。
    - **线程安全**: 强制执行互斥访问，防止数据竞争。
    - **内部可变性**: 即使 `Mutex` 本身是通过不可变引用 (`&Mutex<T>`) 访问的，你仍然可以通过获取锁来获得对内部 `T` 的可变访问 (`&mut T`)。这是通过 Rust 的“内部可变性”模式实现的。
- **使用场景**:
    - 当你需要**修改**共享数据，并且不希望在修改过程中有其他线程同时读写时。
    - 当你对读操作的并发性要求不高，或者读写操作的频率大致相当时。
- **示例**:
    ```rust
    use std::sync::Mutex;
    use std::thread;

    let counter = Mutex::new(0);
    let mut handles = vec![];

    for _ in 0..10 {
        let counter = Arc::new(counter); // 假设这里是 Arc<Mutex<i32>>
        let handle = thread::spawn(move || {
            let mut num = counter.lock().unwrap(); // 获取锁，阻塞直到可用
            *num += 1; // 修改内部数据
            // 锁在 `num` 离开作用域时自动释放
        });
        handles.push(handle);
    }
    // ...
    ```
    在异步代码中，通常使用 `tokio::sync::Mutex` 或 `futures::lock::Mutex`，它们提供 `.lock().await` 方法，在等待锁时不会阻塞线程，而是让出执行权。

---

## 2. `RwLock<T>` (读写锁)

- **作用**: `RwLock` (Read-Write Lock) 读写锁也用于保护共享数据，但它提供了比 `Mutex` 更细粒度的并发控制。
- **特点**:
    - **多读单写**:
        - 允许多个读取者**同时**访问被保护的数据（获取读锁）。
        - 但在有写入者时，会**独占**访问（获取写锁）。当一个写锁被持有，不允许任何其他读锁或写锁存在。
    - **线程安全**: 强制执行访问规则，防止数据竞争。
    - **内部可变性**: 类似于 `Mutex`，通过获取锁来获得对内部 `T` 的访问。
- **使用场景**:
    - 当你的共享数据**读操作远多于写操作**时。`RwLock` 可以显著提高并发读取的性能。
    - 当你需要修改数据时，仍然需要独占访问。
- **示例**:
    ```rust
    use std::sync::RwLock;
    use std::thread;

    let data = RwLock::new(String::from("hello"));
    let mut handles = vec![];

    // 多个线程可以同时读取
    for _ in 0..3 {
        let data = Arc::new(data); // 假设这里是 Arc<RwLock<String>>
        let handle = thread::spawn(move || {
            let s = data.read().unwrap(); // 获取读锁
            println!("Read: {}", *s);
            // 读锁在 `s` 离开作用域时自动释放
        });
        handles.push(handle);
    }

    // 写入时独占
    let data_arc = Arc::new(data); // 假设这里是 Arc<RwLock<String>>
    let handle = thread::spawn(move || {
        let mut s = data_arc.write().unwrap(); // 获取写锁，阻塞直到独占
        s.push_str(" world");
        // 写锁在 `s` 离开作用域时自动释放
    });
    handles.push(handle);
    // ...
    ```
    在异步代码中，通常使用 `tokio::sync::RwLock`，提供 `.read().await` 和 `.write().await`。

---

## 3. `Arc<T>` (原子引用计数)

- **作用**: `Arc` (Atomic Reference Counted) 是一个**智能指针**，用于**共享所有权**。它允许**多个所有者**同时拥有同一份数据，并在所有所有者都消失时自动清理数据。
- **特点**:
    - **共享所有权**: 允许多个 `Arc` 实例指向堆上的同一份数据。
    - **原子操作**: 引用计数器的增减是原子操作，这意味着它是线程安全的，不会导致数据竞争。
    - **不可变共享**: `Arc<T>` 默认提供对内部 `T` 的不可变引用。如果你需要共享**可变**数据，通常需要将 `Arc` 与内部可变性原语（如 `Mutex` 或 `RwLock`）结合使用，例如 `Arc<Mutex<T>>` 或 `Arc<RwLock<T>>`。
    - **自动内存管理**: 当最后一个 `Arc` 实例被丢弃时，它所指向的数据会自动从堆上释放。
- **使用场景**:
    - 当你需要将同一份数据传递给多个线程或异步任务，并且这些任务都需要拥有该数据的“所有权”时。
    - 当你希望数据在所有使用者都完成任务后自动清理时。
    - 它是构建共享可变状态（如 `Arc<Mutex<T>>`）的基础。
- **示例**:
    ```rust
    use std::sync::Arc;
    use std::thread;

    let data = Arc::new(vec![1, 2, 3]); // 创建一个 Arc 拥有 vec![1,2,3]

    for _ in 0..3 {
        let data_clone = Arc::clone(&data); // 克隆 Arc，引用计数增加
        thread::spawn(move || {
            println!("{:?}", *data_clone); // 访问共享数据
        });
    }
    // 当所有 data_clone 离开作用域，引用计数归零，vec![1,2,3] 被释放
    ```

---

## 4. `Atomic` 类型 (原子类型)

- **作用**: `Atomic` 类型（如 `AtomicBool`, `AtomicUsize`, `AtomicU64` 等）用于**在不使用锁的情况下，对单个基本类型进行线程安全的读写操作**。
- **特点**:
    - **无锁操作**: 对原子类型的操作（如 `load`, `store`, `fetch_add`）是原子的，这意味着它们是不可中断的，即使在多线程环境下也能保证操作的完整性，而无需显式地获取和释放锁。
    - **性能高**: 由于避免了锁的开销，原子操作通常比使用 `Mutex` 或 `RwLock` 保护单个值更高效。
    - **仅限基本类型**: 只能用于基本类型（整数、布尔值、指针）。不能用于复杂的数据结构。
    - **内部可变性**: 即使 `Atomic` 类型本身是通过不可变引用 (`&AtomicU64`) 访问的，你仍然可以修改其内部值。
- **使用场景**:
    - 简单的计数器、标志位、状态指示器等。
    - 当你需要对单个、简单的数据类型进行频繁且线程安全的更新时。
- **示例**:
    ```rust
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread;

    let counter = Arc::new(AtomicU64::new(0));

    for _ in 0..10 {
        let counter_clone = Arc::clone(&counter);
        thread::spawn(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst); // 原子地增加计数器
        });
    }
    // ...
    ```

---

## 5. 区别与适用场景总结

| 原语类型       | 共享所有权 | 内部可变性 | 线程安全 | 读写并发性 | 性能开销 | 适用数据类型 | 典型用途                                     |
| :------------- | :--------- | :--------- | :------- | :--------- | :------- | :----------- | :------------------------------------------- |
| `Arc<T>`       | 是         | 否 (默认)  | 是       | N/A        | 低       | 任何 `T`     | 多线程共享不可变数据，或作为 `Mutex`/`RwLock` 的包装 |
| `Mutex<T>`     | 否 (需 `Arc`) | 是         | 是       | 单线程读写 | 中等     | 任何 `T`     | 保护共享可变状态，读写频繁或读写比例接近     |
| `RwLock<T>`    | 否 (需 `Arc`) | 是         | 是       | 多读单写   | 中等     | 任何 `T`     | 保护共享可变状态，读多写少                   |
| `Atomic<T>`    | 否 (需 `Arc`) | 是         | 是       | 无锁读写   | 低       | 基本类型     | 计数器、标志位、状态指示器                   |

---

## 6. 其他常见的并发原语

除了上述核心类型，Rust 标准库和生态系统还提供了其他有用的并发工具：

-   **`std::thread::spawn`**: 创建并运行新线程。
-   **`std::sync::Barrier`**: 允许多个线程在某个点同步，等待所有线程都到达后再继续。
-   **`std::sync::Condvar`**: 条件变量，与 `Mutex` 配合使用，允许线程等待某个条件变为真。
-   **`std::sync::mpsc` (Multi-Producer, Single-Consumer)**: 消息传递通道，用于在线程间发送数据。
-   **`tokio::sync::mpsc` / `flume` / `crossbeam-channel`**: 异步或更高级的通道实现。
-   **`tokio::task::spawn`**: 在异步运行时中创建并运行新的异步任务。
-   **`tokio::sync::Semaphore`**: 信号量，用于控制对有限资源的并发访问数量。
-   **`tokio::sync::Notify`**: 简单的通知机制，用于一个任务通知另一个任务。

---

理解这些原语并选择正确的工具，是编写高效、健壮的 Rust 并发应用程序的关键。通常，你会发现 `Arc` 经常与 `Mutex` 或 `RwLock` 结合使用，以实现共享的可变状态。
