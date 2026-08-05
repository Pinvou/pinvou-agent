//! 流式下载校验 helper:下载到 `.part` → 逐块取消/进度 → sha256 校验 → 原子 rename。
//!
//! 收敛 voice/knowledge 等异步下载路径共享的「下载 → 校验 → 提升」骨架。三调用方
//! 取消机制不同(voice 的 AtomicBool、native_installer 无取消、knowledge 的
//! AtomicBool + 进度事件),因此取消谓词与进度回调一律由调用方以闭包注入,
//! helper 不假定统一取消源。
//!
//! **设计取舍**:本函数为 `async fn`(reqwest 异步流式)。`features/connectors/
//! native_installer.rs::download_verified` 是 `reqwest::blocking` 的同步函数,且始终在
//! `tokio::task::spawn_blocking` 内被调用(飞书/钉钉/企微 ensure_cli),该线程上没有
//! 可 `.await` 的异步运行时;强行 `block_on` 嵌套运行时会与外层 tauri runtime 冲突,
//! 属于 force-fit。故 native_installer 保持独立同步实现(见 task 报告差异表),仅迁移
//! 同构的 voice/knowledge 两路。
//!
//! 复用 [`crate::platform::hashing::sha256_file`] 做校验,不重复实现 sha256。

use std::path::Path;

/// 单次下载请求(下载到 `part` → 校验 → rename 到 `dest`)。
///
/// `is_cancelled` 在下载**开始前**与**每收到一个 chunk 后**被调用,返回 `true` 立即中止、
/// 删除 `.part`。`on_progress(downloaded, total)` 在每次 chunk 后被调用(调用方自行节流,
/// 与原有「每 1~2 MiB 或到达 total 才 emit」行为一致)。二者均为 `Send` 闭包,由调用方
/// 填入具体 AtomicBool 读取 / Tauri emit 逻辑。
pub struct DownloadRequest<'a> {
    /// 下载源 URL。
    pub url: &'a str,
    /// 最终目标路径(校验通过后 `rename` 的目的地)。
    pub dest: &'a Path,
    /// 临时 `.part` 路径(流式落盘位置,失败时删除)。
    pub part: &'a Path,
    /// 期望的 sha256(小写十六进制);空串表示跳过校验(dev 本地包常用)。
    pub expected_sha256: &'a str,
    /// 内容长度**硬上限**(字节数)。`Content-Length` 超过此值即拒绝;流式累计超过即中止。
    /// 设为 `u64::MAX` 等价于不设硬上限。仅用于挡离谱大文件,与进度估算无关。
    pub max_bytes: u64,
    /// 进度 total 的**回退估算值**:仅当响应缺 `Content-Length`(或为 0)时,
    /// 才用此值作为 `on_progress` 的 `total` 参数。二者刻意拆开——voice 把
    /// `expected_size` 当估算、把 `2*expected_size` 当上限;knowledge 把
    /// `MODEL_TARGZ_SIZE` 当估算、把 `2*MODEL_TARGZ_SIZE` 当上限——避免复用单字段
    /// 导致进度停在约 50% 或合法镜像被拒。
    pub total_hint: u64,
    /// 取消谓词:返回 `true` 时中止下载并清理 `.part`。
    /// 需 `Sync` 以便在 `tokio::select!` 中与网络 future 并发轮询(恢复重构前
    /// voice 的 `tokio::select!{ send(), wait_for_cancel() }` 响应性)。
    pub is_cancelled: Box<dyn Fn() -> bool + Send + Sync + 'a>,
    /// 可选 `User-Agent`。重构前 voice/knowledge 各自设置 `pinvou3-asr/1.0`、
    /// `pinvou3-kb/1.0`;helper 通过该字段透传,`None` 时不设置。
    pub user_agent: Option<&'a str>,
    /// 进度回调 `(downloaded, total)`。`total` 为 `Content-Length`(缺省/为 0 时回退到
    /// [`DownloadRequest::total_hint`])。
    pub on_progress: Box<dyn Fn(u64, u64) + Send + 'a>,
    /// 下载流完成(`sync_all` 后)、sha256 校验**开始前**触发的一次性回调。
    /// 用于在原始时点(下载完成 → verify 事件 → 校验)emit `verify` 进度事件,恢复
    /// 「下载 0–95% → verify → done」的 frontend 事件顺序。`None` = 不触发。
    ///
    /// 必须为 `'static`:helper 把刷盘/校验/提升整体挪进 `spawn_blocking`,此回调
    /// 在阻塞任务内触发;调用方需捕获 owned 数据(如克隆的 `AppHandle`),不借用 `req`
    /// 的生命周期参数。voice/knowledge 两处实现均已是 `'static`。
    pub on_pre_verify: Option<Box<dyn FnOnce() + Send + 'static>>,
}

/// `.part` 清理守卫:覆盖所有非成功退出路径(取消 / 网络错误 / 超长 / sha256 不匹配 /
/// 目录创建失败 / rename 失败)的 `.part` 删除。成功路径(`rename` 已消费 `.part`)调用
/// [`PartGuard::disarm`] 解除,Drop 时不再删除。
///
/// 必须声明在使用 `.part` 的 `File` 句柄**之前**,以保证 Drop 顺序为 file 先于 guard
/// (Windows 上文件句柄未关闭会导致 `remove_file` 失败)。
struct PartGuard<'a> {
    part: &'a Path,
    armed: bool,
}

impl<'a> PartGuard<'a> {
    fn new(part: &'a Path) -> Self {
        Self { part, armed: true }
    }
    /// 标记成功:`.part` 已被 `rename` 消费为 `dest`,Drop 时跳过删除。
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl<'a> Drop for PartGuard<'a> {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(self.part);
        }
    }
}

/// 下载到 `.part` → 校验 sha256 → 原子 `rename` 到 `dest`。
///
/// 失败语义:取消 / 网络错误 / 超长 / sha256 不匹配 / 目录创建失败 / rename 失败 时,
/// 由 [`PartGuard`] 删除 `.part` 后返回 `Err`。成功时 `dest` 就绪、`.part` 已被 `rename`
/// 消费(守卫已 `disarm`,不会误删)。
///
/// 事件顺序:`on_progress`(下载中,0–95%)→ `on_pre_verify`(下载完成、校验开始前,
/// 用于 emit `verify`)→ 校验 → rename → done。该顺序恢复重构前的 frontend 事件时序。
/// 取消轮询间隔,与重构前 voice 的 `ASR_CANCEL_POLL_INTERVAL` 一致(50ms)。
///
/// 重构前的 voice 下载用 `tokio::select!` 把 `client.get(url).send()` 与每个
/// `resp.chunk()` 都包在 `wait_for_asr_cancel()` 轮询 future 上,这样取消标志翻转后
/// 即便服务器中途停顿(无 chunk 到达)、或连接阶段卡住,也能在一个轮询周期内中止。
/// helper 以闭包承载各调用方的取消源,这里在 select 分支内轮询该闭包恢复响应性。
const CANCEL_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

pub(crate) async fn download_to_part_with_verify(
    mut req: DownloadRequest<'_>,
) -> Result<(), String> {
    // 下载开始前先查一次取消(与 voice 原循环语义一致)。
    if (req.is_cancelled)() {
        let _ = std::fs::remove_file(req.part);
        return Err("已取消".to_string());
    }

    let client = {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::default());
        if let Some(ua) = req.user_agent {
            builder = builder.user_agent(ua);
        }
        builder
            .build()
            .map_err(|e| format!("创建下载客户端失败: {e}"))?
    };
    let response = {
        // 与重构前 voice 一致:连接阶段也用 select! 竞速取消轮询,
        // 避免服务器响应慢时无法及时中止。轮询闭包每个 poll 周期重新借用 req,
        // 不跨 await 持有,故对非 Sync 的取消源也安全。
        let send_fut = client.get(req.url).send();
        tokio::pin!(send_fut);
        let result = tokio::select! {
            result = &mut send_fut => result,
            _ = async {
                loop {
                    if (req.is_cancelled)() {
                        return;
                    }
                    tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
                }
            } => {
                let _ = std::fs::remove_file(req.part);
                return Err("已取消".to_string());
            }
        };
        result
            .map_err(|e| format!("连接下载源失败: {e}"))?
            .error_for_status()
            .map_err(|e| format!("下载源响应异常: {e}"))?
    };

    // 进度 total 与硬上限解耦:
    // - `total`(进度估算)= 真实 `Content-Length`,缺省/为 0 时回退 `total_hint`;
    // - `max_bytes`(硬上限)只用于挡离谱大文件。二者复用同一字段会把 voice 进度停在
    //   约 50%(expected_size*2 当 total)、或让 knowledge 的合法镜像(略大于
    //   MODEL_TARGZ_SIZE)被拒。
    let total = response
        .content_length()
        .filter(|n| *n > 0)
        .unwrap_or(req.total_hint);
    if total > req.max_bytes {
        let _ = std::fs::remove_file(req.part);
        return Err(format!("下载内容超过 {} 字节上限", req.max_bytes));
    }

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    // 清理守卫:从此处起任何 `Err` 退出都会删除 `.part`。声明在 `file` 之前,使 Drop
    // 顺序为 file 先于 guard(Windows 文件句柄未关闭时 remove 会失败)。
    let mut guard = PartGuard::new(req.part);
    let mut file =
        std::fs::File::create(req.part).map_err(|e| format!("创建下载暂存文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    loop {
        // 与重构前 voice 一致:每个 chunk 的到达也用 select! 竞速取消轮询,
        // 这样服务器中途停顿(无 chunk 到达)时仍能在一个轮询周期内中止。
        let chunk = {
            let next_fut = stream.next();
            tokio::pin!(next_fut);
            tokio::select! {
                item = &mut next_fut => item,
                _ = async {
                    loop {
                        if (req.is_cancelled)() {
                            return;
                        }
                        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
                    }
                } => {
                    return Err("已取消".to_string());
                }
            }
        };
        let Some(chunk) = chunk.transpose().map_err(|e| format!("下载中断: {e}"))? else {
            break;
        };
        // 写盘后查取消(与原实现一致:已写的不丢,但停止接收后续 chunk)。
        use std::io::Write;
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > req.max_bytes {
            return Err(format!("下载内容超过 {} 字节上限", req.max_bytes));
        }
        (req.on_progress)(downloaded, total);
        if (req.is_cancelled)() {
            return Err("已取消".to_string());
        }
    }
    // 刷盘、sha256 校验、原子 rename 都是**同步**文件 I/O:原 knowledge 流程把它们
    // 一起放在 `spawn_blocking` 里(对约 389MB 的模型包做 sha256 会长时间占满 worker)。
    // 这里把文件句柄的 sync + 校验 + 提升整体挪进 `spawn_blocking`,避免在 async
    // runtime 上做阻塞 I/O(影响其他命令与事件处理)。`on_pre_verify` 是 `Send` 的
    // `FnOnce`,可安全带进阻塞任务,在 sync 后、SHA 前触发,恢复重构前事件时序。
    drop(file);
    let part = req.part.to_path_buf();
    let dest = req.dest.to_path_buf();
    let expected_sha256 = req.expected_sha256.to_string();
    let on_pre_verify = req.on_pre_verify.take();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        // 重新打开句柄做 sync_all(create 后未 sync 的句柄已 Drop)。
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&part)
            .map_err(|e| format!("同步下载文件失败: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("同步下载文件失败: {e}"))?;
        drop(file);

        // 下载流已完成、校验即将开始:触发调用方的 `verify` 进度事件,恢复重构前的
        // frontend 事件顺序(下载 0–95% → verify → 校验 → done)。
        if let Some(on_pre_verify) = on_pre_verify {
            on_pre_verify();
        }

        // sha256 校验:空串跳过(与 knowledge/voice 的 dev 兜底一致)。
        if !expected_sha256.trim().is_empty() {
            let got = crate::platform::hashing::sha256_file(&part)
                .map_err(|e| format!("读取下载文件失败: {e}"))?;
            if !got.eq_ignore_ascii_case(&expected_sha256) {
                return Err(format!(
                    "下载校验失败(expected {}, got {})",
                    expected_sha256, got
                ));
            }
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
        }
        if dest.exists() {
            let _ = std::fs::remove_file(&dest);
        }
        std::fs::rename(&part, &dest).map_err(|e| format!("保存下载文件失败: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("下载校验任务异常: {e}"))?;

    // 校验后再查一次取消(rename 是不可逆的最后一步)。
    if (req.is_cancelled)() {
        return Err("已取消".to_string());
    }

    result?;
    // rename 成功:`.part` 已被消费为 `dest`,解除守卫,避免 Drop 时误删。
    guard.disarm();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// "hello world" 的 sha256。
    const HELLO_SHA: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    /// "abc" 的 sha256。
    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn scratch_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pinvou3_download_test_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// 用 `std::net::TcpListener` 起一个最小 HTTP/1.1 fixture(不触网、不新增 mockito):
    /// 接受一次连接,返回固定 body,然后退出。
    fn serve_once(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let url = format!("http://{addr}/payload");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // 读请求行/头(忽略内容,读到空行或阻塞即可)。
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (url, handle)
    }

    /// 起 HTTP/1.1 fixture,但声明的 `Content-Length` 大于实际发送字节数后立即断连,
    /// 使 reqwest 流式读取中途报错(下载中断)。用于验证 `.part` 在网络中途错误时被清理
    /// (Finding 2 的 Drop 守卫契约)。
    fn serve_truncated(
        declared_len: usize,
        send_bytes: usize,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let url = format!("http://{addr}/payload");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    declared_len
                );
                let _ = stream.write_all(header.as_bytes());
                let body = vec![0u8; send_bytes];
                let _ = stream.write_all(&body);
                let _ = stream.flush();
                // 故意不发完声明的字节数就关闭连接 → hyper 报 IncompleteMessage。
            }
        });
        (url, handle)
    }

    /// 起 HTTP/1.1 fixture 但**不声明 `Content-Length`**,改用 `Connection: close`
    /// 让 body 长度由连接结束隐含。用于验证缺 `Content-Length` 时进度 total 回退到
    /// `total_hint`(而非 `max_bytes`,否则进度会停在约 50%)。
    fn serve_without_content_length(body: Vec<u8>) -> (String, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let url = format!("http://{addr}/payload");
        let handle = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = std::io::Read::read(&mut stream, &mut buf);
                let header =
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (url, handle)
    }

    fn never_cancel<'a>() -> Box<dyn Fn() -> bool + Send + Sync + 'a> {
        Box::new(|| false)
    }
    fn noop_progress<'a>() -> Box<dyn Fn(u64, u64) + Send + 'a> {
        Box::new(|_, _| {})
    }

    #[tokio::test]
    async fn happy_path_downloads_verifies_and_renames() {
        let dir = scratch_dir("happy");
        let body = b"hello world".to_vec();
        let (url, handle) = serve_once(body.clone());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: HELLO_SHA,
            max_bytes: 1024,
            total_hint: 1024,
            is_cancelled: never_cancel(),
            user_agent: None,
            on_progress: noop_progress(),
            on_pre_verify: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_ok(), "err = {:?}", result.err());
        assert!(dest.exists(), "dest should exist after rename");
        assert!(!part.exists(), "part should be consumed");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn sha256_mismatch_returns_err_and_cleans_part() {
        let dir = scratch_dir("mismatch");
        let (url, handle) = serve_once(b"abc".to_vec());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: HELLO_SHA, // 故意不匹配 "abc"
            max_bytes: 1024,
            total_hint: 1024,
            is_cancelled: never_cancel(),
            user_agent: None,
            on_progress: noop_progress(),
            on_pre_verify: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_err());
        assert!(!part.exists(), "part should be cleaned on mismatch");
        assert!(!dest.exists(), "dest should not exist on mismatch");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_sha256_skips_verification() {
        let dir = scratch_dir("skip");
        let body = b"abc".to_vec();
        let (url, handle) = serve_once(body.clone());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: "", // 跳过校验(dev 兜底)
            max_bytes: 1024,
            total_hint: 1024,
            is_cancelled: never_cancel(),
            user_agent: None,
            on_progress: noop_progress(),
            on_pre_verify: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_ok(), "err = {:?}", result.err());
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancellation_mid_download_aborts_and_cleans_part() {
        let dir = scratch_dir("cancel");
        // 构造一个足够大的 body,使 chunk 循环能跑到取消点。
        let body = vec![0u8; 64 * 1024];
        let (url, handle) = serve_once(body.clone());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let flag = Arc::new(AtomicBool::new(false));
        // 进度回调:第一次推进后置位取消。
        let cancel = Arc::clone(&flag);
        let on_progress: Box<dyn Fn(u64, u64) + Send> = Box::new(move |_d, _t| {
            cancel.store(true, Ordering::SeqCst);
        });
        let is_cancelled: Box<dyn Fn() -> bool + Send + Sync> = {
            let f = Arc::clone(&flag);
            Box::new(move || f.load(Ordering::SeqCst))
        };

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: "",
            max_bytes: 10 * 1024 * 1024,
            total_hint: 10 * 1024 * 1024,
            is_cancelled,
            user_agent: None,
            on_progress,
            on_pre_verify: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "已取消");
        assert!(!part.exists(), "part should be cleaned on cancel");
        assert!(!dest.exists(), "dest should not exist on cancel");
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn content_length_over_max_bytes_is_rejected() {
        let dir = scratch_dir("toolong");
        let body = vec![0u8; 2048];
        let (url, handle) = serve_once(body.clone());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: ABC_SHA,
            max_bytes: 1024, // body 2048 > 上限 → Content-Length 即拒
            total_hint: 1024,
            is_cancelled: never_cancel(),
            user_agent: None,
            on_progress: noop_progress(),
            on_pre_verify: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("上限"),
            "should mention size cap"
        );
        assert!(!part.exists());
        assert!(!dest.exists());
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn network_error_mid_download_cleans_part() {
        let dir = scratch_dir("neterr");
        // 声明 8 KiB 但只发 1 KiB 就断连 → reqwest 流式读取中途报错(下载中断)。
        let (url, handle) = serve_truncated(8 * 1024, 1024);
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: "",
            max_bytes: 1024 * 1024,
            total_hint: 1024 * 1024,
            is_cancelled: never_cancel(),
            user_agent: None,
            on_progress: noop_progress(),
            on_pre_verify: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_err(), "err = {:?}", result.err());
        assert!(
            !part.exists(),
            "part must be cleaned on mid-download network error (Finding 2)"
        );
        assert!(!dest.exists());
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn on_pre_verify_fires_after_download_before_rename() {
        // Finding 1:`verify` 必须在下载完成后、校验/rename 前触发(恢复重构前时序)。
        // 用共享 Vec 记录事件顺序,断言 verify 出现在最后一条 download 进度之后、
        // 且 helper 返回 Ok(rename 成功)。
        let dir = scratch_dir("preverify");
        let body = vec![0u8; 64 * 1024];
        let (url, handle) = serve_once(body.clone());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        let events = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let total_hint = body.len() as u64;
        let ev_progress = Arc::clone(&events);
        let on_progress: Box<dyn Fn(u64, u64) + Send> = Box::new(move |d, t| {
            if d >= t || d >= total_hint {
                ev_progress.lock().unwrap().push(format!("progress:{d}"));
            }
        });
        let ev_verify = Arc::clone(&events);
        let on_pre_verify: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            ev_verify.lock().unwrap().push("verify".to_string());
        }));

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: "",
            max_bytes: 1024 * 1024,
            total_hint: 1024 * 1024,
            is_cancelled: never_cancel(),
            user_agent: None,
            on_progress,
            on_pre_verify,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_ok(), "err = {:?}", result.err());
        let events = events.lock().unwrap();
        let last_progress = events.iter().rposition(|e| e.starts_with("progress"));
        let verify_idx = events.iter().position(|e| e == "verify");
        assert!(
            last_progress.is_some(),
            "should have emitted download progress"
        );
        assert!(
            verify_idx.is_some_and(|v| v > last_progress.unwrap()),
            "verify must fire AFTER last download progress, got events = {:?}",
            *events
        );
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 缺 `Content-Length` 时,进度 total 必须回退到 `total_hint`(而非 `max_bytes`)。
    /// 此前复用单字段会让进度停在约 50%(voice 的 max_bytes=2*expected_size)。
    #[tokio::test]
    async fn missing_content_length_falls_back_to_total_hint() {
        let dir = scratch_dir("hint");
        let body = vec![0u8; 32 * 1024];
        let (url, handle) = serve_without_content_length(body.clone());
        let part = dir.join("payload.part");
        let dest = dir.join("payload");
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(&dest);

        // 故意让 max_bytes 远大于 total_hint:若 helper 错把 max_bytes 当 total,
        // 进度回调收到的 t 会是 1 GiB;正确实现应回退到 total_hint=32 KiB。
        let seen_total = Arc::new(std::sync::Mutex::new(0u64));
        let seen_total_clone = Arc::clone(&seen_total);
        let on_progress: Box<dyn Fn(u64, u64) + Send> = Box::new(move |_d, t| {
            let mut g = seen_total_clone.lock().unwrap();
            *g = (*g).max(t);
        });

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: "",
            max_bytes: 1024 * 1024 * 1024,
            total_hint: 32 * 1024,
            is_cancelled: never_cancel(),
            on_progress,
            on_pre_verify: None,
            user_agent: None,
        };
        let result = download_to_part_with_verify(req).await;

        assert!(result.is_ok(), "err = {:?}", result.err());
        assert_eq!(
            *seen_total.lock().unwrap(),
            32 * 1024,
            "missing Content-Length should fall back to total_hint, not max_bytes"
        );
        handle.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
