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

- 见本轮提交记录

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

- 见本轮提交记录

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

## 4. 本轮重构具体改了什么

如果只聚焦“这一轮”即第五轮，已经修改的重点如下：

### 4.1 已改

- 新增 [protocol_probe.rs](/Users/liulipeng/workspace/rust/paradown/src/protocol_probe.rs:1)，把资源探测从 `job_prepare` 的局部细节升级成单独的协议层
- `job_prepare.rs` 现在基于协议探测结果决定是否允许 resume、是否允许多 worker，而不是隐式假设服务端支持 range
- `worker.rs` 现在会按探测结果区分 `206 Partial Content` 和 `200 OK` 两种合法路径
- `worker.rs` 现在会从自身已持久化的 `downloaded_size` 继续，而不是每次重启都从 0 开始
- `job_state.rs` 现在会在“恢复到当前进程但尚未重新探测”的场景下重新走准备流程
- `job_workers.rs` 会在协议能力与历史 worker 布局不兼容时重建 worker 集合

### 4.2 还没改完

- `job_prepare.rs` 仍然同时包含下载前目录准备、文件策略和协议探测结果消费
- `worker.rs` 里仍然混着协议判断、重试、节流和写文件逻辑
- `persistence.rs` 与 `repository/*` 的模型边界还没真正重构
- `main.rs` 的 interactive mode 仍未完整接线

### 4.3 本轮实际触达的文件

本轮第五轮重构实际触达的核心文件如下：

- [protocol_probe.rs](/Users/liulipeng/workspace/rust/paradown/src/protocol_probe.rs:1)
- [job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job_prepare.rs:1)
- [job_state.rs](/Users/liulipeng/workspace/rust/paradown/src/job_state.rs:1)
- [job_workers.rs](/Users/liulipeng/workspace/rust/paradown/src/job_workers.rs:1)
- [worker.rs](/Users/liulipeng/workspace/rust/paradown/src/worker.rs:1)
- [task.rs](/Users/liulipeng/workspace/rust/paradown/src/task.rs:1)
- [lib.rs](/Users/liulipeng/workspace/rust/paradown/src/lib.rs:1)

本轮的重构重点不是新增功能，而是把“资源协议事实”真正变成可被作业层和 worker 层共享的显式能力：

- `protocol_probe.rs` 负责探测 total size 和 range 能力
- `job_prepare.rs` 负责消费探测结果并决定是否允许安全 resume
- `job_workers.rs` 负责根据探测结果决定 worker 布局
- `worker.rs` 负责对运行时响应做最终协议校验

这样做的直接收益是：后续如果继续改协议退化策略或下载正确性，不需要再让 `job_prepare` 和 `worker` 各自维护一套隐式假设。

### 4.4 本轮明确未触达的范围

这轮有意识地没有去碰下面这些区域，原因是先把协议探测与响应校验拉直，再进入存储模型和剩余运行时细节：

- [worker.rs](/Users/liulipeng/workspace/rust/paradown/src/worker.rs:1) 里的重试、节流和写文件流程仍然混在一起
- [persistence.rs](/Users/liulipeng/workspace/rust/paradown/src/persistence.rs:1) 与 `repository/*` 的持久化模型一致性
- [job_prepare.rs](/Users/liulipeng/workspace/rust/paradown/src/job_prepare.rs:1) 里的目录准备、文件策略和协议结果消费仍然混杂
- [main.rs](/Users/liulipeng/workspace/rust/paradown/src/main.rs:1) 的 interactive mode 接线
- README、CLI 帮助文案、默认值说明的一致性问题

## 5. 尚未完成的重构

下面这些属于“已经看清楚问题，但这几轮还没完全动到”的部分。

### 5.1 协议探测已经独立，但 prepare / worker / storage 边界还未彻底收口

这一轮之后，协议层已经有了独立入口，但仍未完全收口的职责主要变成：

- `job_prepare.rs` 里的目录准备、文件策略与 resume 决策
- `worker.rs` 里的重试、节流与文件写入细节
- `job_storage.rs` 只是抽离了调用位置，还没有重构底层存储模型

建议后续进一步收敛为：

- `protocol_probe`
- `job_prepare`
- `storage model`
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

### 5.5 协议正确性层还没正式抽象

目前还没做的关键结构性工作：

- 把 range 能力探测独立成协议探测层
- 显式校验 `206 Partial Content`
- 显式校验 `Content-Range`
- 把“服务器支持多段下载吗”从 job_prepare 里再细分出来

这部分会直接决定后续下载正确性。

## 6. 当前未重构但高优先级的问题

这些问题不是“结构还不够漂亮”，而是会影响正确性和稳定性：

- 断点续传仍未真正闭环
- 不支持 range 的服务端场景仍未严谨处理
- SQLite 持久化模型的一致性问题仍在
- Memory backend 的 key 设计问题仍在
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
- 仍需继续整理 `worker.rs` 内部的重试、节流和写文件责任边界

### 阶段 C：重构存储模型

目标：

- 让恢复逻辑可信

计划：

- 统一 task/worker/checksum 的主键与关联设计
- 校正 sqlite 时间字段处理
- 修正 memory backend 的冲突问题
- 为恢复路径补集成测试

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

1. 重构持久化模型
2. 补自动化测试
3. 最后再收 CLI/README/交互体验

原因：

- 当前最大的结构复杂度已经从 `task.rs` 转移到了 `worker.rs` 和存储层
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

截至 2026-04-15，重构工作已经完成了五轮，当前可以认为：

- 对外命名和主干分层已经开始稳定
- `manager.rs` 的结构性压力已经明显下降
- `task.rs` 已经从复杂度中心退回到门面层
- `protocol_probe.rs` 已经补齐，但 `job_prepare.rs`、`worker.rs` 和持久化模型成为新的主要复杂度中心
- 协议正确性和持久化一致性仍是最需要优先解决的稳定性问题

因此，下一轮不建议再做表面命名调整，而应该直接进入持久化模型和 `worker.rs` 运行时职责的实质性重构。
