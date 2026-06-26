# Rust 课程期末项目报告

## 项目名称

`rs-redbase`：基于 Rust 的轻量级数据库内核教学原型

## 一、项目简介

本项目实现了一个基于 Rust 的轻量级数据库内核教学原型，项目名称为 `rs-redbase`。项目参考了 RedBase 的课程设计思路，目标是通过分层设计，逐步实现数据库系统中最核心的基础模块，包括页管理、缓冲池、记录管理、表与目录管理、查询执行层以及最小索引层。项目最终形成了一个具备建表、删表、查询、更新、删除和索引访问能力的数据库原型系统。<mccoremem id="03gdmi8ympocotb2p97w5o71b|03gdfpi3j01b2xog3ohrs5dar|03gdfu0t8fvvd7rwyesdn1wx1" />

本项目的实现重点不在于追求完整商用数据库的复杂功能，而在于通过清晰的模块边界和逐层抽象，完整展示数据库内核从底层存储到上层请求接口的演化过程。与传统 C++ 课程项目不同，本项目使用 Rust 实现，重点利用了 Rust 的所有权、借用检查、枚举建模和错误传播机制，以提升系统软件开发中的安全性与可维护性。

本项目已经具备以下主要能力：

- 固定大小磁盘页管理
- 基于 LRU 的缓冲池
- 固定长度记录管理与记录扫描
- typed predicate、复合过滤和逻辑过滤树
- 表结构与目录管理
- `create/show/describe/drop table`
- `insert/select/update/delete`
- projection、`ORDER BY`、`LIMIT`
- 单列 `Int32` 等值最小索引
- 统一 demo 演示入口。<mccoremem id="01KVZPDR39N8W2GJYY1RBS0PJD|03gdnug3mk35x9pujgjvpu620|01KVZXVKT49D8EGGAYYVFBT1GR|03gdu5gsn4p3w6oq24prsyxz1" />

## 二、小组成员分工（单人不需要）

本项目为个人独立完成，无小组成员分工。

## 三、项目结构

本项目采用自底向上的分层结构设计，主要由以下几个模块组成：

### 1. `storage::page`

负责数据库文件的最底层磁盘页管理，实现页分配、页释放、页读写以及文件重开后的页访问。

### 2. `storage::buffer`

负责缓冲池管理，将磁盘页缓存到内存中，并通过 LRU 策略进行页面替换。该模块还负责脏页回写、pin/unpin 控制，以及共享页访问时的安全封装。<mccoremem id="03gdfpi3j01b2xog3ohrs5dar" />

### 3. `storage::record`

负责固定长度记录的管理，包括记录插入、读取、更新、删除以及页内槽位复用。同时实现了记录扫描接口和多种过滤表达能力。<mccoremem id="03gdfu0t8fvvd7rwyesdn1wx1|01KVYVB6149HWGVEAK2CD8ZYAM" />

### 4. `storage::index`

负责最小索引目录与索引文件管理，实现单列 `Int32` 等值索引以及最小 fast path。<mccoremem id="01KVZXVKT49D8EGGAYYVFBT1GR" />

### 5. `table`

负责表结构、行编解码、catalog 管理以及 relation / attribute metadata 的维护，使底层记录具备数据库“表”的语义。<mccoremem id="03gdmi8ympocotb2p97w5o71b|03gave0liyp1g7z2qg2o45yje" />

### 6. `query`

负责统一的上层请求入口 `QueryEngine`，将 schema 请求、DML 请求、排序、限制和索引访问组织成结构化 API。<mccoremem id="01KVZPDR39N8W2GJYY1RBS0PJD|03gdnug3mk35x9pujgjvpu620" />

### 7. `examples/demo.rs`

负责项目最终演示入口，串联建表、插入、建索引、查询、更新、删除和清理等完整演示流程。<mccoremem id="03gdu5gsn4p3w6oq24prsyxz1" />

## 四、设计与实现

### 1. 整体设计思路

本项目采用分层实现方式，从最底层磁盘页管理开始，逐步向上构建数据库系统能力。每一层只负责自己明确的职责，并通过稳定接口向上提供服务，从而避免不同模块之间逻辑耦合过深。

整体设计路径为：

`page -> buffer -> record -> filter -> table/catalog -> query -> index -> demo`

这种设计方式的优点在于：

- 模块边界清晰，便于调试与测试
- 底层能力可以稳定支撑上层语义
- 每一层都可以单独验证，不必一次性实现全部系统

### 2. Rust 特性在项目中的体现

本项目重点体现了 Rust 在系统软件开发中的若干优势：

- 使用 `struct` 和 `enum` 对页、记录、请求对象、值类型和错误类型进行显式建模
- 使用 `Result<T, E>` 和 `?` 实现统一错误传播，避免大量不安全的 `unwrap`
- 使用所有权和借用模型控制资源访问，避免悬空指针和重复释放
- 在缓冲池中使用 `Arc<RwLock<_>>` 包装共享页，保证并发访问场景下的安全性
- 通过模块化组织实现代码分层，增强可维护性。<mccoremem id="03gdfpi3j01b2xog3ohrs5dar" />

### 3. 上层请求模型设计

在项目后期，我没有继续停留在“底层模块功能集合”的层面，而是实现了统一的 `QueryEngine`。该层支持：

- `create/show/describe/drop table`
- `insert/select/update/delete`
- projection
- `ORDER BY`
- `LIMIT`
- `create/drop/show index`

通过这一层，系统形成了统一的数据库请求入口，使整个项目从“模块实现”进一步发展为“可操作的数据库原型系统”。<mccoremem id="01KVZGK7GCBXHG2YSXWPJ9QYN4|01KVZPDR39N8W2GJYY1RBS0PJD|03gdnug3mk35x9pujgjvpu620" />

## 五、各模块详细说明

### 1. 页管理模块

页管理模块是数据库的最底层存储模块。它的主要职责是以固定大小页为基本单位，管理数据库文件中的空间分配与数据读写。该模块支持：

- 新页分配
- 页回收
- 页读写
- 文件重开后的页访问

这一层为后续缓冲池和记录管理模块提供了稳定的物理存储基础。

### 2. 缓冲池模块

缓冲池模块负责将磁盘页缓存到内存中，减少频繁磁盘 I/O。该模块支持：

- 页注册与文件级管理
- 基于 `(FileId, PageId)` 的页表管理
- LRU 页面替换
- pin 计数控制
- 脏页回写

在设计时，缓冲页数据采用 `Arc<RwLock<_>>` 封装，使共享访问和独占写访问在接口层就得到控制。<mccoremem id="03gdfpi3j01b2xog3ohrs5dar" />

### 3. 记录管理模块

记录管理模块建立在页管理和缓冲池之上，负责固定长度记录的组织与访问。该模块的主要特点包括：

- 使用页头与位图管理槽位占用状态
- 使用空闲页链复用空槽
- 支持插入、读取、更新、删除
- 支持页序、槽序扫描

此外，该模块还扩展了记录扫描过滤能力，包括：

- typed predicate
- 字段对字段复合过滤
- 逻辑条件树 `And / Or / Not`。<mccoremem id="01KVYX61MVP9Q8AESB782ME2K1|01KVYYCMX23KTFKQSVM615NX28|03gdm3ng43f7u09l9pg4afr2e" />

### 4. 表与目录管理模块

为了让底层记录具备更接近数据库的语义，本项目在记录层之上实现了表与目录管理模块。该模块引入了：

- `TableSchema`
- `ColumnSchema`
- `Value`
- `Row`
- `CatalogManager`
- `RelationMeta`
- `AttributeMeta`

通过这些结构，系统可以知道：

- 当前有哪些表
- 每张表有多少列
- 每列的名称、类型、偏移和长度

从而形成最基本的 schema 管理能力。<mccoremem id="03gdmi8ympocotb2p97w5o71b|03gave0liyp1g7z2qg2o45yje" />

### 5. 查询执行模块

查询执行模块使用 `QueryEngine` 作为统一入口。该模块负责：

- 请求结构化表示
- 语义检查
- 调用表层执行具体动作
- 组织最终返回结果

当前已支持的功能包括：

- 建表、看表、描述表、删表
- 插入、查询、更新、删除
- 显式投影
- 排序
- 限制返回条数

这使得项目不只是底层存储模块的堆叠，而是形成了一个具备数据库语义的可操作原型。<mccoremem id="01KVZGK7GCBXHG2YSXWPJ9QYN4|03gdnug3mk35x9pujgjvpu620|01KVZPDR39N8W2GJYY1RBS0PJD" />

### 6. 索引模块

索引模块是本项目面向“更像数据库内核”的一次关键扩展。当前索引层支持：

- 单列索引
- `Int32` 键
- 等值查询 `Eq`
- 索引目录持久化
- 查询 fast path

当查询条件满足“索引列 = `Int32` 常量”时，系统会优先通过索引文件获取候选 `Rid`，再读取记录，而不是始终使用全表扫描。

当前版本为了优先保证正确性，在插入、更新和删除后采用“重建相关索引”的方式维护一致性。这一方案虽然不是最终性能最优方案，但实现稳定、逻辑清晰，适合作为课程项目的最小索引实现。<mccoremem id="01KVZXVKT49D8EGGAYYVFBT1GR" />

## 六、运行截图

本项目建议在报告中插入以下运行截图：

### 图 1：`cargo test` 运行结果

展示项目测试全部通过，说明系统各模块功能已经较为完整。

### 图 2：`cargo clippy --all-targets --all-features -- -D warnings`

展示工程规范与静态检查通过。

### 图 3：`cargo run --example demo` 前半段

建议截取以下内容：

- `Open Database`
- `Create Table`
- `Show Tables`
- `Describe Table`

这一部分主要展示 schema 管理能力。

### 图 4：`cargo run --example demo` 后半段

建议截取以下内容：

- `Create Index`
- `Index Equality Query`
- `Order By + Limit`
- `Update Row`
- `Delete Row`
- `Cleanup`

这一部分主要展示查询执行、最小索引与写路径能力。<mccoremem id="03gdu5gsn4p3w6oq24prsyxz1" />

## 七、遇到的问题与解决方法

### 1. 模块边界容易混乱

数据库项目天然涉及页、缓存、记录、表和查询多个层次，如果边界不清晰，后续修改会非常困难。为了解决这一问题，我采用严格的分层推进方式，每一轮只扩展一层能力，并先稳定接口，再扩展上层语义。

### 2. 写路径与查询路径的统一比较复杂

随着 `insert/select/update/delete` 的逐步完善，请求层容易出现逻辑分散问题。为了解决这一问题，我将各类数据库动作统一收口到 `QueryEngine`，用结构化请求对象描述操作，再由内部统一完成语义检查和调用调度。<mccoremem id="03gdnug3mk35x9pujgjvpu620|01KVZPDR39N8W2GJYY1RBS0PJD" />

### 3. 索引维护策略的选择

如果直接实现复杂的增量索引维护，当前项目复杂度会显著升高，也更容易出现一致性错误。因此本项目先采用“写后重建相关索引”的方式，以保证语义正确性和可演示性。<mccoremem id="01KVZXVKT49D8EGGAYYVFBT1GR" />

### 4. 系统安全性问题

传统数据库课程项目通常使用 C++ 实现，容易在页缓存、记录指针和资源释放方面出现悬空指针、重复释放或共享访问不安全的问题。本项目使用 Rust，通过所有权、借用和 `Result` 机制，在编译期减少这一类错误。<mccoremem id="03gdfpi3j01b2xog3ohrs5dar" />

### 5. 演示组织难度较高

随着功能不断增多，仅通过测试文件展示项目能力不够直观。为解决这一问题，本项目新增统一 demo 入口，并编写了 `DEMO.md` 说明文档，使整个系统能力能够以固定顺序稳定展示。<mccoremem id="03gdu5gsn4p3w6oq24prsyxz1" />

## 八、AI 使用说明

本项目在开发和整理过程中使用了 AI 辅助工具，主要用途如下：

### 1. 设计讨论与实现规划

AI 主要用于协助梳理数据库各层模块的职责边界，帮助形成每一轮开发的 spec、任务拆分和自检清单，例如页管理、缓冲池、记录管理、查询执行层、索引层和演示收口等阶段。

### 2. 代码实现辅助

AI 在项目中承担了代码草案生成、接口设计建议、错误处理结构梳理以及测试用例补充等辅助工作。具体来说，AI 帮助我：

- 设计结构体、枚举和错误类型
- 组织模块接口
- 补充单元测试和集成测试
- 检查实现中的逻辑遗漏

### 3. 文档与展示材料整理

AI 同时协助整理项目讲稿、项目报告、演示说明文档以及成果展示链路，使项目内容更适合课程汇报和答辩。

### 4. 人工主导与校验方式

虽然项目使用了 AI 辅助，但整体开发过程仍由本人主导，包括：

- 确定实现范围与每轮目标
- 审查代码设计是否符合课程要求
- 本地运行 `cargo test`、`cargo clippy`、`cargo fmt`
- 对输出内容和最终结果进行人工确认

因此，AI 在本项目中的定位是“开发辅助与文档整理工具”，而不是完全替代人工实现。所有最终代码和报告内容都经过了人工检查与确认。

## 九、其他需要说明的内容（如果有）

本项目定位为“数据库内核教学原型”，重点在于：

- 分层设计完整
- 核心路径可运行
- 查询与 schema 管理闭环明确
- 具备可测试、可展示、可扩展能力

当前尚未实现的内容主要包括：

- SQL parser
- 更复杂的索引结构，如 B+Tree
- 事务管理与恢复机制
- 查询优化器
- 更复杂的并发控制

这些内容可以作为后续扩展方向，但不影响本项目作为课程期末数据库内核原型的完整性。

## 十、总结

本项目基于 Rust 实现了一个轻量级数据库内核教学原型，从最底层的页管理开始，逐步实现了缓冲池、记录管理、表与目录管理、统一查询执行层以及最小索引层，最终形成了一个具备完整演示能力的数据库原型系统。<mccoremem id="03gdmi8ympocotb2p97w5o71b|03gdnug3mk35x9pujgjvpu620|01KVZXVKT49D8EGGAYYVFBT1GR|03gdu5gsn4p3w6oq24prsyxz1" />

通过本项目，我不仅实现了数据库内核若干关键模块，也更深入地理解了不同层次模块之间的职责划分，以及 Rust 在系统软件开发中对于安全性和可维护性的帮助。整体来看，本项目已经达到了“结构完整、功能闭环、可运行、可测试、可展示”的课程项目目标。
