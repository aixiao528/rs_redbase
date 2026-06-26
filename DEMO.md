# rs-redbase Demo Guide

## 运行命令

在 `D:\MySQL_learn\exp\lab6\rs-redbase` 下执行：

```bash
cargo run --example demo
```

## 建议讲解顺序

### 1. 打开数据库并建表

demo 会先创建一个固定的 `student` 表：

- `id: Int32`
- `score: Float32`
- `name: Char(8)`

这一段主要展示：

- `QueryEngine::open`
- `execute_create_table`
- `show_tables`
- `describe_table`

对应模块：

- `table/catalog/metadata`
- `schema request layer`

### 2. 插入数据并创建索引

demo 会插入 4 条固定学生记录，并创建 `student_id_idx`。

这一段主要展示：

- `insert_into`
- `execute_create_index`
- `show_indexes`

对应模块：

- `record manager`
- `query insert`
- `minimal index`

### 3. 展示查询能力

demo 会展示两类查询：

- `id = 2` 的等值过滤
- `score DESC LIMIT 2` 的排序与限制

这一段主要展示：

- 最小索引 fast path
- `projection`
- `ORDER BY`
- `LIMIT`

对应模块：

- `typed predicate`
- `query select`
- `minimal index fast path`

### 4. 展示更新与删除

demo 会：

- 更新 `id = 2` 的 `score`
- 删除 `id = 1`
- 再次查询并打印剩余结果

这一段主要展示：

- `execute_update`
- `execute_delete`
- 写后索引重建保持一致性

对应模块：

- `minimal DML`
- `table write path`
- `index maintenance`

### 5. 清理演示数据

demo 最后会：

- 删除索引
- 删除表
- 删除 `demo-db` 目录

这一段主要展示：

- `execute_drop_index`
- `execute_drop_table`
- 演示流程可重复运行

## 讲解建议

如果是课程汇报或答辩，可以按下面一句话总述：

> 这个项目已经从底层页管理、缓冲池、记录管理，一路扩展到了表管理、查询执行、最小 DML、模式管理和最小索引层；`demo` 展示的是这些能力在统一入口上的完整闭环。

建议重点强调三点：

1. 不是只做了存储结构，而是形成了统一 `QueryEngine` 请求入口
2. 不只有 CRUD，还有 projection、排序、限制和最小索引访问路径
3. 当前实现强调教学式分层与 Rust 安全边界，先保证语义正确，再逐步演进性能

## 建议校验

在正式展示前建议先运行：

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt -- --check
```
