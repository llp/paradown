//! HTTP 传输驱动。
//!
//! Worker 在运行循环中根据当前下载源选择本驱动，然后依次调用：
//! `build_request` → 发送请求 → `validate_response` →
//! `resolve_content_length` → `stream_response`。
//!
//! 本模块只负责一次 HTTP 传输尝试中的协议细节：
//! - 根据下载源和任务配置构造 GET 请求；
//! - 为分段下载或断点续传添加 `Range` / `If-Range`；
//! - 校验状态码和 `Content-Range`，防止把错误的字节写入目标文件；
//! - 流式读取响应体，并依次执行限速、按偏移写盘和进度上报。
//!
//! 重试、切换下载源以及 Worker 最终状态的处理位于 `worker::runtime`，不由本驱动决定。

use crate::error::Error;
use crate::protocol_probe::parse_content_range;
use crate::runtime::apply_http_request_options;
use crate::transfer::driver::TransferDriver;
use crate::worker::Worker;
use crate::worker::transfer::ProgressReporter;

// `TransferDriver` 会以 `dyn TransferDriver` 的形式动态分发。
// `async_trait` 会把异步 trait 方法转换为装箱 Future，使 HTTP/FTP 驱动能共用同一接口。
use async_trait::async_trait;

// `StreamExt` 为 reqwest 返回的响应体 Stream 提供 `.next().await`。
use futures_util::StreamExt;
use log::debug;
use reqwest::{StatusCode, header};

// 下载字节数只是独立的进度计数器，不承担其他内存状态的同步，因此使用 `Relaxed`。
use std::sync::atomic::Ordering;

/// HTTP、HTTPS、WebSeed 和 HTTP Mirror 共用的传输驱动。
///
/// 该类型本身不保存状态；每次传输所需的数据都来自 `Worker`、`Task` 和 HTTP 响应。
/// `pub(crate)` 表示它只作为 Paradown 内部实现使用，不属于公共库 API。
pub(crate) struct HttpTransferDriver;

/// 单次“申请限速额度 + 写盘”的最大切片大小。
///
/// 网络层返回的 chunk 大小并不固定。继续切成 16 KiB，可以让限速更平滑，
/// 同时缩短取消或暂停请求被再次检查前的最长处理区间。
const RATE_LIMIT_SLICE_BYTES: usize = 16 * 1024;

#[async_trait]
impl TransferDriver for HttpTransferDriver {
    /// 构造当前 Worker 的 HTTP GET 请求，但不在这里发送。
    ///
    /// `range_start` 是本次尝试希望服务器返回的第一个字节位置；首次下载通常等于
    /// `worker.start`，断点续传时则等于 `worker.start + downloaded_size`。
    /// 当 `use_range_requests` 为 `true` 时，请求范围为闭区间
    /// `[range_start, worker.end]`。
    async fn build_request(
        &self,
        worker: &Worker,
        range_start: u64,
        use_range_requests: bool,
    ) -> Result<reqwest::RequestBuilder, Error> {
        // Worker 只弱引用父 Task，以免形成 `Arc` 引用环。
        // 如果升级失败，说明任务已经释放，本次请求也就没有继续执行的上下文。
        let task = worker
            .task
            .upgrade()
            .ok_or_else(|| Error::Other(format!("Worker {} lost its parent task", worker.id)))?;

        // Worker 可能在重试期间切换镜像源，所以每次构造请求都重新取得当前源快照。
        let source = worker.current_source();

        // 来源自己的请求配置优先；没有来源级配置时，回退到 Task 的合并配置。
        // `as_ref()` 只借用 Option 内部值，不移动 `source.request` 的所有权。
        let request_options = source
            .request
            .as_ref()
            .unwrap_or(task.http_request_options());

        // 统一附加 User-Agent、Cookie、认证信息以及自定义请求头。
        let mut request = apply_http_request_options(
            worker.client.get(source.locator.as_str()),
            request_options,
        )?;

        // `Range` 的两个端点都包含在响应范围内；`range_start > worker.end` 时没有可请求字节。
        if use_range_requests && range_start <= worker.end {
            // 例如：`Range: bytes=1048576-2097151`。
            request = request.header("Range", format!("bytes={}-{}", range_start, worker.end));

            // 只有从 Worker 区间中部继续时才是断点续传。
            // `If-Range` 携带之前探测到的 ETag 或 Last-Modified：资源没变时服务器返回 206；
            // 资源已变化或验证条件不成立时，服务器通常返回完整的 200 响应，后续校验会拒绝续写。
            // `&& let` 是 let-chain：只有前一条件成立时才尝试解包验证器。
            if range_start > worker.start
                && let Some(validator) = task.resume_validator().await
            {
                request = request.header(header::IF_RANGE, validator);
            }
        }
        Ok(request)
    }

    /// 得到本次 HTTP 响应体预计包含的字节数。
    ///
    /// 优先相信响应中的 `Content-Length`。响应没有该头时：
    /// - 未知总长度的流式下载返回 `0`，完成后由 Worker 根据实际字节数确定总量；
    /// - Range 下载根据闭区间 `[range_start, worker.end]` 推算；
    /// - 非 Range 下载使用 Worker 被分配区间的完整长度。
    fn resolve_content_length(
        &self,
        worker: &Worker,
        response: &reqwest::Response,
        use_range_requests: bool,
        range_start: u64,
    ) -> u64 {
        // `unwrap_or_else` 只在响应没有可用 Content-Length 时执行兜底计算。
        response.content_length().unwrap_or_else(|| {
            // `0` 在这里表示“当前未知”，不是断言响应体为空。
            if !worker.length_known {
                return 0;
            }
            if use_range_requests {
                // Range 端点是闭区间，所以长度需要 `end - start + 1`。
                // 饱和运算可避免异常边界值触发 u64 下溢或上溢。
                worker.end.saturating_sub(range_start).saturating_add(1)
            } else {
                worker.expected_length()
            }
        })
    }

    /// 在读取响应体之前校验 HTTP 状态码和范围元数据。
    ///
    /// 严格校验的目的不是只判断“请求成功”，而是确保响应字节能安全写到当前
    /// Worker 负责的文件偏移。4xx/5xx 是否立即失败或重试由 Worker 运行时先行分类。
    fn validate_response(
        &self,
        worker: &Worker,
        response: &reqwest::Response,
        use_range_requests: bool,
        expected_start: u64,
    ) -> Result<(), Error> {
        if use_range_requests {
            // 续传位置已经越过 Worker 起点，却收到完整的 200 响应时，不能把响应体
            // 追加到已有数据后面；它可能表示 If-Range 失效，也可能是服务器忽略了 Range。
            if expected_start > worker.start && response.status() == StatusCode::OK {
                return Err(Error::ResumeInvalidated(
                    worker.id,
                    "remote resource no longer matches stored validator".into(),
                ));
            }

            // Range 请求必须得到 206。即使其他 2xx 状态看似成功，也无法证明返回了所需区间。
            if response.status() != StatusCode::PARTIAL_CONTENT {
                return Err(Error::Other(format!(
                    "Expected 206 Partial Content, got {}",
                    response.status()
                )));
            }

            // 206 必须带合法的 Content-Range，例如 `bytes 100-199/1000`。
            let content_range = response
                .headers()
                .get(header::CONTENT_RANGE)
                .ok_or_else(|| Error::Other("Missing Content-Range".into()))?
                .to_str()?;
            let content_range = parse_content_range(content_range)
                .ok_or_else(|| Error::Other("Invalid Content-Range".into()))?;

            // 同时比较起止位置，防止服务器返回错位、过短或过长的数据并污染目标文件。
            if content_range.start != expected_start || content_range.end != worker.end {
                return Err(Error::Other(format!(
                    "Unexpected Content-Range {}-{} for expected {}-{}",
                    content_range.start, content_range.end, expected_start, worker.end
                )));
            }

            return Ok(());
        }

        // 未发送 Range 时只接受完整的 200 响应。
        if response.status() != StatusCode::OK {
            return Err(Error::Other(format!(
                "Expected 200 OK, got {}",
                response.status()
            )));
        }

        Ok(())
    }

    /// 消费 HTTP 响应体，并把字节流写入 PayloadStore 中的正确偏移。
    ///
    /// `downloaded_size` 是当前 Worker 已完成的相对字节数；`write_offset` 是整个 payload
    /// 中的绝对偏移。每个成功写入的切片都会依次更新二者、统计信息和进度事件。
    async fn stream_response(
        &self,
        worker: &Worker,
        response: reqwest::Response,
        downloaded_size: &mut u64,
        reporter: &mut ProgressReporter,
        use_range_requests: bool,
        range_start: u64,
    ) -> Result<(), Error> {
        // 写盘依赖 Task 持有的 PayloadStore；父任务消失后不能继续写入。
        let task = worker
            .task
            .upgrade()
            .ok_or_else(|| Error::Other(format!("Worker {} lost its parent task", worker.id)))?;

        // PayloadStore 负责把全局 payload 偏移映射到实际文件，并处理跨文件边界写入。
        let payload_store = task.payload_store().await?;

        // Range 响应从本次请求的 `range_start` 开始；完整响应则从 Worker 区间起点开始。
        let mut write_offset = if use_range_requests {
            range_start
        } else {
            worker.start
        };

        debug!(
            "[Worker {}] Writing locator {} at payload offset {}",
            worker.id,
            worker.current_source().locator.as_str(),
            write_offset
        );

        // `bytes_stream()` 不会一次性把整个响应加载进内存，而是按网络到达顺序产出 Bytes。
        let mut stream = response.bytes_stream();

        // `None` 表示响应体正常结束；`Some(Err(_))` 会在下面转换为带 Worker id 的网络错误。
        while let Some(chunk) = stream.next().await {
            // 取消和删除被视为受控停止，因此直接返回 Ok；Worker 运行时随后检查停止标志。
            if worker.should_stop_gracefully() {
                return Ok(());
            }

            // 暂停时在这里异步等待；等待期间如果任务被取消或删除，则返回 false。
            if !worker.wait_until_resumed().await {
                return Ok(());
            }

            // 保留 Worker id，便于上层重试日志定位是哪个并发区间发生了网络错误。
            let chunk = chunk.map_err(|err| Error::NetworkError(worker.id, err.to_string()))?;

            // reqwest 的 chunk 可能很大；细分后可提高限速以及暂停/取消检查的粒度。
            for slice in chunk.chunks(RATE_LIMIT_SLICE_BYTES) {
                // chunk 内也要再次检查，否则处理一个大 chunk 时无法及时响应控制命令。
                if worker.should_stop_gracefully() {
                    return Ok(());
                }

                if !worker.wait_until_resumed().await {
                    return Ok(());
                }

                let slice_len = slice.len() as u64;

                // 先申请额度再写盘，避免已经落盘的数据绕过全局下载速率限制。
                worker.acquire_rate_limit(slice_len).await;

                // 按绝对偏移写入，使多个 Worker 能各自写入互不重叠的文件区间。
                payload_store.write_at(write_offset, slice).await?;

                // 只有写盘成功后才推进偏移和进度，防止把未落盘数据计为已完成。
                write_offset = write_offset.saturating_add(slice_len);
                *downloaded_size += slice_len;

                // Stats 记录吞吐量等聚合指标；AtomicU64 提供可被其他任务快速读取的进度快照。
                worker.stats.update_worker(worker.id, slice_len).await;

                // 此计数器不用于发布其他内存状态，`Relaxed` 已满足原子读写需求。
                worker
                    .downloaded_size
                    .store(*downloaded_size, Ordering::Relaxed);

                // Reporter 会根据字节阈值和时间间隔节流，避免每个 16 KiB 都广播事件。
                reporter.maybe_emit(worker, *downloaded_size).await;
            }
        }

        // 响应体结束只代表网络流已读完；长度是否符合预期由 Worker 运行时随后统一校验。
        Ok(())
    }
}
