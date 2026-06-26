# rs-redbase Demo Guide

## 运行命令

在 `D：rs-redbase` 下执行：

```bash
cargo run --example demo
```

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
