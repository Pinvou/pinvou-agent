//! macOS OTA:latest.json 多平台清单 → dmg 流式下载(sha256 校验)→
//! hdiutil attach + PlistBuddy 校验 CFBundleIdentifier + cp -R 到 /Applications →
//! 安装完成返回 Ok(false)(进程不退出),前端自动调 restart_app 切到新版。
//!
//! 设计与 linux_update.rs 对齐:
//! - 更新源同是静态 HTTP latest.json,新增可选 `platforms` map(mac/linux/win 各一项)。
//! - 下载目录用 `crate::bridge::paths::updates_dir()`(~/.pinvou3/updates/),跨平台一致。
//! - dmg 安装不需要 pkexec(/Applications 通常当前用户可写);首次拖拽权限由 Finder 引导。
//! - 安装后自动重启:app.restart() 按路径 exec,bundle 被替换后该路径已指向新文件
//!   (inode 语义与 Linux 同,spawn 新进程即加载新版)。返回 Ok(false) 表示进程未退出,
//!   由前端调 restart_app——与 Linux 同型,区别于 Windows MSI(Ok(true)→app.exit)。
//!
//! 与 linux_update.rs 的差异:
//! - 包格式 dmg(非 deb),校验只看 sha256(非 MD5)。
//! - 安装路径固定 /Applications(非 dpkg 路径)。
//! - 磁盘空间检查用 `df -k`(BSD),非 GNU df 的 --output=avail -B1。

use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::time::timeout;

use crate::bridge::paths;

const UPDATE_MANIFEST_URL: &str = "https://pinvou.com/pinvou3/latest.json";

/// pinvou3.app 的 CFBundleIdentifier。安装前 PlistBuddy 读出来比对,防 dmg 伪装。
const EXPECTED_BUNDLE_ID: &str = "com.pinvou.pinvou3";

fn manifest_url() -> String {
    std::env::var("PINVOU3_UPDATE_URL").unwrap_or_else(|_| UPDATE_MANIFEST_URL.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LatestManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub pub_date: String,
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
    /// 多平台清单(可选):`{ "macos-arm64": PlatformAsset, "linux-arm64": ..., ... }`。
    /// 旧版 latest.json 没这字段 → 空 map → 回退到顶层 url/sha256/size。
    #[serde(default)]
    pub platforms: std::collections::HashMap<String, crate::updater::PlatformAsset>,
}

pub fn check_update_platform_support() -> Result<(), String> {
    Ok(())
}

/// 清理上次 OTA 升级残留的旧 app 备份(`/Applications/.pinvou3.app.old`)。
///
/// 安装时旧 .app 被改名备份(防回滚),清理依赖"下次安装开头"。但如果用户只升级
/// 一次后长期不更新,这份备份(整个旧 app bundle,可达数百 MB)会永久驻留磁盘。
/// 本函数在应用**正常启动时**调用(此时旧进程必然已退出,inode 安全释放),清掉残留。
pub fn cleanup_stale_backup() {
    // 只清理我们自己的备份(固定路径),不会误删用户其它文件。
    let backup = "/Applications/.pinvou3.app.old";
    if Path::new(backup).exists() {
        let _ = std::fs::remove_dir_all(backup);
    }
}

/// 从 manifest 解析本平台下载资产。优先 platforms[build_platform_key()](需 url 与
/// sha256 均非空),缺失或残缺则回退顶层 url/sha256/size(向后兼容旧 manifest,并防
/// 止发布侧一处笔误——如漏填 sha256——导致整平台升级静默瘫痪)。抽出为纯函数便于测试。
///
/// 返回 (url, sha256, size, version, notes)。version 是所选资产的版本号:命中本平台资产
/// 时取 platform.version(为空则退顶层 version);回退顶层时即顶层 version。调用方据此判
/// is_newer,避免「Mac 先发新版、Linux 顶层还停在旧版」时 Mac 客户端读顶层版本看不到更新。
/// notes 同理:命中平台资产取 platform.notes(为空退顶层),确保 macOS 用户看到的是本平台
/// 的更新说明,而非上一次 Linux 发版的日志。
fn resolve_asset(m: &LatestManifest) -> (String, String, u64, String, String) {
    let key = crate::updater::build_platform_key();
    match m.platforms.get(&key) {
        Some(a) if !a.url.is_empty() && !a.sha256.is_empty() => {
            let ver = if a.version.is_empty() {
                m.version.clone()
            } else {
                a.version.clone()
            };
            let notes = if a.notes.is_empty() {
                m.notes.clone()
            } else {
                a.notes.clone()
            };
            (a.url.clone(), a.sha256.clone(), a.size, ver, notes)
        }
        _ => (m.url.clone(), m.sha256.clone(), m.size, m.version.clone(), m.notes.clone()),
    }
}

pub async fn check_for_update_info(
    client: &reqwest::Client,
    current_version: &str,
) -> Result<crate::updater::UpdateInfo, String> {
    let m: LatestManifest = client
        .get(manifest_url())
        .send()
        .await
        .map_err(|e| format!("更新源连接失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("更新源响应异常: {e}"))?
        .json()
        .await
        .map_err(|e| format!("latest.json 解析失败: {e}"))?;

    // 优先用本平台的 platforms[key];缺失或关键字段残缺(url/sha256 为空)则回退到
    // 顶层(向后兼容旧 manifest,并防止发布侧一处笔误——如漏填 sha256——导致整平台
    // 升级静默瘫痪)。逻辑见 resolve_asset(纯函数,有单测)。
    // 版本判等也用所选资产的 version:Mac 发版不 bump 顶层 .version(顶层代表最近一次
    // Linux 发版),Mac 客户端必须读本平台 version 才看得到自己的新版。
    let (url, sha256, size, latest_version, notes) = resolve_asset(&m);

    // 回退顶层时防下载错误格式:顶层 url 代表最近一次 Linux 发版(.deb),macOS 客户端
    // 下载 .deb 存为 .dmg 会让 hdiutil attach 必败且报错与根因无关。如果回退到的顶层
    // url 不以 .dmg 结尾,说明远端尚未发布 macOS 版本 → 标记 available=false,不下载。
    let available = is_newer(&latest_version, current_version) && url.ends_with(".dmg");

    Ok(crate::updater::UpdateInfo {
        available,
        current_version: current_version.to_string(),
        latest_version,
        notes,
        pub_date: m.pub_date,
        url,
        sha256: sha256.to_lowercase(),
        size,
        package_md5: String::new(),
        software_id: String::new(),
        sn: String::new(),
        update_type: String::new(),
        platform: "macos".to_string(),
        ota_host: String::new(),
        platforms: m.platforms,
    })
}

pub async fn download_update_package(
    info: &crate::updater::UpdateInfo,
    app: AppHandle,
    cancel: &AtomicBool,
    stall_timeout: Duration,
) -> Result<crate::updater::DownloadUpdateResult, String> {
    check_update_platform_support()?;
    // 防御性硬上限:即使清单 size 被篡改为 0(跳过预检)或实际投递量远超声明值,
    // 也强制中断,避免无限写入撑爆磁盘。dmg 合理大小远低于 2 GiB。
    const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let dir = paths::updates_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建下载目录失败: {e}"))?;
    let dest = dir.join(format!("pinvou3_{}.dmg", safe_version(&info.latest_version)));
    // 下载写入临时文件(.part),校验通过后再原子 rename 到 dest。
    // 直接 File::create(&dest) 会跟随已存在的符号链接(O_CREAT|O_TRUNC|O_WRONLY),
    // 同用户攻击者可在 ~/.pinvou3/updates/ 预置符号链接 → 信任的更新器截断任意用户文件。
    // 临时文件用 create_new(O_EXCL) 打开:路径已是符号链接时直接失败,不跟随。
    // PID 后缀避免多实例并发写同一 .part。
    let temp = dir.join(format!(
        "pinvou3_{}.dmg.part.{}",
        safe_version(&info.latest_version),
        std::process::id()
    ));
    // 清理上次崩溃残留的 .part(幂等)。
    let _ = std::fs::remove_file(&temp);
    let expected = info.sha256.to_lowercase();
    // sha256 格式预检:清单中的 sha256 必须是 64 位十六进制。格式错误(截断/拼写/
    // 非法字符)时提前拒绝,避免完整下载后才发现校验失败——既浪费带宽又给困惑的错误信息。
    if !is_valid_sha256_hex(&expected) {
        return Err(format!(
            "清单 sha256 格式非法(期望 64 位十六进制,实际 \"{}\")",
            &info.sha256
        ));
    }
    // 已存在 + sha256 匹配 → 直接复用(断点续传场景或重复检查)。
    if dest.exists() && file_sha256(&dest).as_deref() == Some(expected.as_str()) {
        return Ok(crate::updater::DownloadUpdateResult::Path(
            dest.to_string_lossy().into_owned(),
        ));
    }

    // 清掉同目录其它 dmg 和残留 .part(避免堆积;Linux 实现清 .deb,这里清 .dmg)。
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".dmg") && !name.ends_with(".part") {
                let _ = std::fs::remove_file(e.path());
            } else if name.ends_with(".part") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    // 磁盘空间预检(macOS df 是 BSD 系,不支持 GNU 的 --output=avail -B1)。
    if info.size > 0 {
        if let Some(avail) = available_kib(&dir) {
            let need_kib = info.size.saturating_add(64 * 1024 * 1024) / 1024;
            if avail < need_kib {
                return Err(format!(
                    "磁盘空间不足:需约 {} MB,当前可用 {} MB",
                    need_kib / 1024,
                    avail / 1024
                ));
            }
        }
    }

    // 流式下载(与 linux_update.rs 同结构:cancel 检查、stall_timeout、progress 事件)。
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("HTTP client 构建失败: {e}"))?;
    let mut resp = client
        .get(&info.url)
        .send()
        .await
        .map_err(|e| format!("下载请求失败: {e}"))?
        .error_for_status()
        .map_err(|e| format!("下载响应异常: {e}"))?;

    let total = if info.size > 0 {
        info.size
    } else {
        resp.content_length().unwrap_or(0)
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("创建下载临时文件失败: {e}"))?;
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut last_emit: u64 = 0;
    cancel.store(false, Ordering::SeqCst);
    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            return Err("已取消下载".to_string());
        }
        let chunk = match timeout(stall_timeout, resp.chunk()).await {
            Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&temp);
                return Err(format!(
                    "下载停滞:超过 {}s 无数据,已中断(网络异常或更新源无响应)",
                    stall_timeout.as_secs()
                ));
            }
            Ok(Err(e)) => {
                drop(file);
                let _ = std::fs::remove_file(&temp);
                return Err(format!("下载中断: {e}"));
            }
            Ok(Ok(None)) => break,
            Ok(Ok(Some(c))) => c,
        };
        // 防御性硬上限(防 DoS / 磁盘填充):在写盘前检查,避免超额数据落盘。
        if downloaded + chunk.len() as u64 > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "下载体积超出上限 {} MB,已中断(更新源异常或清单 size 被篡改)",
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            ));
        }
        file.write_all(&chunk)
            .map_err(|e| format!("写盘失败: {e}"))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        if downloaded - last_emit >= 262_144 || downloaded == total {
            last_emit = downloaded;
            let _ = app.emit(
                "update:progress",
                serde_json::json!({ "downloaded": downloaded, "total": total }),
            );
        }
    }
    drop(file);

    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "sha256 校验失败(期望 {expected} 实际 {actual}),已删除下载文件"
        ));
    }
    // sha256 通过:原子 rename 临时文件 → 目标(rename 替换目录条目,不跟随符号链接)。
    std::fs::rename(&temp, &dest)
        .map_err(|e| format!("下载文件重命名失败: {e}"))?;
    Ok(crate::updater::DownloadUpdateResult::Path(
        dest.to_string_lossy().into_owned(),
    ))
}

/// 本进程专属的 /Applications 暂存目录。缺陷1:只用 PID 后缀,**不扫整个 /Applications**
/// (此前会误删并发安装实例的暂存)。其它 PID 的残留由其下次启动自家清理。
fn staging_path() -> String {
    format!("/Applications/.pinvou3.app.new.{}", std::process::id())
}

/// 跨进程安装锁(缺陷1)。无 libc/nix 依赖 → 用 `OpenOptions::create_new`(底层 O_EXCL)
/// 在 `~/.pinvou3/updates/.install.lock` 上做原子抢占:仅一个进程能 create_new 成功;
/// 失败且为 AlreadyExists 即他人正在安装 → 调用方拒绝。Drop 时删锁文件释放。
///
/// 已知局限:持锁进程被 kill -9/崩溃会留死锁文件,阻塞后续安装直至人工删除。flock 能在
/// 进程退出时由内核自动释放,但需 libc/nix;此处权衡为不引入新 crate 依赖。
struct InstallGuard(std::path::PathBuf);
impl InstallGuard {
    fn acquire() -> Result<Self, String> {
        let dir = paths::updates_dir();
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建更新目录失败: {e}"))?;
        let lock = dir.join(".install.lock");
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(mut f) => {
                // 写入 PID 便于人工排查死锁(macOS 无 /proc,此处不做自动存活检测)。
                let _ = writeln!(f, "{}", std::process::id());
                Ok(InstallGuard(lock))
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err("另一个安装正在进行中,请等待其完成后再试".to_string())
            }
            Err(e) => Err(format!("创建安装锁失败: {e}")),
        }
    }
}
impl Drop for InstallGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// 安装下载好的 dmg:hdiutil attach → PlistBuddy 校验 CFBundleIdentifier →
/// cp -R 到 /Applications → detach + 清 dmg。返回 false = 由前端提示用户重启
/// (与 Windows MSI 同型;返回 true 表示调用方已重启,这里不适用)。
pub fn install_downloaded_update(
    deb_path: Option<String>,
    installer_path: Option<String>,
    info: Option<crate::updater::UpdateInfo>,
) -> Result<bool, String> {
    // 缺陷1修复:跨进程安装序列化(文件锁)。无 libc/nix,用 OpenOptions::create_new
    // (O_EXCL) 在 ~/.pinvou3/updates/.install.lock 上原子抢占:仅一个进程能创建成功,
    // AlreadyExists 即他人正在安装 → 拒绝。guard 在作用域结束(Drop)时删锁释放。
    let _install_guard = InstallGuard::acquire()?;

    // macOS download_update_package 返回 DownloadUpdateResult::Path(String),经 serde
    // untagged 序列化为纯 JSON 字符串。前端 downloadAndInstallUpdate 的 else 分支用
    // { debPath: downloadResult } 调用本函数(不传 installer_path)。因此 deb_path 是
    // 主来源;installer_path(Windows PreparedUpdate 风格)为回退;两者都没有时按
    // info.latest_version 推默认下载路径(version 经 safe_version 清洗,防路径遍历)。
    let dmg = deb_path
        .or(installer_path)
        .or_else(|| {
            info.as_ref().map(|i| {
                paths::updates_dir()
                    .join(format!("pinvou3_{}.dmg", safe_version(&i.latest_version)))
                    .to_string_lossy()
                    .into_owned()
            })
        })
        .ok_or("缺少 dmg 路径")?;

    // 路径白名单:必须在 ~/.pinvou3/updates/ 内 + 扩展名 .dmg + 无引号(防注入)。
    let dmg_path = validate_dmg_path(Path::new(&dmg))?;

    // 安装前复验 sha256(纵深防御 TOCTOU):下载校验通过后到用户点"安装"的时间窗口内,
    // 任何能写入 ~/.pinvou3/updates/ 的主体可替换 dmg。此处用 info.sha256 重算比对。
    // fail-closed:info 缺失或 sha256 为空时拒绝安装,而非静默跳过校验 —— 否则信任链
    // 仅靠 CFBundleIdentifier 字符串,可被任意伪造。
    let expect = info
        .as_ref()
        .map(|i| i.sha256.to_lowercase())
        .filter(|s| !s.is_empty())
        .ok_or("拒绝安装:缺少 sha256 校验信息(无法验证完整性)")?;
    if file_sha256(&dmg_path).as_deref() != Some(expect.as_str()) {
        let _ = std::fs::remove_file(&dmg_path);
        return Err(format!(
            "安装前 sha256 复验失败(期望 {expect}),已删除可疑文件"
        ));
    }

    // 1. hdiutil attach(挂到用户私有目录下的挂载点,-nobrowse 不进 Finder 边栏)。
    //    不用固定的 /tmp/pinvou3-update:/tmp 全局可写,本地攻击者可预创建同名符号
    //    链接制造竞争;且多实例会撞同一节点并互相强制 detach。挂到 updates_dir(用户
    //    私有)规避这两类问题。外部命令一律用绝对路径(防 $PATH 被污染的 cp/hdiutil)。
    let mountpoint = paths::updates_dir().join(format!(".dmg-mount.{}", std::process::id()));
    let _ = std::fs::create_dir_all(&mountpoint);
    let mp_str = mountpoint.to_string_lossy().into_owned();
    // 先确保挂载点没被占用(上次安装崩了没 detach)。
    let _ = Command::new("/usr/bin/hdiutil")
        .arg("detach")
        .arg(&mp_str)
        .arg("-force")
        .status();
    let attach = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-mountpoint"])
        .arg(&mp_str)
        .arg(dmg_path.as_path())
        .output()
        .map_err(|e| format!("hdiutil attach 失败: {e}"))?;
    if !attach.status.success() {
        return Err(format!(
            "hdiutil attach 失败: {}",
            String::from_utf8_lossy(&attach.stderr)
        ));
    }

    // RAII:确保无论后续步骤成功/失败/提前返回(`?`)都 detach,避免挂载泄漏。
    // (此前 attach 后的错误返回路径——PlistBuddy/cp 启动失败——会跳过 detach。)
    struct MountGuard(String);
    impl Drop for MountGuard {
        fn drop(&mut self) {
            let _ = Command::new("/usr/bin/hdiutil")
                .arg("detach")
                .arg(&self.0)
                .arg("-force")
                .status();
            // 缺陷2修复:detach 后删挂载点目录(此前每次安装留一个空 .dmg-mount.<pid>)。
            // 用 remove_dir 而非 remove_dir_all:它是挂载点,即便 detach 失败也不应递归删;
            // 非空时 remove_dir 自然失败(安全行为)。
            let _ = std::fs::remove_dir(&self.0);
        }
    }
    let _guard = MountGuard(mp_str.clone());

    // 2. PlistBuddy 读 CFBundleIdentifier,防伪装 dmg。
    let app_path = format!("{mp_str}/pinvou3.app");
    let plist_check = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleIdentifier"])
        .arg(format!("{app_path}/Contents/Info.plist"))
        .output()
        .map_err(|e| format!("PlistBuddy 读取失败: {e}"))?;
    let bundle_id = String::from_utf8_lossy(&plist_check.stdout).trim().to_string();
    if bundle_id != EXPECTED_BUNDLE_ID {
        return Err(format!("CFBundleIdentifier 不匹配(期望 {EXPECTED_BUNDLE_ID},实际 {bundle_id})"));
    }

    // 缺陷3修复:降级保护。即便 sha256 + bundle_id 都过,攻击者可下放一个真实历史
    // 签名包(版本号伪装成高)实施降级。无 dmg 代码签名验证,这里多读一个
    // CFBundleShortVersionString 做版本交叉校验:新装版本严格小于当前编译进二进制的
    // CARGO_PKG_VERSION 则拒绝(在改 /Applications 之前 fail-fast)。版本号 parse 不了
    // (非标准)只记日志不拒绝,避免误伤合法的非标准版本号。
    let new_ver_out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString"])
        .arg(format!("{app_path}/Contents/Info.plist"))
        .output()
        .map_err(|e| format!("PlistBuddy 读取版本失败: {e}"))?;
    let new_ver = String::from_utf8_lossy(&new_ver_out.stdout)
        .trim()
        .to_string();
    let cur_ver = env!("CARGO_PKG_VERSION");
    match (parse_semver(&new_ver), parse_semver(cur_ver)) {
        (Some(n), Some(c)) if n < c => {
            return Err(format!(
                "拒绝降级安装:当前 {cur_ver},下载包 {new_ver}(可能 latest.json 被篡改)"
            ));
        }
        (None, _) => {
            eprintln!(
                "macos_update: 下载包版本号 \"{new_ver}\" 非标准 semver,跳过降级校验"
            );
        }
        _ => {}
    }

    // 3. 原子替换 /Applications/pinvou3.app。直接 `cp -R` 到已存在的 .app 是**合并**
    //    而非替换——旧资源(废弃 dylib/Helper/框架)会残留并与新版混合(降级残留 /
    //    app 损坏)。改为:cp 到暂存目录 → 旧 .app 改名备份 → mv 暂存到目标。三者
    //    同处 /Applications(APFS 同卷),mv 原子;失败时把备份改回原名回滚。
    let target = "/Applications/pinvou3.app";
    let staging = staging_path();
    let backup = "/Applications/.pinvou3.app.old";
    // 缺陷1修复:只清理自家 PID 的 staging(此前扫整个 /Applications 删所有
    // .pinvou3.app.new.* 会误删并发安装实例的暂存)。其它 PID 的残留由其下次启动
    // 自家清理(它们拿不到安装锁,跑不到这里)。
    // 防御:staging 若是符号链接(攻击者预置指向 /Applications 外),remove_dir_all
    // 会跟随误删外部目录 → 先 symlink_metadata 拦截;是 symlink 则拒绝安装(fail-closed)。
    match std::fs::symlink_metadata(&staging) {
        Ok(md) if md.is_symlink() => {
            return Err(format!(
                "拒绝安装:暂存路径 {staging} 是符号链接(疑似篡改),请人工检查后重试"
            ));
        }
        Ok(_) => {
            let _ = std::fs::remove_dir_all(&staging);
        }
        Err(_) => {} // 不存在,正常。
    }
    let _ = Command::new("/bin/rm").arg("-rf").arg(backup).status();
    let cp = Command::new("/bin/cp")
        .args(["-R", "-p"])
        .arg(&app_path)
        .arg(&staging)
        .status()
        .map_err(|e| format!("cp 到暂存目录失败: {e}"))?;
    if !cp.success() {
        let _ = Command::new("/bin/rm").arg("-rf").arg(&staging).status();
        return Err("复制到 /Applications 失败(可能需要权限,请在 Finder 手动拖拽)".to_string());
    }
    // 旧 .app 改名备份(若存在)。BSD mv 在目标是目录时会"移入"而非替换,
    // 故备份后须确认 target 已不存在,否则 staging→target 的 mv 会退化成移入目录,
    // 导致新版嵌套进旧 .app 内部且 mv 返回成功(静默假成功安装)。
    if Path::new(target).exists() {
        let bak = Command::new("/bin/mv").arg(target).arg(backup).status();
        match bak {
            Ok(s) if s.success() && !Path::new(target).exists() => {}
            _ => {
                let _ = Command::new("/bin/rm").arg("-rf").arg(&staging).status();
                return Err("备份旧版失败(target 仍在,已中止以防 mv 退化为移入目录)".to_string());
            }
        }
    }
    // 原子 mv 暂存 → 目标。
    let mv = Command::new("/bin/mv")
        .arg(&staging)
        .arg(target)
        .status()
        .map_err(|e| format!("mv 新版到 /Applications 失败: {e}"))?;
    if !mv.success() {
        // 回滚:把备份改回原名(若有备份)。回滚本身失败也不能静默——
        // 否则 /Applications 下可能没有任何 pinvou3.app(旧版已改名备份、新版 mv 失败),
        // 用户无法启动应用且看不到根因。
        if Path::new(backup).exists() {
            let rb = Command::new("/bin/mv").arg(backup).arg(target).status();
            if matches!(rb, Ok(s) if !s.success()) {
                return Err(format!(
                    "安装新版失败且回滚也失败,请手动将 {backup} 改回 pinvou3.app 后重启(可能需要权限)"
                ));
            }
        }
        return Err("安装新版失败(可能需要权限,请在 Finder 手动拖拽)".to_string());
    }
    // 旧 .app 备份留作回滚锚点,下次安装时开头清理(此处不立即删,旧进程仍持旧 inode)。

    // 4. 清 dmg(RAII guard 在函数返回时 detach 挂载点)。
    let _ = std::fs::remove_file(&dmg_path);

    // 5. 返回 false:安装完成但进程不退出(与 Linux 同型)。前端收到 Ok 后自动调
    //    restart_app → app.restart() 按路径 exec 新 bundle(路径已指向新文件)。
    //    Ok(true) 是 Windows MSI 语义(安装器接管→app.exit),这里不适用。
    Ok(false)
}

pub async fn report_pending_update_result_info(
    _client: &reqwest::Client,
    _current_version: &str,
) -> Result<crate::updater::PendingUpdateReportResult, String> {
    // macOS 无 H3C OTA 那种"待反馈"概念,直接返回无待反馈。
    Ok(crate::updater::PendingUpdateReportResult {
        had_pending: false,
        // 与 linux_update.rs 对齐:无待反馈概念时 reported=false(而非 true),
        // 避免前端按 reported 字段做重试逻辑时三平台行为不一致。
        reported: false,
        result: "macOS no pending".to_string(),
        message: String::new(),
    })
}

fn file_sha256(path: &Path) -> Option<String> {
    // 流式哈希(与下载路径一致),避免把整个 dmg(可能数百 MB)一次性读进内存。
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(format!("{:x}", hasher.finalize()))
}

/// macOS df 是 BSD 系,不支持 GNU 的 `--output=avail -B1`。用 `df -k <dir>`
/// 取第 4 列(Available,单位 1024-byte blocks)。返回 KiB。
fn available_kib(dir: &Path) -> Option<u64> {
    let out = Command::new("/bin/df")
        .args(["-k"])
        .arg(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // BSD df -k 列序(固定):Filesystem  1K-blocks  Used  Available  Capacity  Mounted-on
    //   → Available 是第 4 列(tokens.get(3))。
    // 缺陷4修复:此前 nth(1).split_whitespace().nth(3) 在设备名超长折行时会取错列
    // (BSD df 把超长设备名单独占一行,数据字段挤到下一行 → nth(1) 拿到的是折行碎片)。
    // 改为跳过 header 后找第一行「完整数据行」(字段数 >= 6,折行碎片字段不足)。
    // 解析不出或 Available=0 → 返回 None(fail-open:跳过磁盘检查,MAX_DOWNLOAD_BYTES
    // 仍是硬兜底)。
    for line in String::from_utf8_lossy(&out.stdout).lines().skip(1) {
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() < 6 {
            continue;
        }
        let n: u64 = toks.get(3)?.parse().ok()?;
        return if n == 0 { None } else { Some(n) };
    }
    None
}

/// 把版本号清洗为纯 [0-9.] 序列。远程 manifest 的 version 字段不可信,被拼进
/// 下载/安装文件路径前必须清洗,防止路径遍历(如 "../../.ssh/x")。semver 解析已能
/// 挡掉非法版本(使 available=false 不触发下载),但路径构造不应依赖该巧合 —— 纵深防御。
fn safe_version(v: &str) -> String {
    v.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect()
}

/// 校验字符串是否为合法的 sha256 十六进制摘要(64 位小写 hex)。调用方已先 to_lowercase,
/// 这里也接受大写以防遗漏。格式错误时 download_update_package 提前拒绝。
fn is_valid_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    // 与 linux_update.rs 思路一致:trim 'v' 前缀 + 空白,严格 3-part。
    // 额外截掉预发布(-beta)与构建元数据(+build)后缀:降级校验(缺陷3)需能比较
    // "0.7.0-beta" 这类版本,取其主.次.修 数字部分。split 始终至少返回 1 个元素,
    // unwrap_or("") 永不触发,仅用于类型对齐。
    let core = v.trim().trim_start_matches('v');
    let core = core.split('-').next().unwrap_or("");
    let core = core.split('+').next().unwrap_or("");
    let mut it = core.splitn(3, '.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    Some((major, minor, patch))
}

fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// dmg 路径白名单:必须在 ~/.pinvou3/updates/ 内 + 扩展名 .dmg + 无单引号。
/// 镜像 linux_update.rs 的 validate_deb_path(那里是 .deb)。
fn validate_dmg_path(path: &Path) -> Result<std::path::PathBuf, String> {
    let canon = path
        .canonicalize()
        .map_err(|e| format!("dmg 文件不存在: {e}"))?;
    let dir = paths::updates_dir()
        .canonicalize()
        .map_err(|e| format!("更新目录不存在: {e}"))?;
    if !canon.starts_with(&dir) {
        return Err("非法路径:dmg 必须在更新下载目录内".to_string());
    }
    if canon.extension().is_none_or(|x| x != "dmg") {
        return Err("非法路径:只接受 .dmg 文件".to_string());
    }
    if canon.to_string_lossy().contains('\'') {
        return Err("非法路径:含引号".to_string());
    }
    Ok(canon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_numeric_not_lexicographic() {
        assert!(is_newer("0.10.0", "0.2.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.1", "0.2.0"));
    }

    #[test]
    fn semver_equal_or_lower_not_newer() {
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn semver_malformed_not_newer() {
        assert!(!is_newer("abc", "0.2.0"));
        assert!(!is_newer("1.2", "0.2.0"));
        assert!(!is_newer("9.9.9", "garbage"));
    }

    #[test]
    fn semver_tolerates_v_prefix_and_spaces() {
        assert!(is_newer("v0.3.0", "0.2.0"));
        assert!(is_newer(" 0.3.0 ", "0.2.0"));
    }

    #[test]
    fn manifest_optional_fields_default() {
        let m: LatestManifest =
            serde_json::from_str(r#"{"version":"0.3.0","url":"u","sha256":"s"}"#).unwrap();
        assert_eq!(m.notes, "");
        assert_eq!(m.size, 0);
        assert!(m.platforms.is_empty());
    }

    #[test]
    fn manifest_with_platforms_map_parses() {
        let json = r#"{
            "version": "0.3.0",
            "url": "https://example.com/fallback.pkg",
            "sha256": "abc",
            "size": 100,
            "platforms": {
                "macos-arm64": {
                    "url": "https://example.com/m.dmg",
                    "format": "dmg",
                    "sha256": "mac-hash",
                    "size": 200
                },
                "linux-arm64": {
                    "url": "https://example.com/l.deb",
                    "sha256": "linux-hash",
                    "size": 150
                }
            }
        }"#;
        let m: LatestManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.platforms.len(), 2);
        let mac = m.platforms.get("macos-arm64").unwrap();
        assert_eq!(mac.url, "https://example.com/m.dmg");
        assert_eq!(mac.format, "dmg");
        assert_eq!(mac.size, 200);
        // restart_after_install 默认 false
        assert!(!mac.restart_after_install);
    }

    /// validate_dmg_path 白名单(镜像 linux_update.rs 的 validate_deb_path_whitelist)。
    /// install_downloaded_update 接受外部传入的 installer_path,白名单是防注入/遍历的最后防线。
    #[test]
    fn validate_dmg_path_whitelist() {
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var("PINVOU3_HOME").ok();
        let root = std::env::temp_dir().join("pinvou3-macos-updater-test");
        std::env::set_var("PINVOU3_HOME", &root);

        let updates = crate::bridge::paths::updates_dir();
        std::fs::create_dir_all(&updates).unwrap();
        let good = updates.join("pinvou3_9.9.9.dmg");
        std::fs::write(&good, b"fake").unwrap();
        assert!(validate_dmg_path(&good).is_ok());

        // 目录外 → 拒
        let outside = root.join("evil.dmg");
        std::fs::write(&outside, b"fake").unwrap();
        assert!(validate_dmg_path(&outside).is_err());

        // 扩展名非 .dmg → 拒
        let txt = updates.join("note.txt");
        std::fs::write(&txt, b"x").unwrap();
        assert!(validate_dmg_path(&txt).is_err());

        // 不存在 → 拒(canonicalize 失败)
        assert!(validate_dmg_path(&updates.join("ghost.dmg")).is_err());

        // 路径遍历 ../ → 拒(canonicalize 后落在 updates 之外)
        let traversal = updates.join("../evil.dmg");
        assert!(validate_dmg_path(&traversal).is_err());

        let _ = std::fs::remove_dir_all(&root);
        match prev {
            Some(v) => std::env::set_var("PINVOU3_HOME", v),
            None => std::env::remove_var("PINVOU3_HOME"),
        }
    }

    /// resolve_asset 是本 PR 的中心新逻辑(优先 platforms[key] 回退顶层)。此前零直接
    /// 测试覆盖(走真实 HTTP 无法 mock),这几条单测守住回退分支不回归。
    /// platform 元组:(key, url, sha, size, version)。
    fn manifest_for_resolve(
        top: (&str, &str, u64),
        platforms: &[(&str, &str, &str, u64, &str)],
    ) -> LatestManifest {
        let mut pf = std::collections::HashMap::new();
        for (k, url, sha, size, ver) in platforms {
            pf.insert(
                k.to_string(),
                crate::updater::PlatformAsset {
                    url: url.to_string(),
                    sha256: sha.to_string(),
                    size: *size,
                    version: ver.to_string(),
                    ..Default::default()
                },
            );
        }
        LatestManifest {
            version: "9.9.9".to_string(),
            notes: String::new(),
            pub_date: String::new(),
            url: top.0.to_string(),
            sha256: top.1.to_string(),
            size: top.2,
            platforms: pf,
        }
    }

    #[test]
    fn resolve_asset_prefers_platform_key_when_present() {
        // 本平台 key 命中且字段完整 → 取 platform 资产(不用顶层),version 也取平台版本。
        let m = manifest_for_resolve(
            ("https://top/x.dmg", "top-hash", 100),
            &[("macos-arm64", "https://plat/m.dmg", "mac-hash", 200, "0.7.0")],
        );
        let (url, sha, size, ver, _) = resolve_asset(&m);
        assert_eq!(url, "https://plat/m.dmg");
        assert_eq!(sha, "mac-hash");
        assert_eq!(size, 200);
        assert_eq!(ver, "0.7.0");
    }

    #[test]
    fn resolve_asset_falls_back_to_top_level_when_key_missing() {
        // 旧 manifest 无 platforms(空 map)→ 取顶层(Linux 客户端向后兼容路径),version=顶层。
        let m = manifest_for_resolve(("https://top/x.dmg", "top-hash", 100), &[]);
        let (url, sha, size, ver, _) = resolve_asset(&m);
        assert_eq!(url, "https://top/x.dmg");
        assert_eq!(sha, "top-hash");
        assert_eq!(size, 100);
        assert_eq!(ver, "9.9.9");
    }

    #[test]
    fn resolve_asset_falls_back_when_platform_asset_incomplete() {
        // key 命中但 sha256 空(发布侧漏填)→ 回退顶层,而非命中残缺资产导致下载完
        // sha256 必校验失败且无兜底(此前的真实缺陷)。
        let m = manifest_for_resolve(
            ("https://top/x.dmg", "top-hash", 100),
            &[("macos-arm64", "https://plat/m.dmg", "", 200, "0.7.0")],
        );
        let (url, sha, size, _, _) = resolve_asset(&m);
        assert_eq!(url, "https://top/x.dmg");
        assert_eq!(sha, "top-hash");
        assert_eq!(size, 100);

        // url 空同理回退。
        let m2 = manifest_for_resolve(
            ("https://top/x.dmg", "top-hash", 100),
            &[("macos-arm64", "", "mac-hash", 200, "0.7.0")],
        );
        let (url2, _, _, _, _) = resolve_asset(&m2);
        assert_eq!(url2, "https://top/x.dmg");
    }

    /// 平台资产命中但 version 字段为空(旧 manifest / 发布侧漏填)→ version 退顶层,
    /// is_newer 仍可用顶层版本兜底(向后兼容,不让 Mac 客户端因缺 version 而看不到更新)。
    #[test]
    fn resolve_asset_empty_platform_version_falls_back_to_top() {
        let m = manifest_for_resolve(
            ("https://top/x.dmg", "top-hash", 100),
            &[("macos-arm64", "https://plat/m.dmg", "mac-hash", 200, "")],
        );
        let (_, _, _, ver, _) = resolve_asset(&m);
        assert_eq!(ver, "9.9.9");
    }

    /// 平台资产命中且 notes 非空 → notes 取平台 notes(不取顶层 Linux 的更新日志)。
    /// 平台 notes 为空 → 退顶层(向后兼容)。
    #[test]
    fn resolve_asset_notes_prefers_platform_then_top() {
        // platform notes 非空 → 取 platform notes。
        let mut m = manifest_for_resolve(
            ("https://top/x.dmg", "top-hash", 100),
            &[("macos-arm64", "https://plat/m.dmg", "mac-hash", 200, "0.7.0")],
        );
        m.notes = "linux changelog".to_string();
        if let Some(a) = m.platforms.get_mut("macos-arm64") {
            a.notes = "mac changelog".to_string();
        }
        let (_, _, _, _, notes) = resolve_asset(&m);
        assert_eq!(notes, "mac changelog");

        // platform notes 空 → 退顶层。
        let mut m2 = manifest_for_resolve(
            ("https://top/x.dmg", "top-hash", 100),
            &[("macos-arm64", "https://plat/m.dmg", "mac-hash", 200, "0.7.0")],
        );
        m2.notes = "linux changelog".to_string();
        // platform notes 默认空(Default::default())
        let (_, _, _, _, notes2) = resolve_asset(&m2);
        assert_eq!(notes2, "linux changelog");
    }

    #[test]
    fn safe_version_strips_path_traversal() {
        // version 字段来自远程 manifest,拼进文件路径前必须清洗,防路径遍历。
        // 纵深防御:即便 is_newer 已挡住非法版本,路径构造不应依赖该巧合。
        assert_eq!(safe_version("0.6.3"), "0.6.3");
        // 只保留 [0-9.],字母/斜杠/其它符号全被剔除 → 无法构成路径分隔。
        let bad = safe_version("../../.ssh/x");
        assert!(
            bad.chars().all(|c| c.is_ascii_digit() || c == '.'),
            "safe_version 残留非法字符: {bad}"
        );
        assert!(!bad.contains('/'));
        // 预发布后缀字母被剥离。
        assert_eq!(safe_version("v1.2.3-beta"), "1.2.3");
    }

    /// sha256 格式校验:合法摘要通过,截断/非十六进制/空串拒绝。
    #[test]
    fn sha256_format_validation() {
        assert!(is_valid_sha256_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        // 大写也应接受(调用方已 to_lowercase,但纵深防御)。
        assert!(is_valid_sha256_hex(
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        ));
        // 截断
        assert!(!is_valid_sha256_hex("e3b0c442"));
        // 非十六进制字符
        assert!(!is_valid_sha256_hex(
            "z3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
        // 空
        assert!(!is_valid_sha256_hex(""));
        // 长度不对
        assert!(!is_valid_sha256_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85"
        ));
    }

    /// resolve_asset 返回的顶层 url 不以 .dmg 结尾时(旧 manifest 顶层是 .deb/.pkg),
    /// check_for_update_info 应标记 available=false。直接验证 `available` 计算逻辑。
    #[test]
    fn non_dmg_url_not_available() {
        let available = |url: &str, latest: &str, current: &str| {
            is_newer(latest, current) && url.ends_with(".dmg")
        };
        // 顶层 .deb(旧 Linux manifest),有新版 → 不应下载(否则 hdiutil 必败)。
        assert!(!available("https://pinvou.com/pinvou3_0.7.0.deb", "0.7.0", "0.6.0"));
        // 顶层 .dmg → 有新版 → 可用。
        assert!(available("https://pinvou.com/pinvou3_0.7.0.dmg", "0.7.0", "0.6.0"));
        // 顶层 .pkg → 不可用(hdiutil 不处理 pkg)。
        assert!(!available("https://pinvou.com/pinvou3_0.7.0.pkg", "0.7.0", "0.6.0"));
    }

    /// 验证 install_downloaded_update 的 dmg 路径解析优先级:
    /// deb_path > installer_path > info-derived。
    /// 此前第一参数被标记 _deb_path 并忽略,导致前端走 { debPath } 分支时
    /// dmg 路径丢失 → "缺少 dmg 路径" → macOS OTA 安装 100% 失败。
    #[test]
    fn install_path_resolution_priority() {
        // 抽出与 install_downloaded_update 第 286-296 行完全相同的解析逻辑,直接验证。
        // 场景 3 调 paths::updates_dir()(读 PINVOU3_HOME),需持 ENV_LOCK 防与其它
        // 操纵 PINVOU3_HOME 的测试并发竞争。
        let _g = crate::bridge::paths::tests::ENV_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let resolve = |deb_path: Option<&str>,
                       installer_path: Option<&str>,
                       latest_version: Option<&str>|
         -> Result<String, &'static str> {
            deb_path
                .map(|s| s.to_string())
                .or_else(|| installer_path.map(|s| s.to_string()))
                .or_else(|| {
                    latest_version.map(|v| {
                        paths::updates_dir()
                            .join(format!("pinvou3_{}.dmg", safe_version(v)))
                            .to_string_lossy()
                            .into_owned()
                    })
                })
                .ok_or("缺少 dmg 路径")
        };

        // 场景 1:deb_path 存在 → 优先取 deb_path(前端 macOS 走 { debPath } 分支)。
        assert_eq!(
            resolve(Some("/path/from/deb.dmg"), Some("/other.dmg"), Some("0.7.0")),
            Ok("/path/from/deb.dmg".to_string())
        );
        // 场景 2:deb_path 缺失时回退 installer_path(Windows PreparedUpdate 风格)。
        assert_eq!(
            resolve(None, Some("/path/from/installer.dmg"), Some("0.7.0")),
            Ok("/path/from/installer.dmg".to_string())
        );
        // 场景 3:两者都缺失时由 info.latest_version 推导(safe_version 清洗)。
        let r3 = resolve(None, None, Some("0.7.0")).unwrap();
        assert!(r3.ends_with("pinvou3_0.7.0.dmg"));
        // 场景 4:三者都缺失 → Err(此前行为是 panic/unwrap,现 fail-closed)。
        assert_eq!(resolve(None, None, None), Err("缺少 dmg 路径"));
    }

    /// 缺陷1:staging 路径必须只含当前 PID(此前扫整个 /Applications 会误删并发实例)。
    #[test]
    fn staging_path_scoped_to_current_pid() {
        let p = staging_path();
        let pid = std::process::id();
        assert!(p.ends_with(&format!(".pinvou3.app.new.{pid}")), "staging={p}");
        assert!(p.contains(&pid.to_string()));
        // 固定前缀,确认落点在 /Applications(而非可被远程字段污染的任意路径)。
        assert!(p.starts_with("/Applications/.pinvou3.app.new."));
    }

    /// 缺陷3:降级校验依赖 parse_semver 能截掉 pre-release/build 后缀再比较,
    /// 非标准版本号应返回 None(调用方据此只记日志不拒绝,避免误伤)。
    #[test]
    fn parse_semver_strips_prerelease_and_rejects_garbage() {
        assert_eq!(parse_semver("1.2.3"), Some((1, 2, 3)));
        // 截断 pre-release 后缀,取主.次.修。
        assert_eq!(parse_semver("0.7.0-beta"), Some((0, 7, 0)));
        assert_eq!(parse_semver("1.0.0+build.7"), Some((1, 0, 0)));
        assert_eq!(parse_semver("2.5.1-rc.2+meta"), Some((2, 5, 1)));
        // v 前缀与空白容忍(与 is_newer 既有契约一致)。
        assert_eq!(parse_semver("v3.4.5"), Some((3, 4, 5)));
        assert_eq!(parse_semver("  3.4.5  "), Some((3, 4, 5)));
        // 非标准:返回 None → 调用方跳过降级校验。
        assert_eq!(parse_semver("abc"), None);
        assert_eq!(parse_semver("1.2"), None);
        assert_eq!(parse_semver(""), None);
        // 降级判定:new 严格小于 cur 时拒绝。
        assert!(parse_semver("0.6.0").unwrap() < parse_semver("0.7.0").unwrap());
    }
}
