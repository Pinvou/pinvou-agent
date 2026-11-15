// architecture-guard: allow-target-cfg -- the stat-error-vs-delete test needs a
// chmod-000 parent fixture, and mode bits are a unix concept; the production
// branch under test (stat error != NotFound must not delete) is
// platform-independent, only the fixture cannot be expressed portably.

//! 实时增量索引：notify 监听已纳入的根，文件变化即时更新 L0 元数据库。
//!
//! 事件处理（对齐 file_watcher.rs 的「按存在性判定」做法），对事件里的每条路径：
//! - `symlink_metadata` 成功且是普通文件/目录 → **根范围内**组件级排除 +
//!   [`Excluder::is_skipped`]（与扫描器同一策略源，含 `is_dir` 上下文，带点目录/
//!   无扩展名文件与扫描结果一致）后 `upsert`
//! - `NotFound`（已删除 / rename 移走）→ [`Store::delete_tree`]：目录记录连带子树
//!   一并删（dir rename/move-out 只会冒出父路径事件），文件退化为精确删除
//! - 其他 stat 错误（权限收紧/挂载抖动）→ 只告警不删，避免把仍存在的记录误删
//!
//! 非 UTF-8 拼写的路径两侧都不入库：[`scanner::record_from_parts`] 的 `to_str`
//! 是两条路共同的最终闸门（扫描器经 `to_record`、watcher 经事件路径），坏名目录
//! 下的「干净叶子」同样到不了记录装配（扫描器则按 empty-name 语义在 filter_entry
//! 剪掉整棵子树）——落库结果一致。这类路径的删除事件按 lossy 拼写匹配不到任何
//! 行，安全 no-op。
//!
//! 排除范围与扫描器对齐：recursive 监听不会自动剪枝，事件里仍会冒出
//! node_modules/.ssh 等深处的路径，先按**受监听根的可见范围**做组件级排除（根自身
//! basename + 根下组件；根之上的祖先组件扫描器永不可见，不得据此丢整根事件），
//! 删除事件同样适用，存活路径再按 `is_skipped` 判类型。
//!
//! `spawn` 先把每个根 `canonicalize`（失败即告警跳过该根；同一实体的重复拼写只
//! 注册一次），watch 注册与事件归因统一用规范化拼写——macOS fsevent 本来就把
//! 事件投递成 canonical 路径、Linux inotify 会把相对根拼成绝对路径，按原拼写
//! 归因会被拼写差击穿（整根事件落到根外 fallback、或按两套拼写各成一行）。
//! **接线方须给扫描器与 `spawn` 传同一拼写**（建议都传 canonical 路径），否则
//! 扫描行与事件行的拼写分裂，删除/更新事件只命中其一。
//!
//! 已知限制（接线方需知）：
//! - Linux 递归监听 `$HOME` 级根会消耗大量 inotify watch，可能撞
//!   `fs.inotify.max_user_watches`。首注册阶段撞上限：notify 把首个注册错误直接
//!   返回，`watch()` 报 Err、本侧告警且**不**把该根计入已监听——但此前已注册的
//!   watch 不回滚，该根处于部分监听。真正静默的部分监听另有其源：注册遍历中
//!   stat 抖动/不可读条目被后端吞掉（watch() 照常 Ok，日志仍报已 watch）。运行
//!   期新目录注册失败则以 channel `Err`（MaxFilesWatch）到达并只告警一次，此后
//!   新目录都不再被监听直到重启。大批量根以懒触发重扫兜底。
//! - 兜底重扫是**懒触发**的（进文件管理页才跑 `kb_start_scan`），没有周期重扫；
//!   溢出/掉 watch 只能告警，索引在下次重扫前保持陈旧——接线方应把 `need_rescan`
//!   告警接成重扫触发。Linux inotify 的 Q_OVERFLOW 事件**不携带路径**（内核队列
//!   被所有根共享），只能整组根重扫；macOS fsevent 的丢事件信号带受影响路径；
//!   **Windows 没有任何丢事件信号**——后端缓冲溢出/根删除在其内部记日志并静默
//!   停 watch（既无事件也无 channel Err），Windows 上懒触发重扫就是唯一兜底。
//! - 树内软链目录不下探：watcher 以 `with_follow_symlinks(false)` 构造，与扫描器
//!   `follow_links(false)` 同向（notify 默认是跟随的，必须显式关掉）。该配置由
//!   inotify/kqueue/poll 后端执行；fsevent/Windows 忽略它，但那两个后端本就不
//!   下探软链目录（Windows 对 junction 的遍历行为尚未验证，接线时补测）。软链
//!   **根本身**：canonicalize 后按实体路径监听与归因，不再有「空监听/部分监听」
//!   的歧义。
//! - Linux 的目录 rename 只冒出该目录自身的新旧路径事件：旧子树由
//!   [`Store::delete_tree`] 清掉，但**新路径下的子文件没有任何事件**（inotify 的
//!   watch 跟随 inode，未变动的子文件不触发），其记录要到下次重扫或被改写才补齐。
//! - 被监听根自身：Linux inotify 下被删则 watch 永久失效（无父 watch 可重挂），
//!   被 rename 则事件永远带着旧前缀路径（等同失效）——两者都靠重扫兜底；
//!   macOS fsevent 的流按路径存活，根重建后事件恢复。
//! - macOS fsevent 的 rename 以 `RenameMode::Any` 单路径事件到达，新旧两侧各自
//!   独立到达、无法关联；按存在性判定两者都能正确处理——旧侧事件到达即清掉旧
//!   记录，未到达时旧记录等重扫。
//! - 无去抖：同一文件的连发事件各写一次（幂等）；事件 channel 无界，消费端被
//!   长事务/慢盘拖住时队列会积累（十万级事件 ≈ 数十 MB 量级），大批量灌入场景
//!   由接线方评估 coalescing（file_watcher.rs 的防抖只覆盖 sched-*.json，参考其
//!   窗口思路而非照搬）。
//! - 扫描与事件的并发窗：重扫中途删除的文件可能被扫描批次重新写回（批量提交晚
//!   于事件的删除），行留到下次重扫清；需要强一致的接线方可暂停事件应用。另：
//!   目录被改名进排除名（如 `proj`→`node_modules`）后，其下文件的后续事件都带
//!   排除组件被丢弃，改名前入库的旧行等重扫清。
//!
//! 当前未接线：`spawn` 尚无生产调用方（与闭源侧一致为落库待接线状态）；入口由
//! 后续接线方在启动扫描时按根调用。watcher 是 app 级单例语义：`spawn` 内置了
//! 防重入守卫，重复调用直接忽略；线程任何退出路径（init 失败、全部根注册失败、
//! panic 展开）都会释放守卫，之后可重新 `spawn` 重新武装——需要更换根集合时，
//! 仍应先给线程补停止接口（#869），不能靠叠 spawn。

#![allow(dead_code)] // 库先行：接线前 spawn 无生产调用方（见上方注记）

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::exclude::Excluder;
use super::scanner::{ext_of, record_from_parts};
use super::store::{FileRecord, Store};

/// 非 panicking stderr 写：stderr 关闭时 `eprintln!` 会 panic（Rust 忽略 SIGPIPE，
/// 写失败即 panic），watcher 线程死在这类日志上会无声断掉增量索引——写失败就地
/// 忽略（与 mod.rs 扫描线程的内联防护同款，见其注释）。展开成块表达式（而非裸
/// 语句），使调用在 let-else 的表达式位置也可用。
macro_rules! warn {
    ($($arg:tt)*) => {{
        let _ = writeln!(std::io::stderr(), "[kb_watcher] {}", format_args!($($arg)*));
    }};
}

/// 单实例守卫：watcher 线程随 app 存活，重复 `spawn` 会叠出第二条线程对同一批根
/// 重复消费事件（还会叠加 inotify watch 占用）。接线方要换根集合时必须先停线程。
/// RAII：任何退出路径（init 失败、全部根注册失败、panic 展开、线程自然结束）都
/// 释放守卫，之后可重新 `spawn` 重新武装。
static ACTIVE: AtomicBool = AtomicBool::new(false);

struct ActiveGuard;

impl ActiveGuard {
    /// 已有活跃 watcher 时返回 None（重复 `spawn` 直接忽略）。
    fn acquire() -> Option<Self> {
        ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
            .then_some(Self)
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

/// 启动后台 watcher 线程，监听给定根。spawn 后即返回，线程随 app 存活；
/// 已有活跃 watcher 时本次调用直接忽略（见 [`ACTIVE`]）。
pub fn spawn(store: Store, roots: Vec<PathBuf>) {
    if roots.is_empty() {
        return;
    }
    let Some(active) = ActiveGuard::acquire() else {
        warn!("spawn ignored: a watcher thread is already active");
        return;
    };
    thread::spawn(move || {
        // 守卫随线程体存活：panic 展开时先走 Drop 释放，spawn 不被终身闩死。
        let _active = active;
        // 同 feature 扫描线程的同款收容（见 mod.rs）：线程 panic 只留一条可追
        // 日志，不无声死亡。
        if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_watcher(&store, &roots);
        })) {
            warn!(
                "watcher thread panicked (contained; index stays stale until \
                 the next scan): {panic:?}"
            );
        }
    });
}

/// 线程体（独立于 `spawn`，便于 catch_unwind 收容）：注册所有根后循环消费事件。
fn run_watcher(store: &Store, roots: &[PathBuf]) {
    let ex = Excluder::default();
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    // follow_symlinks(false)：notify 默认跟随软链（inotify 注册时连软链目录
    // 一起下探），与扫描器 follow_links(false) 相反——必须在构造处显式关闭。
    // 该配置由 inotify/kqueue/poll 后端执行；fsevent/Windows 忽略它，但那两个
    // 后端本就不下探软链目录。
    let mut watcher: RecommendedWatcher =
        match RecommendedWatcher::new(tx, notify::Config::default().with_follow_symlinks(false)) {
            Ok(w) => w,
            Err(e) => {
                warn!("init failed: {e}");
                return; // ActiveGuard::drop 释放守卫
            }
        };
    // 根先 canonicalize：macOS fsevent 把事件投递成 canonical 路径、Linux inotify
    // 把相对根拼成绝对路径，按调用方原拼写归因会被拼写差击穿（整根事件落到根外
    // fallback，或两套拼写各成一行）。watch 注册与事件归因统一用规范化拼写。
    let mut watched_roots: Vec<PathBuf> = Vec::new();
    for root in roots {
        let Ok(canonical) = root.canonicalize() else {
            warn!("root({}) canonicalize failed; skipping it", root.display());
            continue;
        };
        if watched_roots.contains(&canonical) {
            continue; // 同一实体的重复拼写只注册一次
        }
        match watcher.watch(&canonical, RecursiveMode::Recursive) {
            Ok(()) => watched_roots.push(canonical),
            // 多半是 inotify 上限/权限；告警即可，懒触发重扫兜底。
            Err(e) => warn!("watch({}) failed: {e}", canonical.display()),
        }
    }
    if watched_roots.is_empty() {
        warn!("no root could be watched; inactive until a later spawn retries");
        return; // ActiveGuard::drop 释放守卫
    }
    warn!("watching {}/{} root(s)", watched_roots.len(), roots.len());

    let mut max_files_warned = false;
    for res in rx {
        match res {
            Ok(ev) => {
                // 后端丢事件信号（inotify Q_OVERFLOW / fsevent KERNEL_DROPPED 等，
                // 以 EventKind::Other + Rescan flag 到达）：这是「增量索引已不可靠」
                // 的唯一主动通知，必须喊出来，接线方据此安排重扫。
                if ev.need_rescan() {
                    warn!(
                        "backend dropped events (rescan requested); \
                         index stays stale until the next scan"
                    );
                }
                if matches!(
                    ev.kind,
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                ) {
                    apply(&ex, store, &watched_roots, &ev);
                }
            }
            // 通道上的 Err：读循环 IO 错误、中途 MaxFilesWatch（notify 之后不再扩
            // watch）等，同样不能静默吞掉。MaxFilesWatch 会对每个新目录重复到达，
            // 只告警一次防日志风暴。
            Err(e) => {
                if matches!(e.kind, notify::ErrorKind::MaxFilesWatch) {
                    if !max_files_warned {
                        max_files_warned = true;
                        warn!(
                            "event error: {e} (watch limit hit; new dirs stay \
                             unwatched until restart, rescan covers the gap)"
                        );
                    }
                } else {
                    warn!("event error: {e}");
                }
            }
        }
    }
}

/// 事件准入：只有 Create/Modify/Remove 改变索引状态。Access（读/开/关）与
/// Other（无 Rescan flag 的杂项）不得进入存在性判定——否则一条 Access 事件会
/// 把「已索引但刚被删」的路径当作删除信号以外的误判源（读一个不存在的路径
/// 不意味着索引要删它）。
fn apply(ex: &Excluder, store: &Store, roots: &[PathBuf], ev: &Event) {
    if !matches!(
        ev.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) {
        return;
    }
    handle_event(ex, store, roots, ev);
}

/// 事件路径的组件级排除，按受监听根取**相对范围**：与扫描器的可见组件一致
/// （walkdir 的 filter_entry 首个看到的就是根本身，其上的祖先组件扫描器永不可见）。
/// roots 已在 spawn 里 canonicalize、与后端投递的路径拼写一致，找不到所属根正常
/// 不会发生——保留全路径判定仅作防御（canonicalize 与注册之间的竞态等）。
fn excluded(ex: &Excluder, roots: &[PathBuf], path: &Path) -> bool {
    match roots
        .iter()
        .filter(|r| path.starts_with(r))
        .max_by_key(|r| r.components().count())
        .map(|r| r.as_path())
    {
        Some(root) => ex.is_excluded_under_root(path, root),
        None => ex.is_excluded_path(path),
    }
}

/// 处理单条事件：按存在性分桶（存活 → upsert，消失 → delete），每事件一次批量写。
/// 独立于线程闭包，便于单元测试。
fn handle_event(ex: &Excluder, store: &Store, roots: &[PathBuf], ev: &Event) {
    let mut ups: Vec<FileRecord> = Vec::new();
    let mut dels: Vec<String> = Vec::new();
    for path in &ev.paths {
        // 组件级排除（根自身 + 根下的 names/secrets）：churn 目录整条跳过，不 stat。
        if excluded(ex, roots, path) {
            continue;
        }
        match std::fs::symlink_metadata(path) {
            Ok(md) => {
                let ft = md.file_type();
                if !ft.is_file() && !ft.is_dir() {
                    continue; // symlink/socket/fifo 本体不入库（与扫描器一致）
                }
                let is_dir = ft.is_dir();
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                let ext = if is_dir { None } else { ext_of(path) };
                if ex.is_skipped(name, is_dir, ext.as_deref()) {
                    continue;
                }
                if let Some(rec) = record_from_parts(path, name, is_dir, Some(&md)) {
                    ups.push(rec);
                }
            }
            // 已删除 / rename 移走：删记录；目录连带子树（inotify 对被移走的子树
            // 不会再有事件）。
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                dels.push(path.to_string_lossy().into_owned());
            }
            // 权限收紧/ESTALE 等抖动：不删——误删仍存在的记录比漏一次更新更伤，
            // 懒触发重扫会补齐。
            Err(e) => warn!("stat({}) failed: {e}", path.display()),
        }
    }
    // 写失败与 stat 抖动同类：都是「增量索引悄悄漏了一拍」。必须喊出来——索引在
    // 下次重扫前会一直缺这条更新，静默吞掉会让「需要重扫」的信号无处可寻。
    // 删除先于写入：同一事件若同时含「目录删除」与其下「新文件」（现役后端不会
    // 合成这种形状，防御性保证），先删后写让新行存活，反之会被前缀删除吃掉。
    if !dels.is_empty() {
        if let Err(e) = store.delete_tree(&dels) {
            warn!("delete({}) failed: {e}", dels.len());
        }
    }
    if !ups.is_empty() {
        if let Err(e) = store.upsert_many(&ups) {
            warn!("upsert({}) failed: {e}", ups.len());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind, RenameMode};
    use std::fs;
    use std::path::Path;

    fn tmp(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("pinvou3_kb_watch_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    fn ev(kind: EventKind, paths: &[&Path]) -> Event {
        let mut e = Event::new(kind);
        for p in paths {
            e = e.add_path(p.to_path_buf());
        }
        e
    }

    fn indexed(store: &Store) -> Vec<String> {
        let mut v: Vec<String> = store.load_index().unwrap().into_keys().collect();
        v.sort();
        v
    }

    /// Create → upsert（字段来自真实 stat）、Remove → delete；Access 事件不进入
    /// 存在性判定（对「已索引但已消失」的路径发 Access 不得触发删除）。
    #[test]
    fn create_and_remove_events_track_the_store() {
        let base = tmp("cr");
        fs::write(base.join("notes.md"), b"# note").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("notes.md")]),
        );
        assert_eq!(
            indexed(&store),
            vec![base.join("notes.md").to_string_lossy().into_owned()],
            "create event upserts exactly the touched file"
        );

        // Access（读/关）不改变索引状态：路径已从盘上消失但事件只是 Access——
        // 若准入失守，这条事件会顺着 NotFound 走到删除分支。
        fs::remove_file(base.join("notes.md")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Access(AccessKind::Close(AccessMode::Write)),
                &[&base.join("notes.md")],
            ),
        );
        assert_eq!(
            indexed(&store).len(),
            1,
            "access events must not reach the delete branch"
        );

        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Remove(RemoveKind::File),
                &[&base.join("notes.md")],
            ),
        );
        assert!(
            indexed(&store).is_empty(),
            "removed path must leave the index"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 目录 Remove 事件：目录记录连带子树一并删（inotify 对子树不再有事件），
    /// 只共享前缀字符的兄弟路径不能被误删。
    #[test]
    fn dir_remove_deletes_the_subtree() {
        let base = tmp("dirsub");
        fs::create_dir_all(base.join("项目/sub")).unwrap();
        fs::write(base.join("项目/报告.pdf"), b"p").unwrap();
        fs::write(base.join("项目/sub/a.md"), b"a").unwrap();
        fs::write(base.join("项目备份.md"), b"s").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Create(CreateKind::Folder), &[&base.join("项目")]),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Create(CreateKind::File),
                &[
                    &base.join("项目/报告.pdf"),
                    &base.join("项目/sub/a.md"),
                    &base.join("项目备份.md"),
                ],
            ),
        );
        assert_eq!(indexed(&store).len(), 4, "dir + 2 children + sibling");

        fs::remove_dir_all(base.join("项目")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Remove(RemoveKind::Folder), &[&base.join("项目")]),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![base.join("项目备份.md").to_string_lossy().into_owned()],
            "subtree gone, prefix-overlapping sibling intact"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// rename（inotify 的 Both 事件带 [from, to]）：旧路径删除、新路径入库。
    #[test]
    fn rename_event_deletes_old_and_upserts_new() {
        let base = tmp("ren");
        fs::write(base.join("old.md"), b"# old").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("old.md")]),
        );
        fs::rename(base.join("old.md"), base.join("新文档.md")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                &[&base.join("old.md"), &base.join("新文档.md")],
            ),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![base.join("新文档.md").to_string_lossy().into_owned()]
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 分离的 From/To 事件与 Both 等价：Windows 后端只发分离事件、从不发 Both，
    /// macOS 只发新路径——「skip From/To、Both 收尾」式的重构若破坏分离形状，
    /// Windows 的 rename 处理会静默坏掉，这条测试咬住它。
    #[test]
    fn separate_from_to_rename_events_match_the_both_shape() {
        let base = tmp("renft");
        fs::write(base.join("旧.md"), b"# old").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("旧.md")]),
        );
        fs::rename(base.join("旧.md"), base.join("新.md")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::From)),
                &[&base.join("旧.md")],
            ),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Name(RenameMode::To)),
                &[&base.join("新.md")],
            ),
        );

        assert_eq!(
            indexed(&store),
            vec![base.join("新.md").to_string_lossy().into_owned()],
            "From deletes the old path, To upserts the new one"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 排除范围止于受监听根：根**之上**的祖先组件不再参与排除——扫描器对
    /// `build`/`snap` 这类排除名祖先下的根照常全量入库（walkdir 从根本身走起，
    /// 看不见上面的祖先），watcher 若按全路径判组件会把整根事件静默丢光；而根本
    /// 身 basename 命中名单时，扫描器的 filter_entry 对首个 entry（根本身）同样
    /// 剪枝整棵树，watcher 必须一致。
    #[test]
    fn exclusion_scope_stops_at_the_watched_root() {
        let base = tmp("rootscope");
        let root = base.join("build/proj"); // 祖先 "build" ∈ SKIP_NAMES
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("报告.pdf"), b"r").unwrap();
        let excluded_root = base.join("node_modules"); // 根自身 basename 命中名单
        fs::create_dir_all(excluded_root.join("pkg")).unwrap();
        fs::write(excluded_root.join("pkg/x.pdf"), b"x").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![root.clone(), excluded_root.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&root.join("报告.pdf")]),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Create(CreateKind::Folder), &[&excluded_root]),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&excluded_root.join("pkg/x.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![root.join("报告.pdf").to_string_lossy().into_owned()],
            "ancestors above the watched root must not exclude events; a root whose \
             own basename is excluded stays pruned like the scanner"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 排除策略与扫描器同源：churn 祖先整条跳过（即使末级是白名单类型）、无扩展名
    /// 敏感文件不入库、带点目录按目录放行、源码排除、白名单类型入库。
    #[test]
    fn exclusion_matches_the_scanner_policy() {
        let base = tmp("excl");
        fs::create_dir_all(base.join("node_modules/pkg")).unwrap();
        fs::create_dir_all(base.join("backup.old")).unwrap();
        fs::write(base.join("node_modules/pkg/保险报价单.pdf"), b"p").unwrap();
        fs::write(base.join(".bash_history"), b"hist").unwrap();
        fs::write(base.join("main.c"), b"int main").unwrap();
        fs::write(base.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        let creates = EventKind::Create(CreateKind::File);
        for p in [
            "node_modules/pkg/保险报价单.pdf",
            ".bash_history",
            "main.c",
            "报告.pdf",
        ] {
            apply(&ex, &store, &roots, &ev(creates, &[&base.join(p)]));
        }
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Create(CreateKind::Folder),
                &[&base.join("backup.old")],
            ),
        );

        let idx = indexed(&store);
        assert_eq!(
            idx,
            vec![
                base.join("backup.old").to_string_lossy().into_owned(),
                base.join("报告.pdf").to_string_lossy().into_owned(),
            ],
            "allowed file + dot-suffixed dir indexed; churn ancestor, extensionless \
             secret and source ext are not"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 带点目录的删除事件同样要走完（组件级排除不吞它）：目录记录与子树都被清掉，
    /// 而不是像旧版那样被末级扩展名判定挡在删除之外、留陈旧行等重扫。
    #[test]
    fn dot_suffixed_dir_delete_event_cleans_the_record() {
        let base = tmp("dotdir");
        fs::create_dir_all(base.join("backup.old")).unwrap();
        fs::write(base.join("backup.old/报告.pdf"), b"p").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Create(CreateKind::Folder),
                &[&base.join("backup.old")],
            ),
        );
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("backup.old/报告.pdf")]),
        );
        assert_eq!(indexed(&store).len(), 2);

        fs::remove_dir_all(base.join("backup.old")).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Remove(RemoveKind::Folder),
                &[&base.join("backup.old")],
            ),
        );

        assert!(
            indexed(&store).is_empty(),
            "dot-suffixed dir rows must be removable by events, not wait for a rescan"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// symlink 本体不入库（与扫描器 to_record 拒收、follow_links(false) 一致）。
    #[test]
    #[cfg(unix)]
    fn symlink_events_do_not_index_the_link() {
        let base = tmp("symlink");
        fs::write(base.join("报告.pdf"), b"r").unwrap();
        std::os::unix::fs::symlink(base.join("报告.pdf"), base.join("link.pdf")).unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("link.pdf")]),
        );

        assert!(
            indexed(&store).is_empty(),
            "the symlink body must not enter the index"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// stat 抖动（父目录权限收紧等）不得把仍存在的记录当删除处理——
    /// `Path::exists()` 会把一切错误折叠成 false，旧版在此误删活记录。
    #[test]
    #[cfg(unix)]
    fn transient_stat_errors_do_not_delete_records() {
        use notify::event::DataChange;
        use std::os::unix::fs::PermissionsExt as _;

        let base = tmp("staterr");
        fs::create_dir_all(base.join("sub")).unwrap();
        fs::write(base.join("sub/notes.md"), b"# n").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&base.join("sub/notes.md")]),
        );
        assert_eq!(store.stats().unwrap().total_files, 1);

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        let readable = base.join("sub").read_dir().is_ok();
        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            // root（或忽略 mode bit 的 fs）绕过权限检查，无法制造 stat 错误，跳过。
            let _ = fs::remove_dir_all(&base);
            return;
        }

        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o000)).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                &[&base.join("sub/notes.md")],
            ),
        );
        fs::set_permissions(base.join("sub"), fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            store.stats().unwrap().total_files,
            1,
            "an unstat-able path must not take the delete branch"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 多根嵌套时事件按**最深**受监听根归因（`max_by_key`）：选出内层根后，外层根
    /// basename（即使命中排除名单）不参与判定——扫描器对内层根照常全量入库
    /// （walkdir 从内层根本身走起），watcher 若误选外层根会把整根事件丢光。
    #[test]
    fn nested_root_events_use_the_deepest_root() {
        let base = tmp("nested");
        let outer = base.join("node_modules"); // 外层根本身命中排除名单
        let inner = outer.join("proj");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![outer.clone(), inner.clone()];

        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&inner.join("报告.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![inner.join("报告.pdf").to_string_lossy().into_owned()],
            "the deepest containing root decides; the outer root's excluded \
             basename must not drop the event"
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 根外路径走防御性全路径判定（canonicalize 后正常不可达）：命中排除组件的
    /// 照旧丢弃，干净路径照常按存在性入库——不得因「不在任何根下」静默丢光。
    #[test]
    fn out_of_root_paths_fall_back_to_full_path_judgment() {
        let base = tmp("fallback");
        fs::create_dir_all(base.join("root")).unwrap();
        let outside =
            std::env::temp_dir().join(format!("pinvou3_kb_watch_out_{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("报告.pdf"), b"r").unwrap();
        // 排除组件的 fixture 必须能通过类型白名单（用报告.pdf 而非 id_rsa），
        // 否则 is_skipped 会在组件门之后再次拦下它，掩盖组件门本身的回归。
        fs::create_dir_all(outside.join(".ssh")).unwrap();
        fs::write(outside.join(".ssh/报告.pdf"), b"r").unwrap();
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.join("root")];

        // 根外干净路径：全路径判定放行 → stat 存活 → 入库。
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&outside.join("报告.pdf")]),
        );
        // 根外命中排除组件的路径：全路径判定丢弃（组件级即拦，无需 stat）。
        apply(
            &ex,
            &store,
            &roots,
            &ev(creates_file(), &[&outside.join(".ssh").join("报告.pdf")]),
        );

        assert_eq!(
            indexed(&store),
            vec![outside.join("报告.pdf").to_string_lossy().into_owned()],
            "out-of-root clean paths must survive the defensive fallback; \
             excluded components must still drop"
        );

        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&outside);
    }

    /// 非 UTF-8 拼写两侧都不入库（`record_from_parts` 的 `to_str` 是扫描器与
    /// watcher 共同的最终闸门；扫描器另按 empty-name 语义在 filter_entry 剪掉
    /// 整棵子树）——落库结果一致。这条钉住「干净叶子也不入」的语义，防止将来把
    /// 记录装配改成 lossy 拼写造成两侧分裂。
    #[test]
    #[cfg(unix)]
    fn non_utf8_spellings_never_enter_the_index() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let base = tmp("nonutf8");
        let bad_dir = base.join(OsStr::from_bytes(b"\xff\xfe.dir"));
        if fs::create_dir_all(&bad_dir).is_err() {
            // 敌意文件系统（如 APFS 拒绝非 UTF-8 名）造不出 fixture，跳过。
            let _ = fs::remove_dir_all(&base);
            return;
        }
        let leaf = bad_dir.join("报告.pdf");
        if fs::write(&leaf, b"r").is_err() {
            let _ = fs::remove_dir_all(&base);
            return;
        }
        let store = Store::open_in_memory().unwrap();
        let ex = Excluder::default();
        let roots = vec![base.clone()];

        apply(&ex, &store, &roots, &ev(creates_file(), &[&leaf]));
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Create(CreateKind::Folder), &[&bad_dir]),
        );
        // 删除事件：lossy 拼写匹配不到任何行，安全 no-op，不误删也不报错。
        fs::remove_dir_all(&bad_dir).unwrap();
        apply(
            &ex,
            &store,
            &roots,
            &ev(EventKind::Remove(RemoveKind::Folder), &[&bad_dir]),
        );

        assert!(
            indexed(&store).is_empty(),
            "non-UTF-8 spellings must stay out of the index on both sides"
        );

        let _ = fs::remove_dir_all(&base);
    }

    fn creates_file() -> EventKind {
        EventKind::Create(CreateKind::File)
    }
}
