# paradown 多协议下载引擎可执行改造路线图

更新时间：2026-04-15

仓库路径：`/Users/liulipeng/workspace/rust/paradown`

关联文档：

- [多协议演进架构分析](/Users/liulipeng/workspace/rust/paradown/docs/multi-protocol-architecture.md:1)
- [项目重构计划](/Users/liulipeng/workspace/rust/paradown/docs/refactor-plan.md:1)

## 1. 目标定义

本路线图的目标不是“继续把 HTTP 下载器写复杂”，而是把 `paradown` 从当前的：

- `URL / 文件 / 区间` 模型

逐步升级成：

- `DownloadSpec / SessionManifest / Source / Piece / PayloadStore` 模型

最终支撑的能力边界是：

- `HTTP/HTTPS`
- `FTP`
- `BT`
- `Magnet`
- `DHT / Tracker / Peer Wire`
- 多源调度
- P2SP
- 可选的离线下载 / 云端缓存源

## 2. 路线总览

整个改造建议拆成 8 个阶段，前 5 个属于“单机引擎重构”，后 3 个属于“协议扩展与多源能力”。

```mermaid
flowchart LR
    P0["P0 基线加固"] --> P1["P1 抽离下载规格与会话壳层"]
    P1 --> P2["P2 引入发现层与传输驱动接口"]
    P2 --> P3["P3 引入 Manifest / Piece / PayloadStore"]
    P3 --> P4["P4 用 HTTP 先跑通 Piece 调度"]
    P4 --> P5["P5 接入 Torrent / Magnet 发现层"]
    P5 --> P6["P6 接入 BT Peer 传输与 Swarm"]
    P6 --> P7["P7 引入多源调度与 P2SP"]
    P7 --> P8["P8 接入云端离线与缓存源"]
```

建议的执行原则：

1. 每一阶段都必须保持主干可运行
2. 每一阶段都必须有新增测试，不允许只挪代码
3. 新模型先“并行引入”，后“切断旧模型”
4. 不要在一个阶段里同时引入“新抽象 + 新协议 + 新调度”

### 2.1 当前进度

截至当前代码状态：

- `P1` 已完成：`DownloadSpec` 已经进入主路径
- `P2` 已完成：`discovery / transfer` trait 边界已经立住
- `P3` 已完成第一阶段：`SessionManifest / PieceLayout / PayloadStore` 已经接入 HTTP 单文件下载主路径
- `P4` 已完成：HTTP 已经跑在 `manifest + scheduler + piece state` 这条调度主线上
- `FTP` 目前只有架构占位，真实发现与传输实现还未开始
- 恢复仍然通过 worker 状态落盘，但运行时已能从 worker 进度重建 piece bitmap

## 3. 当前代码与目标代码的映射

先把当前核心模块和未来目标位置对齐。

### 3.1 当前核心模块

- `src/coordinator/`
- `src/job/`
- `src/worker/`
- `src/storage/`
- `src/repository/`
- `src/request/`
- `src/scheduler/`
- `src/protocol_probe.rs`

### 3.2 目标核心模块

建议逐步落成：

```text
src/
  api/
    download.rs
  coordinator/
  discovery/
    origin.rs
    torrent.rs
    magnet.rs
    cloud.rs
  domain/
    spec.rs
    session.rs
    manifest.rs
    piece.rs
    source.rs
    events.rs
  scheduler/
    planner.rs
    piece_picker.rs
    source_scoring.rs
    bandwidth.rs
  transfer/
    driver.rs
    http.rs
    ftp.rs
    bt_peer.rs
    cloud.rs
  payload/
    store.rs
    file_map.rs
    verifier.rs
  state/
    store.rs
    mapping.rs
    repository/
  cli/
```

### 3.3 当前到未来的职责迁移

- 当前 `request::TaskRequest` -> 未来 `domain::spec::DownloadSpec`
- 当前 `job::Task` -> 未来 `domain::session::Session`
- 当前 `worker::Worker` -> 未来 `transfer` 层的执行单元
- 当前 `protocol_probe.rs` -> 未来 `discovery::origin`
- 当前 `scheduler::planner` -> 未来可继续细化为 piece picker / bandwidth / source scoring
- 当前 `storage::Store` -> 未来 `state::store::StateStore`
- 新增 `payload::store::PayloadStore`

## 4. 分阶段可执行计划

下面每个阶段都包含：

- 目标
- 具体改造任务
- 需要新增或迁移的模块
- 验收标准
- 本阶段明确不做的事

## 4.1 P0 基线加固

### 目标

在开始大改前，先把现有 HTTP 引擎的行为基线锁住，防止后面每一步都在“边改边坏”。

### 具体任务

1. 补全当前 HTTP 引擎的行为测试矩阵
2. 为恢复、暂停、取消、限速、错误分类补集成测试
3. 给当前公开 API 记录一份“将被废弃”的迁移清单
4. 给当前持久化模型加 schema 版本字段

### 模块设计

新增：

- `tests/http_resume_integration.rs`
- `tests/http_control_flow_integration.rs`
- `src/state_version.rs` 或并入 `storage/`

### 验收标准

- 现有 HTTP 功能有稳定回归基线
- 每次重构都可以靠测试判断有没有行为回退
- 持久化格式开始具备版本演进能力

### 本阶段不做

- 不引入新领域模型
- 不改主干公开 API
- 不上 BT

## 4.2 P1 抽离下载规格与会话壳层

### 目标

把“用户输入是什么”和“内部会话是什么”从当前 `TaskRequest / Task` 里拆开。

### 具体任务

1. 新建 `domain::spec::DownloadSpec`
2. 新建 `domain::session::SessionId / Session / SessionState`
3. 让 `Manager` 内部开始面向 `Session` 而不是直接面向 `Task`
4. 让当前 `TaskRequest` 退化成 HTTP 兼容适配器
5. 将当前 `download::Task` 对外 API 标记为阶段性壳层

### 建议数据结构

```rust
pub enum DownloadSpec {
    Http { url: String },
    Https { url: String },
    Ftp { url: String },
    TorrentFile { path: PathBuf },
    Magnet { uri: String },
    OfflineAsset { asset_id: String },
}

pub struct Session {
    pub id: u32,
    pub spec: DownloadSpec,
    pub state: SessionState,
    pub manifest_id: Option<String>,
}
```

### 模块设计

新增：

- `src/domain/spec.rs`
- `src/domain/session.rs`

改造：

- `src/download.rs`
- `src/coordinator/mod.rs`
- `src/request/task.rs`

### 验收标准

- `Manager::add_task()` 内部不再以 `url` 为核心真相
- 新会话层可以承载非 URL 型任务
- 现有 HTTP 下载仍然能跑通

### 本阶段不做

- 不引入 piece 模型
- 不改 worker 传输模型

## 4.3 P2 引入发现层与传输驱动接口

### 目标

把“准备资源”和“实际传输”从现在的 `job/prepare + worker` 里拆出去，先建立协议无关边界。

### 具体任务

1. 把 `protocol_probe.rs` 下沉到 `discovery/origin.rs`
2. 定义 `DiscoveryDriver` trait
3. 定义 `TransferDriver` trait
4. 把 `reqwest::Client` 从 `Worker` 结构中移出
5. 让 `job/prepare.rs` 只负责 orchestration，不直接做协议探测

### 建议接口

```rust
pub trait DiscoveryDriver {
    async fn discover(&self, spec: &DownloadSpec) -> Result<DiscoveryResult, Error>;
}

pub trait TransferDriver {
    async fn fetch_block(&self, assignment: BlockAssignment) -> Result<BlockPayload, Error>;
}
```

### 模块设计

新增：

- `src/discovery/mod.rs`
- `src/discovery/origin.rs`
- `src/transfer/mod.rs`
- `src/transfer/driver.rs`
- `src/transfer/http.rs`

改造：

- `src/job/prepare.rs`
- `src/worker/mod.rs`
- `src/worker/runtime.rs`
- `src/worker/transfer.rs`

### 验收标准

- 发现层与传输层有明确 trait 边界
- HTTP 驱动成为第一个正式驱动实现
- `Worker` 不再直接内嵌 `reqwest::Client + url + start/end` 这组 HTTP 真相

### 本阶段不做

- 不引入 BT 协议
- 不改持久化模型

## 4.4 P3 引入 Manifest / Piece / PayloadStore

### 目标

建立统一资源模型，这是整个工程里最关键的阶段。

### 具体任务

1. 新建 `SessionManifest`
2. 新建 `FileManifest`
3. 新建 `PieceLayout`
4. 新建 `PieceState / BlockState`
5. 新建 `PayloadStore`
6. 让 HTTP 单文件也先转换成 manifest 再下载

### 建议数据结构

```rust
pub struct SessionManifest {
    pub id: String,
    pub files: Vec<FileManifest>,
    pub total_size: u64,
    pub piece_size: u32,
    pub piece_count: u32,
    pub checksums: ManifestChecksums,
    pub sources: Vec<SourceDescriptor>,
}

pub struct FileManifest {
    pub path: PathBuf,
    pub length: u64,
    pub offset: u64,
}

pub struct PieceLayout {
    pub piece_index: u32,
    pub offset: u64,
    pub length: u32,
    pub hash: Option<Vec<u8>>,
}
```

### 模块设计

新增：

- `src/domain/manifest.rs`
- `src/domain/piece.rs`
- `src/payload/store.rs`
- `src/payload/file_map.rs`
- `src/payload/verifier.rs`

改造：

- `src/job/mod.rs`
- `src/job/finalize.rs`
- `src/storage/mod.rs`

### 持久化调整

需要开始区分两类存储：

- `StateStore`
  - 会话状态
  - 恢复元信息
  - piece bitmap
- `PayloadStore`
  - 实际内容写入
  - 文件映射
  - piece 提交

### 验收标准

- HTTP 单文件能被统一转换成 `SessionManifest`
- 下载进度能按 piece 或 block 聚合
- worker 不再直接决定最终文件写入位置
- 完整性校验能挂到 piece / payload 层

### 本阶段不做

- 不接入 BT peer
- 不做多源调度

## 4.5 P4 用 HTTP 先跑通 Piece 调度

### 目标

在不引入 BT 的前提下，先让新的 piece/block 调度模型在 HTTP 场景下稳定工作。

### 具体任务

1. 用 piece/block 替代当前 `chunk.rs` 的统一地位
2. 把 HTTP range 请求映射成 piece/block 获取
3. 引入基础 `Scheduler`
4. 让限速、重试、backoff 下沉到 scheduler + transfer 协作
5. 让恢复逻辑基于 piece bitmap，而不是 worker downloaded bytes

### 模块设计

新增：

- `src/scheduler/mod.rs`
- `src/scheduler/planner.rs`
- `src/scheduler/piece_picker.rs`
- `src/scheduler/bandwidth.rs`

改造：

- `src/chunk.rs`
- `src/job/workers.rs`
- `src/recovery.rs`
- `src/storage/mapping.rs`

### 验收标准

- HTTP 下载不再依赖“一个 worker = 一个固定字节区间”
- 恢复时以 piece/block 状态为真相
- 旧的 range 逻辑变成 HTTP adapter，而不是全局模型

### 本阶段不做

- 不接入 DHT / tracker
- 不实现 peer source

## 4.6 P5 接入 Torrent / Magnet 发现层

### 目标

把非 URL 型资源纳入统一发现流程，生成标准 manifest。

### 具体任务

1. 实现 torrent 元数据解析
2. 实现 magnet URI 解析
3. 实现 metadata 获取状态机
4. 初步引入 tracker 解析
5. 将 torrent / magnet 最终转换成 `SessionManifest`

### 模块设计

新增：

- `src/discovery/torrent.rs`
- `src/discovery/magnet.rs`
- `src/domain/source.rs`

### 新领域对象

```rust
pub enum SourceDescriptor {
    OriginHttp { url: String },
    OriginFtp { url: String },
    Tracker { url: String },
    DhtBootstrap { addr: SocketAddr },
    CloudAsset { asset_id: String },
}
```

### 验收标准

- 可以从 `.torrent` 文件生成 manifest
- 可以从 magnet 进入 metadata 获取流程
- 即使还没真正下载 BT 数据，也已经能进入统一会话模型

### 本阶段不做

- 不实现 peer data transfer
- 不做多源调度

## 4.7 P6 接入 BT Peer 传输与 Swarm

### 目标

让 BT 成为真正可下载的 source 类型，而不只是“解析得出来”。

### 具体任务

1. 实现 peer wire 基础协议
2. 实现 bitfield / have / request / piece 消息处理
3. 实现 peer session 管理
4. 实现 tracker announce
5. 实现基础 DHT 节点发现
6. 把 peer 传输接入 `TransferDriver`

### 模块设计

新增：

- `src/transfer/bt_peer.rs`
- `src/discovery/dht.rs`
- `src/discovery/tracker.rs`
- `src/swarm/mod.rs`
- `src/swarm/peer_state.rs`

### 验收标准

- 可以完成最小 BT 下载闭环
- piece hash 校验生效
- 多文件 torrent 能写入 payload store

### 本阶段不做

- 不做 P2SP
- 不做云端离线

## 4.8 P7 引入多源调度与 P2SP

### 目标

让同一 piece 能从多个来源竞争和协作，系统从“多协议”进入“多源平台”。

### 具体任务

1. 定义 `SourceScore`
2. 定义 `SourceHealth`
3. 实现多源 `PiecePicker`
4. 实现 `BandwidthAllocator`
5. 实现失败惩罚、冷却和恢复
6. 支持同一 manifest 同时挂 origin + peer source

### 模块设计

新增：

- `src/scheduler/source_scoring.rs`
- `src/scheduler/source_selector.rs`
- `src/scheduler/cooldown.rs`

### 建议核心结构

```rust
pub struct SourceScore {
    pub latency_ms: u64,
    pub throughput_bps: u64,
    pub error_rate: f32,
    pub availability: f32,
    pub cost_weight: f32,
}
```

### 验收标准

- 同一 piece 可以动态选择不同 source
- 慢源 / 坏源会被自动降权
- origin + peer 能协同完成同一任务

### 本阶段不做

- 不实现云端离线系统

## 4.9 P8 接入云端离线与缓存源

### 目标

把“云端已下载资产”抽象成 source，让系统具备迅雷式“离线 + 回传”能力的基础。

### 具体任务

1. 定义 `CloudSource`
2. 定义 `OfflineTaskProvider`
3. 增加离线任务状态同步
4. 增加云端资产回传驱动
5. 支持 cache hit 场景直接注入 source 列表

### 模块设计

新增：

- `src/discovery/cloud.rs`
- `src/transfer/cloud.rs`
- `src/cloud/mod.rs`

### 验收标准

- 本地会话可以挂接云端 source
- 会话层能表达“云端已完成、仅需回传”
- 调度器可以把 cloud source 与 origin / peer 统一参与评分

### 本阶段不做

- 不在本仓库内实现完整云端服务端
- 不在客户端里内嵌业务系统逻辑

## 5. 每一阶段的代码任务清单

为了方便直接开工，下面给出更细的任务清单。

## 5.1 P1 任务清单

- 新建 `src/domain/mod.rs`
- 新建 `src/domain/spec.rs`
- 新建 `src/domain/session.rs`
- 给 `download.rs` 增加新的公开类型出口
- `Manager::add_task()` 增加 `DownloadSpec` 入口
- 把旧 `TaskRequest` 改成 HTTP adapter
- 增加 `tests/spec_session_smoke.rs`

## 5.2 P2 任务清单

- 新建 `src/discovery/mod.rs`
- 新建 `src/transfer/mod.rs`
- 拆 `protocol_probe.rs`
- 定义 discovery / transfer trait
- 把 `reqwest::Client` 迁到 `transfer/http.rs`
- 增加 `tests/http_driver_integration.rs`

## 5.3 P3 任务清单

- 新建 `src/domain/manifest.rs`
- 新建 `src/domain/piece.rs`
- 新建 `src/payload/store.rs`
- 新建 `src/payload/file_map.rs`
- 新建 `src/payload/verifier.rs`
- 给 sqlite 增加 manifest / piece bitmap 表
- 增加 payload store 单元测试

## 5.4 P4 任务清单

- 新建 `src/scheduler/mod.rs`
- 用 piece planner 替换 `chunk.rs` 的中心地位
- 改造恢复逻辑
- 增加 HTTP piece 调度集成测试

## 5.5 P5 任务清单

- 新建 torrent parser
- 新建 magnet parser
- 定义 metadata acquisition state
- 增加 `.torrent -> manifest` 测试
- 增加 `magnet -> metadata` 测试

## 5.6 P6 任务清单

- 新建 BT peer session
- 新建 tracker announce
- 新建 DHT bootstrap
- 增加 piece hash 校验回归测试
- 增加最小 torrent 下载集成测试

## 5.7 P7 任务清单

- 新建 source scoring
- 新建多源 selector
- 引入 source cooldown
- 增加 origin + peer 混合下载测试
- 增加坏源降权测试

## 5.8 P8 任务清单

- 定义 cloud discovery 接口
- 定义 cloud transfer 接口
- 增加离线资产回传测试桩
- 增加 cache hit 场景测试

## 6. 推荐的迁移顺序

这里给一个更稳妥的执行顺序。

### 6.1 先建新层，不先删旧层

优先方式：

- 先建 `domain / discovery / transfer / payload / scheduler`
- 让 HTTP 路径先接新层
- 确认稳定后再裁掉旧路径

不要一开始就：

- 先删 `job/worker/storage` 再重写

### 6.2 先用 HTTP 跑通新模型

顺序必须是：

1. HTTP 在新模型里跑通
2. 再接 torrent / magnet
3. 再接 peer
4. 最后接多源调度

这是为了避免一次性改三个维度：

- 新模型
- 新协议
- 新调度

### 6.3 先单源 BT，再混合 P2SP

不要一上来就做：

- origin + peer 混合拉同一 piece

应该先做：

1. 纯 BT swarm 下载
2. BT 与 HTTP manifest 统一
3. 再引入多源调度

## 7. 关键 schema 演进建议

当前存储模型以后会不够用，建议按阶段演进。

### 7.1 新增表建议

- `sessions`
- `session_manifests`
- `manifest_files`
- `manifest_pieces`
- `piece_states`
- `sources`
- `source_states`
- `swarm_peers`

### 7.2 表职责

- `sessions`：会话级状态
- `session_manifests`：资源描述
- `manifest_files`：多文件映射
- `manifest_pieces`：piece 元信息
- `piece_states`：恢复真相
- `sources`：可用来源定义
- `source_states`：来源健康度和统计
- `swarm_peers`：BT peer 会话状态

### 7.3 迁移策略

- 增加 schema version
- 新旧 schema 并存两个阶段
- 由恢复层根据版本做转换

## 8. 推荐的测试策略

这一条必须跟着阶段走，不然会越改越虚。

### 8.1 单元测试

覆盖：

- spec 解析
- manifest 构建
- piece 计算
- source 评分
- payload 写入

### 8.2 集成测试

覆盖：

- HTTP 单文件
- HTTP 多并发 piece
- torrent metadata
- magnet metadata
- BT swarm 下载
- 多源混合下载
- 恢复重启

### 8.3 故障测试

覆盖：

- 坏 piece
- 坏 peer
- 断网恢复
- tracker 不可用
- DHT bootstrap 失败
- cloud source 失效

## 9. 风险与避坑

## 9.1 不要把 `Worker` 继续当核心领域对象

未来 `Worker` 应该只是执行单元，不是系统中心。

## 9.2 不要让 `url` 继续成为任务真相

多协议一旦起来，这会变成最大的历史包袱。

## 9.3 不要让 payload 写入继续散落在传输层

piece/block 写入必须统一进 `PayloadStore`。

## 9.4 不要过早把 P2SP 和 BT 一起做

先把 BT swarm 下载单独打稳，再上混合源。

## 9.5 不要跳过 manifest 阶段

如果没有统一 manifest，后面的多协议和多源都会变成分支地狱。

## 10. 推荐的最近三步

如果按“现在就开工”的视角，我建议最近三步是：

### 第一步

做 P1：引入 `DownloadSpec` 和 `Session`

原因：

- 这是当前模型从“URL 下载器”脱身的第一步
- 对现有代码侵入还可控

### 第二步

做 P2：建立 discovery / transfer trait

原因：

- 不先把协议边界抽出来，后面所有协议接入都会继续污染 `job/worker`

### 第三步

做 P3：引入 `SessionManifest` 和 `PayloadStore`

原因：

- 这是 BT / Magnet / 多源的地基
- 没有它，后续调度层没法统一

## 11. 最后给一句执行建议

如果你决定真的往“迅雷式下载引擎”走，这件事最正确的做法不是“现在就加 BT”，而是：

**先把引擎的世界观改掉，再把协议一个个挂上去。**

对这个项目来说，真正的起点不是 `BT`，而是：

- `DownloadSpec`
- `Session`
- `SessionManifest`
- `Piece`
- `PayloadStore`

等这几个中心对象立住了，HTTP、FTP、BT、Magnet、P2SP、离线下载都会开始变成“接入问题”；在那之前，它们都会是“重写问题”。
