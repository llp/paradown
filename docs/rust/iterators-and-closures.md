# Rust 语法：迭代器、闭包、链式调用

代表代码：

- `src/domain/source.rs`
- `src/storage/mapping.rs`
- `src/scheduler/planner.rs`
- `src/payload/file_map.rs`

## 1. 迭代器是惰性的

```rust
items.iter().map(|item| ...)
```

这行本身不会立刻生成新集合。

只有遇到消费方法时才真正执行，例如：

- `.collect()`
- `.find(...)`
- `.any(...)`
- `.count()`
- `.fold(...)`

## 2. `iter`

```rust
pieces.iter()
```

借用遍历，元素类型通常是 `&T`。

原集合仍然可用。

## 3. `iter_mut`

```rust
self.sources.iter_mut()
```

可变借用遍历，元素类型通常是 `&mut T`。

可以修改集合中的元素。

## 4. `into_iter`

```rust
pieces.into_iter()
```

消费集合，元素类型通常是 `T`。

调用后原集合不再可用。

适合已经拥有集合并想把元素移动到新结构中的场景。

## 5. 闭包

```rust
|piece| PieceState {
    piece_index: piece.piece_index,
    completed: piece.completed,
}
```

`|piece| ...` 是闭包，也就是匿名函数。

闭包可以捕获外部变量：

```rust
.map(|piece| DBDownloadPiece {
    task_id,
    piece_index: piece.piece_index,
})
```

这里闭包捕获了外部的 `task_id`。

## 6. `map`

`map` 把每个元素转换成另一个值。

```rust
pieces.iter().map(|piece| DBDownloadPiece { ... })
```

输入是 `PieceState`，输出是 `DBDownloadPiece`。

## 7. `filter`

`filter` 保留满足条件的元素。

```rust
.filter(|source| source.can_transfer_payload())
```

闭包返回 `bool`。

## 8. `find`

`find` 返回第一个满足条件的元素。

```rust
.find(|source| source.id == source_id)
```

返回 `Option<&T>` 或 `Option<&mut T>`。

## 9. `any`

`any` 判断是否至少有一个元素满足条件。

```rust
.any(|existing| existing.id == source.id)
```

返回 `bool`。

## 10. `collect`

`collect` 把迭代器收集成集合。

```rust
.collect::<Vec<_>>()
```

很多时候 Rust 可以从函数返回类型推断出目标集合，所以项目里常写：

```rust
.collect()
```

## 11. `sort_by_key`

```rust
pieces.sort_by_key(|piece| piece.piece_index);
```

`sort_by_key` 会根据闭包返回的 key 排序。

它需要可变集合，所以前面通常会看到：

```rust
let mut pieces = pieces.to_vec();
```
