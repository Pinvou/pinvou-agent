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
    /// 内容长度上限(字节数)。`Content-Length` 超过此值即拒绝;流式累计超过即中止。
    /// 设为 `u64::MAX` 等价于不设硬上限。
    pub max_bytes: u64,
    /// 取消谓词:返回 `true` 时中止下载并清理 `.part`。
    pub is_cancelled: Box<dyn Fn() -> bool + Send + 'a>,
    /// 进度回调 `(downloaded, total)`。`total` 为 `Content-Length`(缺省/为 0 时由调用方
    /// 在构造请求时填入估算值)。
    pub on_progress: Box<dyn Fn(u64, u64) + Send + 'a>,
}

/// 下载到 `.part` → 校验 sha256 → 原子 `rename` 到 `dest`。
///
/// 失败语义:取消 / 网络错误 / 超长 / sha256 不匹配 时删除 `.part` 后返回 `Err`。
/// 成功时 `dest` 就绪、`.part` 已被 `rename` 消费。
pub(crate) async fn download_to_part_with_verify(req: DownloadRequest<'_>) -> Result<(), String> {
    // 下载开始前先查一次取消(与 voice 原循环语义一致)。
    if (req.is_cancelled)() {
        let _ = std::fs::remove_file(req.part);
        return Err("已取消".to_string());
    }

    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::default())
        .build()
        .map_err(|e| format!("创建下载客户端失败: {e}"))?;
    let response = client
        .get(req.url)
        .send()
        .await
        .map_err(|e| format!("连接下载源失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载源响应异常: {e}"))?;

    // 上限以 max_bytes 为准:Content-Length 申报超限直接拒。
    let total = response
        .content_length()
        .filter(|n| *n > 0)
        .unwrap_or(req.max_bytes);
    if total > req.max_bytes {
        let _ = std::fs::remove_file(req.part);
        return Err(format!("下载内容超过 {} 字节上限", req.max_bytes));
    }

    use futures_util::StreamExt;
    let mut stream = response.bytes_stream();
    let mut file =
        std::fs::File::create(req.part).map_err(|e| format!("创建下载暂存文件失败: {e}"))?;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .map_err(|e| format!("下载中断: {e}"))?
    {
        // 写盘后查取消(与原实现一致:已写的不丢,但停止接收后续 chunk)。
        use std::io::Write;
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > req.max_bytes {
            drop(file);
            let _ = std::fs::remove_file(req.part);
            return Err(format!("下载内容超过 {} 字节上限", req.max_bytes));
        }
        (req.on_progress)(downloaded, total);
        if (req.is_cancelled)() {
            drop(file);
            let _ = std::fs::remove_file(req.part);
            return Err("已取消".to_string());
        }
    }
    // 刷盘后再做校验(rename 前确保数据落盘)。
    file.sync_all()
        .map_err(|e| format!("同步下载文件失败: {e}"))?;
    drop(file);

    // sha256 校验:空串跳过(与 knowledge/voice 的 dev 兜底一致)。
    if !req.expected_sha256.trim().is_empty() {
        let got = crate::platform::hashing::sha256_file(req.part)
            .map_err(|e| format!("读取下载文件失败: {e}"))?;
        if !got.eq_ignore_ascii_case(req.expected_sha256) {
            let _ = std::fs::remove_file(req.part);
            return Err(format!(
                "下载校验失败(expected {}, got {})",
                req.expected_sha256, got
            ));
        }
    }

    // 校验后再查一次取消(rename 是不可逆的最后一步)。
    if (req.is_cancelled)() {
        let _ = std::fs::remove_file(req.part);
        return Err("已取消".to_string());
    }

    if let Some(parent) = req.dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
    }
    if req.dest.exists() {
        let _ = std::fs::remove_file(req.dest);
    }
    std::fs::rename(req.part, req.dest).map_err(|e| format!("保存下载文件失败: {e}"))?;
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

    fn never_cancel<'a>() -> Box<dyn Fn() -> bool + Send + 'a> {
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
            is_cancelled: never_cancel(),
            on_progress: noop_progress(),
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
            is_cancelled: never_cancel(),
            on_progress: noop_progress(),
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
            is_cancelled: never_cancel(),
            on_progress: noop_progress(),
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
        let is_cancelled: Box<dyn Fn() -> bool + Send> = {
            let f = Arc::clone(&flag);
            Box::new(move || f.load(Ordering::SeqCst))
        };

        let req = DownloadRequest {
            url: &url,
            dest: &dest,
            part: &part,
            expected_sha256: "",
            max_bytes: 10 * 1024 * 1024,
            is_cancelled,
            on_progress,
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
            is_cancelled: never_cancel(),
            on_progress: noop_progress(),
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
}
