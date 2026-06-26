# rs-redbase

`rs-redbase` 是一个基于 Rust 实现的轻量级数据库内核教学原型，inspired by the RedBase course project。  
它采用自底向上的 layered design，从 `page` / `buffer` / `record` 一路扩展到 `table`、`query` 和 minimal `index` layer，重点强调：

- clear module boundaries
- Rust memory safety
- step-by-step evolution
- course-demo friendly architecture

本项目定位是 **educational prototype / 课程项目原型**，而不是 production-ready database。

## Overview | 项目概览

当前项目已经实现：

- page management
- buffer pool with LRU replacement
- fixed-length record management
- record scan with typed / compound / logic filters
- table schema and catalog management
- metadata access
- unified `QueryEngine` request layer
- minimal DML: `insert / select / update / delete`
- projection, `ORDER BY`, `LIMIT`
- minimal single-column index for `Int32 = constant`
- runnable demo entry for presentation

换句话说，这个仓库已经不是单纯的底层存储模块集合，而是一个具备 **schema + query + CRUD + minimal index** 的完整教学型数据库原型。

## Architecture | 分层结构

系统整体结构如下：

```text
storage::page
    -> storage::buffer
        -> storage::record
            -> table / catalog / metadata
                -> query::QueryEngine
                    -> demo / presentation entry
```

### Main modules | 主要模块

- `src/storage/page`
  - disk page allocation / reuse / read / write
  - 磁盘页分配、回收和读写
- `src/storage/buffer`
  - buffer pool, page cache, LRU eviction, dirty-page flush
  - 缓冲池、页面缓存、LRU 替换和脏页回写
- `src/storage/record`
  - fixed-length record layout and record scan
  - 固定长度记录组织、插入删除更新与扫描
- `src/storage/index`
  - minimal index catalog and equality lookup path
  - 最小索引目录与等值查询 fast path
- `src/table`
  - table schema, row encode/decode, catalog, metadata
  - 表结构、行编解码、catalog 与元数据管理
- `src/query`
  - unified high-level requests through `QueryEngine`
  - 统一数据库请求入口
- `examples/demo.rs`
  - end-to-end demo for presentation
  - 用于课程展示的统一 demo 脚本

## Features | 功能特性

### Storage Layer | 存储层

- fixed-size page file abstraction
- buffer pool with `Arc<RwLock<_>>`
- dirty page flushing
- free-slot reuse for fixed-length records

### Query Layer | 查询层

- create / show / describe / drop table
- insert / select / update / delete
- named insert requests
- projection by selected columns
- `ORDER BY`
- `LIMIT`

### Filter Support | 过滤能力

- typed constant comparison
- field-to-field comparison
- multi-clause `AND`
- logic tree: `And / Or / Not`

### Index Layer | 索引层

- create / drop / show index
- minimal persistent index catalog
- equality fast path for indexed `Int32` columns
- write-path consistency maintained by index rebuild after writes

## Demo | 演示入口

仓库内置了一个可直接运行的 demo：

```bash
cargo run --example demo
```

demo 会按固定顺序展示完整能力链：

1. open database
2. create table
3. show / describe table
4. insert rows
5. create index
6. run equality query
7. run ordered query with limit
8. update row
9. delete row
10. cleanup

See [DEMO.md](./DEMO.md) for the presentation-oriented walkthrough.  
更适合答辩或录屏展示的讲解顺序，也在 `DEMO.md` 中给出。

## Build And Test | 构建与测试

在项目根目录执行：

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt -- --check
```

## Repository Structure | 仓库结构

```text
rs-redbase/
├─ src/
│  ├─ storage/
│  │  ├─ page/
│  │  ├─ buffer/
│  │  ├─ record/
│  │  └─ index/
│  ├─ table/
│  ├─ query/
│  └─ lib.rs
├─ tests/
├─ examples/
│  └─ demo.rs
├─ DEMO.md
├─ README.md
└─ 项目报告-Word版.md
```

## Current Scope | 当前范围

目前已经完成的核心内容：

- storage engine basics
- catalog and metadata
- CRUD
- ordering and limiting
- minimal index access path
- demo and report materials

当前尚未实现：

- SQL parser
- B+ tree indexes
- transactions and recovery
- optimizer / planner
- advanced concurrency control

因此，这个项目更适合作为 **database kernel course project / 数据库内核课程项目成果**，而不是完整数据库产品。

## Why Rust | 为什么使用 Rust

本项目使用 Rust 的主要原因是，希望在实现数据库内核这类 systems software 时，更好地利用：

- ownership
- borrowing
- structured error handling
- safe shared-state primitives

相比传统 pointer-heavy implementation，Rust 能让 resource lifetime、module boundary 和 error propagation 更加显式，也更适合教学场景下展示安全边界。

## Notes | 说明

- This is an educational project.
- 当前实现优先考虑 correctness、structure 和 explainability。
- 某些能力故意采用 minimal first-step design，以保证架构清晰、便于展示和逐层扩展。
