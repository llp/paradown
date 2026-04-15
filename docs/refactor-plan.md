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
  - `DownloadCoordinator`：协调器
  - `DownloadJob`：单个下载作业
  - `SegmentWorker`：单作业内的分段下载执行单元

- `coordinator_*`
  - 事件收敛
  - 队列与 permit 管理
  - 作业注册与恢复

- `job_*`
  - 下载前准备
  - 下载完成收敛

- `storage::*`
  - 持久化入口
  - repository 抽象
  - sqlite / memory 实现

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
- 新增 `storage` 命名空间：[storage.rs](/Users/liulipeng/workspace/rust/paradown/src/storage.rs:1)
- 为核心类型补充领域命名别名：
  - `DownloadCoordinator`
  - `DownloadJob`
  - `SegmentWorker`
  - `DownloadStore`
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

### 3.2 第二轮：拆分 DownloadJob 的准备与完成流程

提交：

- `962e2fc` `拆分下载作业的准备与完成流程`

已完成内容：

- 新增下载前准备模块：[job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job_prepare.rs:1)
  - 下载目录准备
  - 文件名/文件路径准备
  - `HEAD + Content-Length` 探测
  - 已存在文件策略处理
  - 零字节文件处理
- 新增下载完成收敛模块：[job_finalize.rs](/Users/liulipeng/workspace/rust/paradown/src/job_finalize.rs:1)
  - checksum 校验
  - 完成/失败状态收敛
  - 完成/失败事件发送
- `task.rs` 不再把准备阶段和完成阶段全部揉在 `start()` 和 worker 完成事件里
- 抽出 `resolve_or_init_file_name()` 等辅助能力，减小 `task.rs` 的局部复杂度

这一轮解决的主要问题：

- `task.rs` 既做准备又做完成收敛，方法过长
- checksum 收敛逻辑与下载执行逻辑混杂
- 文件准备逻辑难以单独替换或测试

### 3.3 第三轮：拆分 DownloadCoordinator 的事件、队列和注册逻辑

提交：

- `1ee3b01` `拆分下载协调器的事件队列与注册逻辑`

已完成内容：

- 新增协调器事件模块：[coordinator_events.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_events.rs:1)
  - manager 事件循环迁出
  - terminal 事件统一收敛
- 新增协调器队列模块：[coordinator_queue.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_queue.rs:1)
  - permit 获取/释放
  - 排队
  - 触发下一个任务
- 新增协调器注册模块：[coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_registry.rs:1)
  - 从持久化恢复任务
  - 注册新任务
  - worker 恢复装配
- `manager.rs` 明显瘦身，更多保留公开 API 和薄协调逻辑
- `start_task / resume_task` 的骨架已开始统一收敛

这一轮解决的主要问题：

- `manager.rs` 同时承担事件消费、排队调度、恢复组装、注册逻辑
- 协调器壳层太厚，不利于后续继续演进

### 3.4 第四轮：拆分 DownloadJob 的 worker、state 与 storage 逻辑

提交：

- `0a1d8ef` `拆分下载作业的 worker 状态与存储逻辑`

已完成内容：

- 新增 worker 协调模块：[job_workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job_workers.rs:1)
  - worker 事件监听
  - worker 创建与装配
  - worker 启动与 join 收敛
  - worker 级进度/完成/错误/取消事件处理
- 新增作业状态模块：[job_state.rs](/Users/liulipeng/workspace/rust/paradown/src/job_state.rs:1)
  - `start` 前状态判断
  - pause/resume/cancel/delete 状态流转
  - reset/delete file/clear workers 等状态辅助逻辑
- 新增作业存储模块：[job_storage.rs](/Users/liulipeng/workspace/rust/paradown/src/job_storage.rs:1)
  - task/checksum/worker 持久化
  - task/workers/checksums 清理
- [task.rs](/Users/liulipeng/workspace/rust/paradown/src/task.rs:1) 收敛为作业门面
  - 保留结构定义、`new`、`snapshot`、`init`、`start`
  - 保留文件名/文件路径辅助方法
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1) 接入新的内部模块
- `task.rs` 体量从重构前的 `884` 行下降到本轮后的 `247` 行

这一轮解决的主要问题：

- `task.rs` 同时承担 worker 生命周期、状态机、持久化辅助和文件清理
- `DownloadJob` 作为门面类型仍然过重，不利于继续演进
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
- [job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job_prepare.rs:1) 接入协议探测结果
  - 不再只依赖 `HEAD + Content-Length`
  - 显式记录目标资源是否支持 range
  - 对不支持 range 的部分文件恢复改为安全回退到整文件重下
  - 文件缺失但持久化仍有进度时，清理过期 worker/progress 状态
- [task.rs](/Users/liulipeng/workspace/rust/paradown/src/task.rs:1) 增加协议探测状态
  - `range_requests_supported`
  - `protocol_probe_completed`
- [job_state.rs](/Users/liulipeng/workspace/rust/paradown/src/job_state.rs:1) 调整恢复语义
  - 恢复到当前进程后，如果还没有重新探测协议能力，暂停任务会重新走准备流程
- [job_workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job_workers.rs:1) 根据协议能力决定 worker 规划
  - 不支持 range 时只允许单 worker
  - 不兼容的历史 worker 布局会被重建
- [worker.rs](/Users/liulipeng/workspace/rust/paradown/src/worker.rs:1) 收紧响应校验
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

- [sqlite_repository.rs](/Users/liulipeng/workspace/rust/paradown/src/repository/sqlite_repository.rs:1) 修正了 task 持久化主键语义
  - `download_tasks` 改为按 `id` upsert，而不是按 `url` upsert
  - 保存 task 时显式写入 `id`
  - 避免 task id 与 URL upsert 语义漂移
- [sqlite_repository.rs](/Users/liulipeng/workspace/rust/paradown/src/repository/sqlite_repository.rs:1) 修正了时间字段 round-trip
  - 新写入的时间戳统一为 RFC3339 风格文本
  - 读取时兼容 RFC3339 和 SQLite `CURRENT_TIMESTAMP` 旧格式
- [memory_repository.rs](/Users/liulipeng/workspace/rust/paradown/src/repository/memory_repository.rs:1) 修正了 worker/checksum 的 key 冲突
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

- [persistence.rs](/Users/liulipeng/workspace/rust/paradown/src/persistence.rs:1) 新增了更明确的 storage API
  - 新增 `StoredDownloadBundle`
  - 新增 `load_task_bundles()`
  - 新增 DB 模型到 `DownloadTaskRequest` / `DownloadWorkerRequest` 的转换方法
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_registry.rs:1) 不再直接拼装底层 DB 模型
  - 恢复逻辑改为消费 `persistence.rs` 提供的 bundle 和转换结果
  - 恢复状态判断与存储读取职责开始分离
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_registry.rs:1) 收紧了恢复信任边界
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

### 3.8 第八轮：拆分 DownloadWorker 的运行时、传输与重试逻辑

提交：

- `084c852` `拆分下载 worker 的运行时与传输重试逻辑`

已完成内容：

- 新增 worker 运行时模块：`src/worker_runtime.rs`
  - 把 `start()` 主流程从 `worker.rs` 迁出
  - 收敛 worker 启动、成功、失败、重试主循环
  - 把 `is_running` 的收尾从多处分散设置改为统一出口
- 新增 worker 传输模块：`src/worker_transfer.rs`
  - 抽离请求构建
  - 抽离响应协议校验
  - 抽离文件打开、seek、写入和进度节流
  - 把 worker 进度上报封装成独立 helper
- 新增 worker 重试模块：`src/worker_retry.rs`
  - 让 `initial_delay / max_delay / backoff_factor` 真正参与运行时
  - 为 retry delay 增加独立单测
- [worker.rs](/Users/liulipeng/workspace/rust/paradown/src/worker.rs:1) 收敛为 worker 门面
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

- 见本轮提交记录

已完成内容：

- 新增恢复模块：[recovery.rs](/Users/liulipeng/workspace/rust/paradown/src/recovery.rs:1)
  - 把恢复计划、状态降级、worker 布局校验从 `coordinator_registry.rs` 迁出
  - 让恢复层直接消费统一的 storage bundle，而不是耦合协调器实现
  - 为 completed 文件尺寸不匹配场景补了针对性测试
- 新增存储映射模块：[storage_mapping.rs](/Users/liulipeng/workspace/rust/paradown/src/storage_mapping.rs:1)
  - 把 runtime -> DB 的 task/worker/checksum 转换迁出 `persistence.rs`
  - 把 DB -> restore request 的转换迁出 `persistence.rs`
  - 为文本字段归一化和 worker 请求映射补了单测
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_registry.rs:1) 收敛为注册层
  - `restore_tasks()` 现在只负责读取 bundle、调用 recovery、注册任务
  - worker 恢复装配单独收成 helper，不再混着恢复规则
- [persistence.rs](/Users/liulipeng/workspace/rust/paradown/src/persistence.rs:1) 收敛为存储门面
  - 保留 repository 调用、bundle 加载、task/worker/checksum 保存入口
  - 不再直接承载一整套模型转换细节
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1) 接入新的内部模块

这一轮解决的主要问题：

- `coordinator_registry.rs` 之前仍然夹着完整恢复规则，职责边界不清
- `persistence.rs` 之前同时承担存储入口和模型映射，文件过厚
- 恢复规则与存储转换都不方便独立测试
- 协调器、恢复层、存储层之间还缺一层明确的“恢复输入/恢复决策”边界

## 4. 本轮重构具体改了什么

如果只聚焦“这一轮”即第九轮，已经修改的重点如下：

### 4.1 已改

- 恢复计划和恢复规则已经整体迁到 `recovery.rs`
- `persistence.rs` 里的模型转换已经迁到 `storage_mapping.rs`
- `coordinator_registry.rs` 现在基本只保留任务注册、恢复装配和去重逻辑
- storage mapping 和 recovery 都补了独立测试，恢复层的 completed 文件尺寸校验也补齐了
- 这轮后 `persistence.rs` 和 `coordinator_registry.rs` 都不再是“既当门面又当规则中心”的混合角色

### 4.2 还没改完

- `persistence.rs` 这个命名仍然偏历史包袱，对外虽然有 `storage`，内部还没彻底对齐
- `request.rs` 里的 task/worker request 还没有按 job/segment 拆开
- `worker` runtime 的限速、I/O 错误分类和更细的可观测性还没继续收口
- 恢复路径虽然已经模块化，但还缺少更完整的跨进程端到端验证
- `main.rs` 的 interactive mode 仍未完整接线

### 4.3 本轮实际触达的文件

本轮第九轮重构实际触达的核心文件如下：

- [recovery.rs](/Users/liulipeng/workspace/rust/paradown/src/recovery.rs:1)
- [storage_mapping.rs](/Users/liulipeng/workspace/rust/paradown/src/storage_mapping.rs:1)
- [coordinator_registry.rs](/Users/liulipeng/workspace/rust/paradown/src/coordinator_registry.rs:1)
- [persistence.rs](/Users/liulipeng/workspace/rust/paradown/src/persistence.rs:1)
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1)

本轮的重构重点不是新增功能，而是让恢复链路和存储映射成为两层明确职责：

- `recovery.rs` 负责恢复输入的可信性判断和恢复计划构建
- `storage_mapping.rs` 负责 runtime 模型和存储模型之间的转换
- `coordinator_registry.rs` 负责把恢复计划装配回运行时任务

这样做的直接收益是：后续如果继续补恢复集成测试、重构 storage 命名或调整恢复规则，不需要再改协调器和存储门面的大文件。

### 4.4 本轮明确未触达的范围

这轮有意识地没有去碰下面这些区域，原因是先把恢复边界和存储映射层拉直，再进入更细的运行时与用户层收口：

- `storage` 的内部命名仍然还没有完全替换掉 `persistence`
- `rate_limit_kbps` 还没有真正接到 worker 运行时
- [job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job_prepare.rs:1) 里的目录准备、文件策略和协议结果消费仍然混杂
- [main.rs](/Users/liulipeng/workspace/rust/paradown/src/main.rs:1) 的 interactive mode 接线
- README、CLI 帮助文案、默认值说明的一致性问题

## 5. 尚未完成的重构

下面这些属于“已经看清楚问题，但这几轮还没完全动到”的部分。

### 5.1 recovery/storage 边界已经明确，但还没完全产品化

这一轮之后，恢复链路已经更稳，但仍未完全收口的职责主要变成：

- `storage` 命名空间和 `persistence.rs` 历史命名之间的不一致
- `request.rs` 里的恢复请求模型仍然偏杂
- worker runtime 里剩余的限速、I/O 分类与更细的错误分层

建议后续进一步收敛为：

- `protocol_probe`
- `job_prepare`
- `storage model`
- `recovery layer`
- `worker runtime`

### 5.2 持久化层命名和模型还未真正重构

虽然对外已经补了 `storage` 命名空间，但内部仍然主要是：

- `persistence.rs`
- `repository/*`

还没有做的事：

- 统一 `storage` 层命名
- 梳理 task/worker/checksum 的主键与关联关系
- 修正持久化模型和恢复模型之间的边界

### 5.3 `request.rs` 仍然偏杂

当前 `DownloadTaskRequest` 和 `DownloadWorkerRequest` 还混在一个文件里，后续可以考虑：

- job request
- segment request
- builder

分开组织。

### 5.4 `main.rs` 仍然有 CLI 与交互壳逻辑

虽然 `main.rs` 比之前清爽很多，但还没完成的点包括：

- interactive mode 真实接线
- CLI 参数与配置覆盖策略统一
- 帮助信息与 README 对齐

### 5.5 协议正确性主干已建立，但运行时与恢复验证还未完全收口

目前还没做完的关键结构性工作：

- 让 `rate_limit_kbps` 这类配置真正影响运行时行为
- 为 worker 的 I/O 错误和协议错误补更细的分类
- 为跨进程恢复补更完整的端到端验证
- 为 recovery/storage 边界补更靠近真实场景的集成测试

这部分会直接决定“恢复后是否真的能稳定继续下载”。

## 6. 当前未重构但高优先级的问题

这些问题不是“结构还不够漂亮”，而是会影响正确性和稳定性：

- 断点续传仍未真正闭环
- 恢复路径虽然已模块化，但还缺端到端验证来证明规则真的可靠
- worker 运行时职责仍然偏杂，出错路径较难继续收紧
- CLI/README 仍然存在偏差
- 测试覆盖仍然很薄

## 7. 下一阶段重构计划

### 阶段 A：继续拆 `task.rs`

目标：

- 让 `DownloadJob` 成为真正的作业门面，而不是 God object

计划：

- 拆 worker 生命周期与 worker 集合管理
- 拆作业状态流转
- 拆作业重置/删除逻辑
- 拆作业持久化辅助逻辑

状态：

- 已完成
- 当前 `task.rs` 已收敛为门面层，后续不再优先做表面拆分

### 阶段 B：重构协议探测与下载正确性层

目标：

- 先让下载行为正确，再谈恢复与优化

计划：

- 独立 range 能力探测
- 校验 `206` 与 `Content-Range`
- 明确单线程退化路径
- 明确分块下载允许条件

状态：

- 已完成第一阶段
- 已完成第二阶段
- 仍需继续接入限速与补齐 worker 端到端验证

### 阶段 C：重构存储模型

目标：

- 让恢复逻辑可信

计划：

- 统一 task/worker/checksum 的主键与关联设计
- 校正 sqlite 时间字段处理
- 修正 memory backend 的冲突问题
- 为恢复路径补集成测试

状态：

- 已完成第一阶段
- 已完成第二阶段
- 已完成第三阶段
- 仍需继续补 recovery/storage 的集成测试与对外命名收口

### 阶段 D：补测试

目标：

- 让重构后可以稳定迭代

计划：

- chunk 规划单测
- job_prepare 单测
- finalize 单测
- manager/coordinator 行为测试
- 模拟 server 的集成测试

### 阶段 E：整理用户层体验

目标：

- 让 CLI 和文档一致

计划：

- 清理 interactive mode 接线
- 对齐 README 与实际实现
- 明确实验性能力和正式能力

## 8. 当前建议的推进顺序

建议按照下面顺序继续：

1. 补 worker 与恢复路径的端到端测试
2. 接上 `rate_limit_kbps` 和更细的运行时错误分类
3. 最后再收 CLI/README/交互体验

原因：

- 当前最大的结构复杂度已经从 `task.rs` 转移到了 worker runtime 的剩余细节和缺失的恢复集成验证
- 当前最大的正确性风险仍然在协议层和恢复层
- 如果先做 UI 或 CLI 体验，只会把错误行为包装得更漂亮

## 9. 文档维护规则

从现在开始，每一轮重构都应该更新本文件：

- 写明本轮提交号
- 写明本轮改了哪些模块
- 写明本轮没有改哪些问题
- 写明下一轮计划

这样能保证重构过程可回顾、可中断、可接续。

## 10. 当前判断

截至 2026-04-15，重构工作已经完成了九轮，当前可以认为：

- 对外命名和主干分层已经开始稳定
- `manager.rs` 的结构性压力已经明显下降
- `task.rs` 已经从复杂度中心退回到门面层
- `protocol_probe.rs` 已经补齐，存储后端的主键/时间模型也更稳定了
- `worker.rs`、`coordinator_registry.rs`、`persistence.rs` 都已经明显瘦身，恢复规则和存储映射也开始独立
- 当前更需要优先补的是恢复/运行时的真实场景验证，而不只是继续拆文件
- 协议正确性和持久化一致性仍是最需要优先解决的稳定性问题

因此，下一轮不建议再做表面命名调整，而应该直接进入恢复链路与 worker runtime 的端到端测试和运行时行为补齐。
