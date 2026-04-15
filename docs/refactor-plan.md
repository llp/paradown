# paradown 重构计划

更新时间：2026-04-15

仓库路径：`/Users/liulipeng/workspace/rust/paradown`

本文档用于记录当前已经完成的重构、尚未完成的重构，以及接下来的重构路线。目标不是“为了拆而拆”，而是逐步把项目从原型式结构收敛成更稳定、职责更清晰、可继续演进的下载器架构。

## 1. 重构目标

本轮和后续几轮重构的目标主要有四个：

- 收紧命名，让模块和类型更符合领域语义
- 降低 `main.rs`、`manager.rs`、`task.rs` 的复杂度
- 把运行时、调度、准备、完成、存储等横切职责拆开
- 为后续修复下载正确性、恢复正确性和测试体系打基础

当前重构优先级是：

1. 先收架构边界
2. 再修协议与恢复正确性
3. 再补测试与可观测性
4. 最后收用户体验与高级特性

## 2. 当前架构目标图

期望中的架构分层如下：

- `main.rs`
  - CLI 入口
  - 参数解析
  - 调用库层公开 API

- `download::*`
  - `Manager`：协调器
  - `Task`：单个下载作业
  - `Worker`：单作业内的分段下载执行单元

- `coordinator/`
  - 事件收敛
  - 队列与 permit 管理
  - 作业注册与恢复

- `job/`
  - 下载前准备
  - 下载完成收敛

- `storage::*`
  - 持久化入口
  - repository 抽象
  - sqlite / memory 实现

- `request/`
  - `TaskRequest`
  - `SegmentRequest`

- `chunk.rs`
  - 分块规划

- `runtime.rs`
  - logger 初始化
  - HTTP client 构建

## 3. 已完成的重构

### 3.1 第一轮：命名和运行时边界收拢

提交：

- `401e531` `重构下载架构命名并收拢运行时边界`

已完成内容：

- 新增 `download` 命名空间：[download.rs](/Users/liulipeng/workspace/rust/paradown/src/download.rs:1)
- 新增 `storage` 命名空间：[storage.rs](/Users/liulipeng/workspace/rust/paradown/src/storage/mod.rs:1)
- 为核心类型补充领域命名别名：
  - `Manager`
  - `Task`
  - `Worker`
  - `Store`
- 新增运行时模块：[runtime.rs](/Users/liulipeng/workspace/rust/paradown/src/runtime.rs:1)
  - 统一 logger 初始化
  - 统一 HTTP client 构建
- 新增分块规划模块：[chunk.rs](/Users/liulipeng/workspace/rust/paradown/src/chunk.rs:1)
  - 从 `task.rs` 中抽离 chunk 计算
  - 修复小文件/大 worker 数导致的非法分块结构问题
- `main.rs` 改为直接依赖库 crate，而不是重复声明整套模块
- 去掉 `main.rs` 和 `manager.rs` 的重复 logger 初始化问题

这一轮解决的主要问题：

- 命名过于泛化
- 运行时能力分散
- 入口层耦合过深
- 分块规划和作业编排混杂

### 3.2 第二轮：拆分 Task 的准备与完成流程

提交：

- `962e2fc` `拆分下载作业的准备与完成流程`

已完成内容：

- 新增下载前准备模块：[job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job/prepare.rs:1)
  - 下载目录准备
  - 文件名/文件路径准备
  - `HEAD + Content-Length` 探测
  - 已存在文件策略处理
  - 零字节文件处理
- 新增下载完成收敛模块：[job_finalize.rs](/Users/liulipeng/workspace/rust/paradown/src/job/finalize.rs:1)
  - checksum 校验
  - 完成/失败状态收敛
  - 完成/失败事件发送
- `task.rs` 不再把准备阶段和完成阶段全部揉在 `start()` 和 worker 完成事件里
- 抽出 `resolve_or_init_file_name()` 等辅助能力，减小 `task.rs` 的局部复杂度

这一轮解决的主要问题：

- `task.rs` 既做准备又做完成收敛，方法过长
- checksum 收敛逻辑与下载执行逻辑混杂
- 文件准备逻辑难以单独替换或测试

### 3.3 第三轮：拆分 Manager 的事件、队列和注册逻辑

提交：

- `1ee3b01` `拆分下载协调器的事件队列与注册逻辑`

已完成内容：

- 新增协调器事件模块：[coordinator_events.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/events.rs:1)
  - manager 事件循环迁出
  - terminal 事件统一收敛
- 新增协调器队列模块：[coordinator_queue.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/queue.rs:1)
  - permit 获取/释放
  - 排队
  - 触发下一个任务
- 新增协调器注册模块：[coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/registry.rs:1)
  - 从持久化恢复任务
  - 注册新任务
  - worker 恢复装配
- `manager.rs` 明显瘦身，更多保留公开 API 和薄协调逻辑
- `start_task / resume_task` 的骨架已开始统一收敛

这一轮解决的主要问题：

- `manager.rs` 同时承担事件消费、排队调度、恢复组装、注册逻辑
- 协调器壳层太厚，不利于后续继续演进

### 3.4 第四轮：拆分 Task 的 worker、state 与 storage 逻辑

提交：

- `0a1d8ef` `拆分下载作业的 worker 状态与存储逻辑`

已完成内容：

- 新增 worker 协调模块：[job_workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job/workers.rs:1)
  - worker 事件监听
  - worker 创建与装配
  - worker 启动与 join 收敛
  - worker 级进度/完成/错误/取消事件处理
- 新增作业状态模块：[job_state.rs](/Users/liulipeng/workspace/rust/paradown/src/job/state.rs:1)
  - `start` 前状态判断
  - pause/resume/cancel/delete 状态流转
  - reset/delete file/clear workers 等状态辅助逻辑
- 新增作业存储模块：[job_storage.rs](/Users/liulipeng/workspace/rust/paradown/src/job/storage.rs:1)
  - task/checksum/worker 持久化
  - task/workers/checksums 清理
- [task.rs](/Users/liulipeng/workspace/rust/paradown/src/job/mod.rs:1) 收敛为作业门面
  - 保留结构定义、`new`、`snapshot`、`init`、`start`
  - 保留文件名/文件路径辅助方法
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1) 接入新的内部模块
- `task.rs` 体量从重构前的 `884` 行下降到本轮后的 `247` 行

这一轮解决的主要问题：

- `task.rs` 同时承担 worker 生命周期、状态机、持久化辅助和文件清理
- `Task` 作为门面类型仍然过重，不利于继续演进
- 后续如果继续修 worker 协调或状态流转，需要频繁修改超大文件

### 3.5 第五轮：引入协议探测层并收紧 Range 下载正确性

提交：

- `a4b4f96` `引入协议探测并收紧 Range 下载正确性`

已完成内容：

- 新增协议探测模块：[protocol_probe.rs](/Users/liulipeng/workspace/rust/paradown/src/protocol_probe.rs:1)
  - `HEAD` 探测
  - `Range: bytes=0-0` 探测
  - `Content-Range` 解析
  - Range 支持能力判断
  - 探测单元测试
- [job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job/prepare.rs:1) 接入协议探测结果
  - 不再只依赖 `HEAD + Content-Length`
  - 显式记录目标资源是否支持 range
  - 对不支持 range 的部分文件恢复改为安全回退到整文件重下
  - 文件缺失但持久化仍有进度时，清理过期 worker/progress 状态
- [task.rs](/Users/liulipeng/workspace/rust/paradown/src/job/mod.rs:1) 增加协议探测状态
  - `range_requests_supported`
  - `protocol_probe_completed`
- [job_state.rs](/Users/liulipeng/workspace/rust/paradown/src/job/state.rs:1) 调整恢复语义
  - 恢复到当前进程后，如果还没有重新探测协议能力，暂停任务会重新走准备流程
- [job_workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job/workers.rs:1) 根据协议能力决定 worker 规划
  - 不支持 range 时只允许单 worker
  - 不兼容的历史 worker 布局会被重建
- [worker.rs](/Users/liulipeng/workspace/rust/paradown/src/worker/mod.rs:1) 收紧响应校验
  - 支持 range 时显式要求 `206 Partial Content`
  - 显式校验 `Content-Range`
  - 不支持 range 时显式要求 `200 OK`
  - worker 恢复时使用持久化的已下载偏移，而不是总从 0 开始

这一轮解决的主要问题：

- 下载前探测和 worker 执行之间没有共享“协议事实”
- 服务端不支持 range 时仍可能按多段下载路径执行
- worker 恢复时没有使用已持久化的下载偏移
- `HEAD` 不可用时没有合适的探测回退路径

### 3.6 第六轮：修正存储模型一致性并补恢复层测试

提交：

- `7df40ee` `修正存储模型一致性并补仓储层测试`

已完成内容：

- [sqlite_repository.rs](/Users/liulipeng/workspace/rust/paradown/src/repository/sqlite.rs:1) 修正了 task 持久化主键语义
  - `download_tasks` 改为按 `id` upsert，而不是按 `url` upsert
  - 保存 task 时显式写入 `id`
  - 避免 task id 与 URL upsert 语义漂移
- [sqlite_repository.rs](/Users/liulipeng/workspace/rust/paradown/src/repository/sqlite.rs:1) 修正了时间字段 round-trip
  - 新写入的时间戳统一为 RFC3339 风格文本
  - 读取时兼容 RFC3339 和 SQLite `CURRENT_TIMESTAMP` 旧格式
- [memory_repository.rs](/Users/liulipeng/workspace/rust/paradown/src/repository/memory.rs:1) 修正了 worker/checksum 的 key 冲突
  - worker 改为按 `(task_id, index)` 存储
  - checksum 改为按 `(task_id, algorithm)` 存储
  - 避免多任务下相同 worker index 或 checksum id 相互覆盖
- 为 repository 层补了针对性测试
  - memory backend 的 worker/checksum 隔离测试
  - sqlite backend 的 task id/timestamp round-trip 测试

这一轮解决的主要问题：

- SQLite task upsert 以 URL 为主键，导致 task id 语义不稳定
- SQLite 时间字段格式前后不一致，读取时可能丢失时间信息
- Memory backend 在多任务场景下会出现 worker/checksum 覆盖
- 持久化模型缺少最基本的回归测试

### 3.7 第七轮：下沉恢复转换并收紧恢复信任边界

提交：

- `0c81e96` `下沉恢复转换并收紧恢复信任边界`

已完成内容：

- [persistence.rs](/Users/liulipeng/workspace/rust/paradown/src/storage/mod.rs:1) 新增了更明确的 storage API
  - 新增 `StoredBundle`
  - 新增 `load_task_bundles()`
  - 新增 DB 模型到 `TaskRequest` / `SegmentRequest` 的转换方法
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/registry.rs:1) 不再直接拼装底层 DB 模型
  - 恢复逻辑改为消费 `persistence.rs` 提供的 bundle 和转换结果
  - 恢复状态判断与存储读取职责开始分离
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/registry.rs:1) 收紧了恢复信任边界
  - `Running/Preparing` 统一恢复为 `Paused`
  - 本地文件缺失时，不再信任已持久化的 worker/progress
  - worker 布局必须满足 task_id 正确、index 唯一、range 有效、下载偏移合法、分块连续覆盖
  - 不可信的 worker 布局会整组丢弃，并把任务回退到 `Pending`
- 为恢复路径补了针对性测试
  - 缺文件时降级为 `Pending`
  - 合法 paused 恢复时保留 worker，并按 worker 重新计算下载进度
  - 非法 worker 布局会被整组丢弃

这一轮解决的主要问题：

- `coordinator_registry.rs` 之前既读 DB 模型又做恢复策略，职责混杂
- 恢复路径对持久化 worker 数据过于信任
- 文件缺失、分块重叠、分块不完整等场景缺少明确的降级规则

### 3.8 第八轮：拆分 Worker 的运行时、传输与重试逻辑

提交：

- `084c852` `拆分下载 worker 的运行时与传输重试逻辑`

已完成内容：

- 新增 worker 运行时模块：`src/worker/runtime.rs`
  - 把 `start()` 主流程从 `worker.rs` 迁出
  - 收敛 worker 启动、成功、失败、重试主循环
  - 把 `is_running` 的收尾从多处分散设置改为统一出口
- 新增 worker 传输模块：`src/worker/transfer.rs`
  - 抽离请求构建
  - 抽离响应协议校验
  - 抽离文件打开、seek、写入和进度节流
  - 把 worker 进度上报封装成独立 helper
- 新增 worker 重试模块：`src/worker/retry.rs`
  - 让 `initial_delay / max_delay / backoff_factor` 真正参与运行时
  - 为 retry delay 增加独立单测
- [worker.rs](/Users/liulipeng/workspace/rust/paradown/src/worker/mod.rs:1) 收敛为 worker 门面
  - 保留结构体、构造器、暂停/恢复/取消/删除控制逻辑
  - 新增通用的状态设置、事件发送、停止判断 helper
- 修正了 `resume()` 的锁顺序和状态流转
  - 避免 `resume()` 在持有 status lock 时再次调用 `start()` 导致死锁
  - 避免 paused worker 被 resume 后又被 `start()` 重新打回暂停
- 补上了 worker 级回归测试
  - 验证 paused worker 能恢复且不会死锁

这一轮解决的主要问题：

- `worker.rs` 同时混着生命周期、重试、协议校验、文件写入和进度上报
- retry 配置之前只接上了 `max_retries`
- worker 失败路径和 `is_running` 收尾逻辑分散，后续很难继续收紧
- `resume()` 的锁顺序和状态切换存在明显缺陷

### 3.9 第九轮：拆分 recovery 规则与 storage 模型转换层

提交：

- `8909c0a` `拆分恢复规则与存储映射层`

已完成内容：

- 新增恢复模块：[recovery.rs](/Users/liulipeng/workspace/rust/paradown/src/recovery.rs:1)
  - 把恢复计划、状态降级、worker 布局校验从 `coordinator_registry.rs` 迁出
  - 让恢复层直接消费统一的 storage bundle，而不是耦合协调器实现
  - 为 completed 文件尺寸不匹配场景补了针对性测试
- 新增存储映射模块：[storage_mapping.rs](/Users/liulipeng/workspace/rust/paradown/src/storage/mapping.rs:1)
  - 把 runtime -> DB 的 task/worker/checksum 转换迁出 `persistence.rs`
  - 把 DB -> restore request 的转换迁出 `persistence.rs`
  - 为文本字段归一化和 worker 请求映射补了单测
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/registry.rs:1) 收敛为注册层
  - `restore_tasks()` 现在只负责读取 bundle、调用 recovery、注册任务
  - worker 恢复装配单独收成 helper，不再混着恢复规则
- [persistence.rs](/Users/liulipeng/workspace/rust/paradown/src/storage/mod.rs:1) 收敛为存储门面
  - 保留 repository 调用、bundle 加载、task/worker/checksum 保存入口
  - 不再直接承载一整套模型转换细节
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1) 接入新的内部模块

这一轮解决的主要问题：

- `coordinator_registry.rs` 之前仍然夹着完整恢复规则，职责边界不清
- `persistence.rs` 之前同时承担存储入口和模型映射，文件过厚
- 恢复规则与存储转换都不方便独立测试
- 协调器、恢复层、存储层之间还缺一层明确的“恢复输入/恢复决策”边界

### 3.10 第十轮：补全恢复与下载链路集成测试

提交：

- `654e26a` `补充恢复与下载链路集成测试`

已完成内容：

- 新增集成测试文件：[runtime_integration.rs](/Users/liulipeng/workspace/rust/paradown/tests/runtime_integration.rs:1)
  - 验证 SQLite 持久化数据在 manager 重启后能恢复为 paused 任务
  - 验证本地 HTTP server 下真实下载链路可以完整跑通
- 为后续限速和 CLI 收尾建立了稳定的端到端回归基线

这一轮解决的主要问题：

- 之前恢复路径和下载运行时主要靠单元测试验证
- 缺少真正穿过 `manager -> task -> worker -> storage` 的回归路径

### 3.11 第十一轮：接入全局速率限制并收紧运行时错误分类

提交：

- `01dc93e` `接入全局速率限制并收紧运行时错误分类`

已完成内容：

- 新增全局限速模块：[rate_limiter.rs](/Users/liulipeng/workspace/rust/paradown/src/rate_limiter.rs:1)
- [manager.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator/mod.rs:1) 接入共享 rate limiter，并提供运行时更新接口
- [worker/transfer.rs](/Users/liulipeng/workspace/rust/paradown/src/worker/transfer.rs:1) 在实际写入路径上接入限速控制
- 流式响应的 chunk 错误改为更明确的 `NetworkError`
- 集成测试补上真实限速下载验证

这一轮解决的主要问题：

- `rate_limit_kbps` 之前只是配置字段，没有真正影响下载行为
- worker 流式下载中的网络错误分类不够明确

### 3.12 第十二轮：收拢 CLI 入口并拆分请求模型

提交：

- `124cd8e` `收拢 CLI 入口并拆分请求模型`

已完成内容：

- [main.rs](/Users/liulipeng/workspace/rust/paradown/src/main.rs:1) 改为真正的 CLI 入口层
  - CLI 覆盖会统一应用到默认配置或 TOML 配置之上
  - 默认下载完成后自动退出，不再无条件卡在 stdin
  - interactive mode 改为显式 `--interactive`
  - interactive 命令已真正接入 manager
- [cli.rs](/Users/liulipeng/workspace/rust/paradown/src/cli.rs:1) 收成更明确的命令层
  - 支持 `help/status/pause/resume/cancel/limit <kbps|off>`
  - 为命令解析补了单测
- [request.rs](/Users/liulipeng/workspace/rust/paradown/src/request/mod.rs:1) 及其子模块拆分为：
  - [task.rs](/Users/liulipeng/workspace/rust/paradown/src/request/task.rs:1)
  - [segment.rs](/Users/liulipeng/workspace/rust/paradown/src/request/segment.rs:1)
- [README.md](/Users/liulipeng/workspace/rust/paradown/README.md:1) 已与实际实现重新对齐

这一轮解决的主要问题：

- CLI 之前会无条件等待 stdin，非交互运行体验不正确
- interactive mode 之前只是壳层，没有真正接入 manager
- `request.rs` 之前仍然把 job/segment 请求混在同一个文件
- README 和 `--help` 与实际实现不一致

### 3.13 第十三轮：重组包结构并清理公开 API

提交：

- `384efd6` `重组包结构并重塑公开接口`

已完成内容：

- `src` 根目录按子域收口为目录模块：
  - `coordinator/`
  - `job/`
  - `worker/`
  - `storage/`
  - `request/`
  - `repository/`
- 公开 API 重新收成两层正式出口：
  - `download::{Manager, Task, Worker, TaskRequest, SegmentRequest, Event, Status}`
  - `storage::{Store, Backend}`
- 根级导出统一为更短、更领域化的名字：
  - `Config`
  - `ConfigBuilder`
  - `Error`
  - `Checksum`
- 去掉历史兼容别名，不再继续暴露：
  - `DownloadCoordinator`
  - `DownloadTask`
  - `DownloadWorker`
  - `DownloadPersistenceManager`
  - `PersistenceType`
- 配置字段和 builder API 进一步语义化：
  - `max_concurrent_downloads` -> `concurrent_tasks`
  - `worker_threads` -> `segments_per_task`
  - `persistence_type` -> `storage_backend`
- `repository/` 目录继续收口：
  - `repository.rs` -> `contract.rs`
  - `memory_repository.rs` -> `memory.rs`
  - `sqlite_repository.rs` -> `sqlite.rs`
  - trait 名称统一为 `Repository`
- 移除了未接入主流程的旧 `progress.rs` 模块，并同步删掉无效依赖

这一轮解决的主要问题：

- `src` 根目录文件过多，模块结构已经不能表达真实子域
- 对外 API 仍然保留大量历史兼容别名，学习成本偏高
- 配置命名带有实现痕迹，不够贴近使用语义
- `repository/` 内部文件命名和 trait 命名不统一
- 旧进度模块长期未接线，但仍然参与编译和告警

### 3.14 第十四轮：引入下载规格并拆出单源发现传输边界

提交：

- `59d7517` `引入下载规格并拆出单源发现传输边界`

已完成内容：

- 新增下载规格模型：
  - [spec.rs](/Users/liulipeng/workspace/rust/paradown/src/domain/spec.rs:1)
  - `DownloadSpec` 统一表达 `HTTP / HTTPS / FTP`
- 新增发现层边界：
  - [origin.rs](/Users/liulipeng/workspace/rust/paradown/src/discovery/origin.rs:1)
  - 统一把资源探测收口成 `OriginMetadata`
- 新增传输驱动边界：
  - [driver.rs](/Users/liulipeng/workspace/rust/paradown/src/transfer/driver.rs:1)
  - [http.rs](/Users/liulipeng/workspace/rust/paradown/src/transfer/http.rs:1)
  - [ftp.rs](/Users/liulipeng/workspace/rust/paradown/src/transfer/ftp.rs:1)
- `Manager / TaskRequest / Task / Worker` 都开始面向 `DownloadSpec` 工作
- `Manager` 增加 `add_download(DownloadSpec)` 入口
- 对外导出 `Session / SessionSnapshot`，把“会话”概念先立起来

这一轮解决的主要问题：

- 系统内部仍把裸 `url` 当作唯一真相
- 协议差异散落在 worker 执行层，不利于后续扩协议
- 发现逻辑、传输逻辑和作业生命周期之间缺少明确边界

### 3.15 第十五轮：引入会话 Manifest 与 PayloadStore

提交：

- 见当前这轮 P3 提交

已完成内容：

- 新增统一资源描述模型：
  - [manifest.rs](/Users/liulipeng/workspace/rust/paradown/src/domain/manifest.rs:1)
  - [piece.rs](/Users/liulipeng/workspace/rust/paradown/src/domain/piece.rs:1)
  - `SessionManifest / FileManifest / PieceLayout / PieceBlock`
- 新增 payload 层：
  - [store.rs](/Users/liulipeng/workspace/rust/paradown/src/payload/store.rs:1)
  - [file_map.rs](/Users/liulipeng/workspace/rust/paradown/src/payload/file_map.rs:1)
  - [verifier.rs](/Users/liulipeng/workspace/rust/paradown/src/payload/verifier.rs:1)
- [mod.rs](/Users/liulipeng/workspace/rust/paradown/src/job/mod.rs:1) 现在持有 manifest 和 payload store
- [prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job/prepare.rs:1) 会先把 HTTP 单文件转换成 `SessionManifest`，再准备 payload store
- [http.rs](/Users/liulipeng/workspace/rust/paradown/src/transfer/http.rs:1) 不再直接决定文件 seek/write，而是通过 payload store 以全局偏移写入
- [finalize.rs](/Users/liulipeng/workspace/rust/paradown/src/job/finalize.rs:1) 与准备阶段复用统一的 payload checksum 校验逻辑
- 对外公开导出 `SessionManifest / FileManifest / PieceLayout / PieceBlock`

这一轮解决的主要问题：

- manifest / piece 模型只停留在设计文档，没有真正进入运行时
- worker 仍直接面向单文件写入，无法为多文件/piece/block 模型让路
- checksum 校验逻辑在准备阶段和完成阶段重复实现
- 后续要接 BT、磁力或多源时，缺少统一的 payload 抽象

### 3.16 第十六轮：引入 piece-aware 调度规划骨架

提交：

- 见当前这轮 P4 提交

已完成内容：

- 新增调度模块：
  - [planner.rs](/Users/liulipeng/workspace/rust/paradown/src/scheduler/planner.rs:1)
  - 基于 `SessionManifest.pieces` 生成 worker assignment
  - 新增 HTTP 场景的 piece size 建议策略
- [prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job/prepare.rs:1) 不再只按固定默认 piece size 生成 manifest
  - 会根据 `segments_per_task` 和 range 能力生成更适合并行的 piece 布局
- [workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job/workers.rs:1) 不再从总字节数直接切 chunk
  - worker 创建改为消费 scheduler 生成的 piece-aligned assignment
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1) 接入 `scheduler/` 子域
- 为 scheduler 补上 piece size 与 assignment 规划测试

这一轮解决的主要问题：

- `P3` 虽然已经有 piece/manifest 模型，但 worker 规划仍然停留在 `total_size -> byte chunk`
- 小文件在进入 piece 模型后容易退化成单 piece / 单 worker
- 调度真相还没有真正从 `chunk.rs` 迁到 `manifest + scheduler`

### 3.17 第十七轮：补齐 piece state 与基于 piece 的运行时进度重建

提交：

- 见当前这轮 P4 收尾提交

已完成内容：

- [piece.rs](/Users/liulipeng/workspace/rust/paradown/src/domain/piece.rs:1) 新增 `PieceState`
  - 可初始化 piece bitmap
  - 可按覆盖区间标记完整 piece
  - 增加 piece state 单测
- [mod.rs](/Users/liulipeng/workspace/rust/paradown/src/job/mod.rs:1) 现在在任务运行时持有 `piece_states`
  - 安装 manifest 时初始化 piece state
  - 可从当前 worker 进度重建 piece bitmap
  - `TaskSnapshot` 新增 `completed_pieces / piece_count`
- [workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job/workers.rs:1)
  - worker progress / complete / pause 事件都会触发 piece-aware 进度重算
  - 既有 worker 是否可复用，改为校验是否仍匹配当前 scheduler assignment
- [finalize.rs](/Users/liulipeng/workspace/rust/paradown/src/job/finalize.rs:1)
  - 成功完成后统一把 piece state 收敛为全部完成
- [main.rs](/Users/liulipeng/workspace/rust/paradown/src/main.rs:1)
  - 任务摘要输出增加 piece 维度进度

这一轮解决的主要问题：

- `P4` 之前虽然已有 piece-aware planner，但运行时进度仍主要停留在裸 worker byte
- 恢复后的 piece 完成度不能从已有 worker 状态重建
- scheduler 改了之后，旧 worker 布局是否仍可复用没有严格校验

## 4. 当前状态

截至当前，主干架构重构已经完成，多协议基础改造已经完成 P4。

已经完成的核心收口：

- `manager`、`task`、`worker`、`persistence/storage` 的职责边界已经明显清晰
- 包结构已经按 `coordinator / job / worker / storage / request / repository` 收口
- 恢复规则和存储映射已经从协调器与存储门面中拆出
- worker 运行时已经拆成 facade / runtime / transfer / retry 四层
- `request` 模型已经拆成 task request 与 segment request
- CLI、interactive mode、README、`--help` 已经和当前实现基本对齐
- 对外 API 已经切换到新的正式出口，不再保留旧兼容别名
- 自动化验证已经覆盖单元测试、恢复测试、真实下载集成测试、限速集成测试
- HTTP/HTTPS 已经跑在 `DownloadSpec + discovery + transfer + SessionManifest + PayloadStore` 这条新主线上
- HTTP worker 规划已经切到 `SessionManifest.pieces -> scheduler::planner -> Worker`
- HTTP 运行时已经具备 `piece state / piece bitmap` 重建能力
- FTP 目前已经有发现层/传输层架构占位，但真实协议实现还没开始

## 5. 仍可继续优化的点

这些已经不再是主干架构重构的阻塞项，更偏后续增强：

- `Backend::JsonFile(...)` 仍未实现
- [job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job/prepare.rs:1) 仍然兼有目录准备、文件策略和协议结果消费，后续还可以再细拆
- interactive mode 目前是全局命令，不是按 task 粒度控制

## 6. 当前判断

截至 2026-04-15，重构工作已经完成了十七轮，当前可以认为：

- 主干架构重构已经完成
- 多协议演进的前三步骨架已经落地
- `P4` 已经完成，HTTP 调度真相已经切到 piece 维度
- 运行时正确性、恢复正确性、速率限制和入口体验都已经落到可验证状态
- 项目从“原型结构”推进到了“有清晰边界和回归基线的可演进结构”
- 后续更值得投入的是 P5 的 torrent/magnet discovery、FTP 真实现和更深的协议扩展，而不是继续大规模拆主干文件
