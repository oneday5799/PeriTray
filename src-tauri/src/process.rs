//! 进程级基础工具集（模块名沿用历史）：日志子系统（append_log/clean_old_logs）、
//! 本地时间戳 chrono_str（GetLocalTime 直取系统本地时间）、exe 路径、
//! Win32 互操作（to_wide/shell_open）与各类系统面板/文件打开器。
//! 为全仓约三分之二模块提供公共依赖，新增跨模块基础工具优先落于此处。
//!
//! **日志写入自 B5 起是异步的**：`append_log` / `append_verbose_log` 只把行推入
//! 有界队列（`try_send`，永不阻塞），落盘由独立线程 `peritray-log` 按批完成。
//! 故进程退出前须调用 [`flush_log`] 等待队列排空，否则会丢掉最后几行。

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

/// 获取 exe 所在目录
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 获取日志目录（`<exe目录>/logs`）
pub fn logs_dir() -> PathBuf {
    exe_dir().join("logs")
}

/// 获取数据目录（`<exe目录>/data`）
pub fn data_dir() -> PathBuf {
    exe_dir().join("data")
}

/// 获取日志文件路径（写入 logs/ 子目录；once 为 debug_once_{pid}.log，其余按天 debug_YYYYMMDD.log）
fn log_path() -> std::path::PathBuf {
    let dir = logs_dir();
    if crate::config::log_once() {
        dir.join(format!("debug_once_{}.log", std::process::id()))
    } else {
        dir.join(format!("debug_{}.log", local_date_str()))
    }
}

/// 追加日志到文件（标准级：生命周期摘要与各模块常规行）
pub fn append_log(msg: &str) {
    if !crate::config::standard_log_enabled() {
        return;
    }
    write_log(msg);
}

/// 追加诊断日志到文件（详细级：逐路径/轮次/缓存决策等现场细节）
pub fn append_verbose_log(msg: &str) {
    if !crate::config::verbose_log_enabled() {
        return;
    }
    write_log(msg);
}

/// 标准级日志宏：前置开关判断惰性求值，日志关闭时 format! 不执行、零堆分配
#[macro_export]
macro_rules! standard_log {
    ($($arg:tt)*) => {{
        if $crate::config::standard_log_enabled() {
            $crate::process::append_log(&format!($($arg)*));
        }
    }};
}

/// 详细级日志宏：前置开关判断惰性求值，详细日志关闭时 format! 不执行、零堆分配
#[macro_export]
macro_rules! verbose_log {
    ($($arg:tt)*) => {{
        if $crate::config::verbose_log_enabled() {
            $crate::process::append_verbose_log(&format!($($arg)*));
        }
    }};
}

/// 落盘失败计数：首次失败告警，后续静默（防止高频日志重复刷屏）
static LOG_WRITE_FAILS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ═══════════════════════════════════════════════════════════════
// 日志写入：独立写线程 + 有界队列（B5）
//
// 旧实现是「全进程共用一把 `Mutex<Option<(PathBuf, File)>>` + 无缓冲 write_all」：
// 调用方在锁内完成 `log_path()`（读配置锁）→ 建目录 → 打开文件 → `write_all`
// 落盘。`logcost` 探针实测 `write=0ms`、耗时全在 `lock=`，即瓶颈是**锁排队**，
// 而本机杀软实时扫描下单行落盘约 21ms ⇒ 高频日志会串行化成整进程瓶颈，
// 且这条路径就在**主线程**的命令处理上（`update_config`、`toggle_device_hidden` …）。
//
// 新结构把「生产」与「落盘」彻底分离：
//   · 业务线程只做 `try_send`（入队）——**永不阻塞**，队列满则丢弃并计数；
//   · 专用写线程独占文件句柄，按批落盘（文件句柄缓存不再需要任何锁）；
//   · 跨天 / `log_once` 切换由写线程按批重新求值 `log_path()` 处理。
// 这样既摘掉了业务路径上的文件锁，也顺带把「每次写日志都读一次配置锁」消掉。
// ═══════════════════════════════════════════════════════════════

/// 日志队列容量（行）。满则丢弃——**绝不阻塞调用方**，这是本设计的全部意义。
const LOG_QUEUE_CAP: usize = 4096;

/// 单批最多落盘行数：摊薄系统调用开销，同时避免批量过大导致日志延迟可见。
const LOG_BATCH_MAX: usize = 256;

/// 已成功入队 / 已落盘的行数。二者相等即「队列已排空」，`flush_log` 据此等待。
static LOG_QUEUED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static LOG_WRITTEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 因队列已满被丢弃的行数
static LOG_DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

static LOG_TX: std::sync::OnceLock<std::sync::mpsc::SyncSender<String>> =
    std::sync::OnceLock::new();

/// 取日志队列发送端，首次调用时惰性启动写线程。
fn log_sender() -> &'static std::sync::mpsc::SyncSender<String> {
    LOG_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(LOG_QUEUE_CAP);
        // 线程名便于在调试器 / 任务管理器里辨认
        if std::thread::Builder::new()
            .name("peritray-log".to_string())
            .spawn(move || writer_loop(rx))
            .is_err()
        {
            // 线程创建失败（资源耗尽等极端情况）：`rx` 随闭包结束被丢弃，
            // 后续 `try_send` 一律返回 `Disconnected`，`enqueue` 会自动退回同步写。
            eprintln!("[process] 日志写线程创建失败，退回同步写入");
        }
        tx
    })
}

/// 记录一次丢弃；返回是否为「首次丢弃」（用于只告警一次）。
/// 抽成接收计数器的纯函数以便单测（不改全局状态）。
fn note_drop(counter: &std::sync::atomic::AtomicU64) -> bool {
    counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0
}

/// 入队一行日志。**本函数永不阻塞**：这是 B5 的核心契约。
fn enqueue(line: String) {
    use std::sync::mpsc::TrySendError;
    match log_sender().try_send(line) {
        Ok(()) => {
            LOG_QUEUED.fetch_add(1, std::sync::atomic::Ordering::Release);
        }
        Err(TrySendError::Full(_dropped)) => {
            // 队列满 = 落盘速度跟不上日志产生速度。丢弃是刻意选择：
            // 宁可少几行日志，也不能让业务线程（含主线程）在这里排队。
            if note_drop(&LOG_DROPPED) {
                eprintln!(
                    "[process] 日志队列已满（容量 {} 行），开始丢弃日志；不影响应用运行",
                    LOG_QUEUE_CAP
                );
            }
        }
        Err(TrySendError::Disconnected(line)) => {
            // 写线程不可用（创建失败或已异常退出）：退回同步写。
            // 这条路径正常永不触发，性能不重要，正确性优先。
            write_line_sync(&line);
        }
    }
}

/// 同步落盘一行（降级路径，仅当写线程不可用时使用）
fn write_line_sync(line: &str) {
    use std::io::Write;
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            let _ = f.write_all(line.as_bytes());
        }
        Err(_) => {
            if !LOG_WRITE_FAILS.swap(true, std::sync::atomic::Ordering::SeqCst) {
                eprintln!("[process] 日志写入失败，日志可能丢失: {:?}", path);
            }
        }
    }
}

/// 写线程主体：阻塞取首行 → 尽可能多取（至多 `LOG_BATCH_MAX`）→ 批量落盘。
/// 发送端是静态量、进程存续期内不会析构，故 `recv()` 只在写线程异常时才返回 `Err`。
fn writer_loop(rx: std::sync::mpsc::Receiver<String>) {
    let mut sink = LogSink::default();
    let mut batch: Vec<String> = Vec::with_capacity(LOG_BATCH_MAX);
    while let Ok(first) = rx.recv() {
        batch.clear();
        batch.push(first);
        while batch.len() < LOG_BATCH_MAX {
            match rx.try_recv() {
                Ok(m) => batch.push(m),
                Err(_) => break,
            }
        }
        // 按批重新求值路径：跨天轮转与 `log_once` 切换在此自然生效
        sink.write_batch(&log_path(), &batch);
        LOG_WRITTEN.fetch_add(batch.len() as u64, std::sync::atomic::Ordering::Release);
    }
}

/// 把队列中的日志尽快落盘，最长等待 2 秒。
///
/// 用途：进程退出前调用，避免丢掉最后几行（关停路径恰恰是排查时最想看的部分）。
/// **不得在持有配置锁时调用**：写线程每批都要经 `log_path()` 读配置锁，
/// 持锁等待会把「等队列排空」变成「等自己」（本函数有 2s 上限，会退化为超时而非死锁）。
pub fn flush_log() {
    use std::sync::atomic::Ordering;
    let queued = LOG_QUEUED.load(Ordering::Acquire);
    if LOG_WRITTEN.load(Ordering::Acquire) >= queued {
        return;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
    while LOG_WRITTEN.load(Ordering::Acquire) < queued && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// 日志文件句柄缓存。**由写线程独占持有**，故不再需要任何锁
/// （旧实现需要一把全进程共用的 Mutex，正是 B5 要消除的瓶颈）。
#[derive(Default)]
struct LogSink {
    cached: Option<(std::path::PathBuf, std::fs::File)>,
}

impl LogSink {
    /// 把一批行追加到 `path`；路径与缓存不一致时（跨天 / `log_once` 切换）重建句柄。
    fn write_batch(&mut self, path: &std::path::Path, lines: &[String]) {
        use std::io::Write;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Some((ref cached_path, _)) = self.cached {
            if cached_path.as_path() != path {
                self.cached = None;
            }
        }
        if self.cached.is_none() {
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                Ok(f) => self.cached = Some((path.to_path_buf(), f)),
                Err(_) => {
                    if !LOG_WRITE_FAILS.swap(true, std::sync::atomic::Ordering::SeqCst) {
                        eprintln!("[process] 日志写入失败，日志可能丢失: {:?}", path);
                    }
                    return;
                }
            }
        }
        if let Some((_, ref mut file)) = self.cached {
            for line in lines {
                let _ = file.write_all(line.as_bytes());
            }
        }
    }
}

/// 组装一行日志（时间戳前缀 + 换行）并入队
fn write_log(msg: &str) {
    enqueue(format!("[{}]{}\n", chrono_str(), msg));
}

/// 判断 `name` 是否是**本应用产出的**日志文件名。
///
/// 只认三种形态，其余一律不碰（P2-9）：
/// - `debug_YYYYMMDD.log`：当前按天命名；
/// - `debug_once_{pid}.log`：保留时长 = 「仅一次」时的命名（`{pid}` 为**纯数字**）；
/// - `debug.log`：历史版本（迁移到 `logs/` 子目录前的根目录命名）。
///
/// **为什么不能再用 `starts_with("debug") && ends_with(".log")`**：
/// 那条前缀规则会把用户自己放进 `logs/` 的无关文件（`debug_user.log`、
/// `debug_2024_notes.log`……）一并当成「旧格式」删掉。删除是不可逆操作，
/// 匹配必须「只删自己写的」，容错方向要偏向**不删**。
fn is_managed_log_name(name: &str) -> bool {
    if name == "debug.log" {
        return true;
    }
    // debug_once_{pid}.log：pid 必须是纯数字，避免 debug_once_backup.log 之类被误判
    if let Some(rest) = name
        .strip_prefix("debug_once_")
        .and_then(|r| r.strip_suffix(".log"))
    {
        return !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit());
    }
    // debug_YYYYMMDD.log：复用日期解析，天然要求严格的 8 位数字
    parse_log_date(name).is_some()
}

/// 判定 `name` 对应的日志文件是否应当删除。
///
/// `today_days` 为「今天」的自 1970-01-01 起的天数，由调用方算好传入 ⇒
/// 本函数**不读时钟**，单测可自由构造「今天」而无需等到特定日期。
fn should_delete_log(name: &str, retention: crate::config::LogRetention, today_days: i64) -> bool {
    use crate::config::LogRetention;

    if !is_managed_log_name(name) {
        return false;
    }
    // 「仅一次」模式下，非本次运行的日志都该清掉。不误删活动日志靠两道防线：
    // 本进程的按文件名排除（调用方传 `current_name`），其他实例的靠活动性探测
    // （见 `is_file_in_use`）。缺了后者，第二实例启动就会删掉主实例正在写的日志。
    if retention == LogRetention::Once {
        return true;
    }
    let Some((fy, fm, fd)) = parse_log_date(name) else {
        // 非日期命名（`debug.log` / `debug_once_{pid}.log`）属历史格式，直接清掉
        return true;
    };
    today_days - days_from_civil(fy as i64, fm as i64, fd as i64) >= retention_days(retention)
}

/// 判断日志文件是否正被**任意进程**持有（活动性探测）。
///
/// **为什么需要它**：`Once` 保留策略下 `should_delete_log` 一律返回 true，而
/// 「跳过活动日志」原本只比对**本进程**的文件名（`log_path()` 按 pid 命名）。
/// 于是第二个实例启动时——也就是用户双击图标的最高频路径——会把**主实例正在写**
/// 的 `debug_once_{主pid}.log` 删掉；主实例的写线程仍缓存着该句柄（`LogSink`），
/// 后续日志全部写进「已从目录删除」的文件 ⇒ 磁盘有数据但目录里看不见。
/// `clean_old_logs()` 早于单实例插件注册（`main.rs`），故单实例机制帮不上忙。
///
/// **探测原理**：Rust 的 `File` 在 Windows 上默认以
/// `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE` 打开，故用
/// `share_mode(0)`（不共享）打开同一路径时，只要已有持有者就必然失败并返回
/// `ERROR_SHARING_VIOLATION (32)`。本机实测：本进程持有与跨进程持有均返回 raw=32，
/// 而同为默认共享模式打开则成功 ⇒ 失败确实源于共享位，而非权限或路径问题。
///
/// **容错方向**：只有 `NotFound` 判为「未占用」，其余错误（权限不足、杀软锁、
/// 路径异常）一律判为**被占用**。删除不可逆：误判成「被占用」只是少清一个文件
/// （下次启动会重试），误判成「未占用」则丢日志。
///
/// 注：本模块整体依赖 Win32（顶部 `OsStrExt` 等），无跨平台分支的必要。
fn is_file_in_use(path: &std::path::Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    match std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
    {
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

/// 一次目录清理的计数结果（供单测断言，也让调用方可汇总）
#[derive(Debug, Default, PartialEq, Eq)]
struct CleanOutcome {
    /// 已删除的文件数
    deleted: usize,
    /// 因正被某个进程持有而跳过的文件数
    in_use: usize,
}

/// 清理 `dir` 下按保留策略应删除的日志文件。
///
/// 与 `clean_old_logs` 拆开是为了**可测**：后者固定作用于 `logs_dir()`
/// （= exe 目录），单测无法在不污染真实日志目录的前提下验证
/// 「删除判据 + 活动性探测」这条接线是否真的接通。
fn clean_log_dir(
    dir: &std::path::Path,
    retention: crate::config::LogRetention,
    current_name: Option<&std::ffi::OsStr>,
    today_days: i64,
) -> CleanOutcome {
    let mut out = CleanOutcome::default();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // 第一道防线：跳过本进程正在写入的日志（按文件名，零系统调用）。
        // 探测法也能挡住它，但这一条更廉价，且不依赖「句柄已建立」这一前提。
        if current_name == Some(name.as_os_str()) {
            continue;
        }
        if !should_delete_log(&name_str, retention, today_days) {
            continue;
        }
        // 第二道防线：**其他实例**正在写的日志。按文件名排除覆盖不到它们，
        // 这是 `Once` 模式下唯一的防线（也正是本函数被拆出来的原因）。
        if is_file_in_use(&entry.path()) {
            out.in_use += 1;
            crate::verbose_log!("[process] 跳过被占用的日志: {}", name_str);
            continue;
        }

        match std::fs::remove_file(entry.path()) {
            Ok(()) => {
                out.deleted += 1;
                // 删除是不可逆的：留下一条可追溯的记录，否则「日志莫名少了」无法定位
                crate::standard_log!("[process] 清理旧日志: {}", name_str);
            }
            Err(e) => {
                // 单个文件删除失败（被占用 / 只读）不应影响其余文件的清理
                crate::verbose_log!("[process] 清理旧日志失败: {} ({})", name_str, e);
            }
        }
    }
    out
}

/// 清理旧日志文件（根据保留时长设置）
pub fn clean_old_logs() {
    let retention = crate::config::with_config(|c| c.log_retention);

    // 根目录遗留：旧版本把日志写在 exe 根目录，这里无条件清除，避免根目录杂乱。
    // 自 B5 起 `log_path()` 恒指向 `logs/`，故根目录不可能存在活动日志，无需探测。
    remove_legacy_root_logs();

    let current_name = log_path().file_name().map(|n| n.to_owned());
    let (y, m, d) = local_date();
    let today_days = days_from_civil(y as i64, m as i64, d as i64);

    clean_log_dir(&logs_dir(), retention, current_name.as_deref(), today_days);
}

/// 清除 exe 根目录下旧版本遗留的 debug*.log（迁移至 logs/ 前的历史文件）
///
/// 与 `clean_old_logs` 用同一套 `is_managed_log_name` 判定：根目录那种
/// 「`debug` 开头 + `.log` 结尾」的宽匹配同样会误删用户的文件（P2-9）。
fn remove_legacy_root_logs() {
    if let Ok(entries) = std::fs::read_dir(exe_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if is_managed_log_name(&name_str)
                && entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            {
                // 失败记 verbose 级，与 `clean_old_logs` 对同类操作的处理保持一致（P3-7）。
                // 清理失败不算状态不一致（下次启动会重试），但完全静默会让
                // 「根目录日志删不掉」无从归因。
                if let Err(e) = std::fs::remove_file(entry.path()) {
                    verbose_log!("[process] 清理根目录旧日志失败 {}: {}", name_str, e);
                }
            }
        }
    }
}

/// 保留时长 → 天数（Once 不在此路径，其值仅占位）
fn retention_days(r: crate::config::LogRetention) -> i64 {
    match r {
        crate::config::LogRetention::OneDay => 1,
        crate::config::LogRetention::ThreeDays => 3,
        crate::config::LogRetention::OneWeek => 7,
        crate::config::LogRetention::OneMonth => 30,
        crate::config::LogRetention::Once => 0,
    }
}

/// 从 `debug_YYYYMMDD.log` 解析日期；不是「debug_ + 8 位数字 + .log」则返回 None。
/// once 文件为 `debug_once_{pid}.log`（前缀天然区分），无需日历合法性校验。
fn parse_log_date(name: &str) -> Option<(i32, i32, i32)> {
    let rest = name.strip_prefix("debug_")?.strip_suffix(".log")?;
    if rest.len() != 8 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y = rest[0..4].parse::<i32>().ok()?;
    let m = rest[4..6].parse::<i32>().ok()?;
    let d = rest[6..8].parse::<i32>().ok()?;
    Some((y, m, d))
}

/// 本地日期（年、月、日）。Windows 直取 GetLocalTime；非 Windows 由 epoch 天数反解。
fn local_date() -> (i32, i32, i32) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        (st.wYear as i32, st.wMonth as i32, st.wDay as i32)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Ok(dur) =
            std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH)
        else {
            return (1970, 1, 1);
        };
        let days: i64 = (dur.as_secs() / 86400) as i64 + 719_468;
        let era = days.div_euclid(146_097);
        let doe = days - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let mut y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        if m <= 2 {
            y += 1;
        }
        (y as i32, m as i32, d as i32)
    }
}

/// 本地日期字符串 YYYYMMDD（用于按天轮转的日志文件名）
fn local_date_str() -> String {
    let (y, m, d) = local_date();
    format!("{:04}{:02}{:02}", y, m, d)
}

/// civil 日期 → 自 1970-01-01 的天数（Hinnant 算法；仅用于求差，偏移常量不影响正确性）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn chrono_str() -> String {
    // 直接使用系统本地时间，避免手动 UTC 偏移计算的边界问题
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        return format!(
            "{:04}.{:02}.{:02} {:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::time::SystemTime;
        let Ok(dur) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) else {
            return "????.??.?? ??:??:??".into();
        };
        let secs = dur.as_secs();
        let h = (secs / 3600) % 24;
        let min = (secs / 60) % 60;
        let s = secs % 60;
        let days = secs / 86400 + 719468;
        let era = days / 146097;
        let doe = days - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mon = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mon <= 2 { y + 1 } else { y };
        format!("{:04}.{:02}.{:02} {:02}:{:02}:{:02}", y, mon, d, h, min, s)
    }
}

/// 将字符串转换为 Windows 宽字符串 (null-terminated UTF-16)
pub fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 动态加载 DLL 并返回模块句柄（封装 kernel32 LoadLibraryA；name 须以 `\0` 结尾）
pub unsafe fn load_library(name: &[u8]) -> *mut core::ffi::c_void {
    LoadLibraryA(name.as_ptr())
}

/// 按名取 DLL 导出函数地址（封装 kernel32 GetProcAddress；name 须以 `\0` 结尾）
pub unsafe fn get_proc_address(
    module: *mut core::ffi::c_void,
    name: &[u8],
) -> *mut core::ffi::c_void {
    GetProcAddress(module, name.as_ptr())
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut core::ffi::c_void;
    fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> *mut core::ffi::c_void;
}

/// 通过 ShellExecuteW 打开文件/URL/命令。
/// **全仓「打开外部目标」的唯一实现**——原 `open_with_system` 走
/// `cmd /c start`，会经 cmd.exe 二次解析命令行（Rust 1.77+ 虽已针对
/// `cmd`/`bat` 打了 BatBadBut 补丁，但这层隐式依赖不应保留），已统一到此处。
///
/// 失败判据来自 ShellExecuteW 的返回值：**> 32 为成功，≤ 32 为错误码**
/// （见 ShellExecute 文档的返回值表），因此本函数能报出「协议未注册」
/// 「文件不存在」这类真实失败，而非仅捕获进程创建失败。
pub fn shell_open(file: &str, params: Option<&str>) -> Result<(), String> {
    let wide_file = to_wide(file);
    let wide_params = params.map(to_wide);
    let wide_verb = to_wide("open");
    let ret = unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            wide_verb.as_ptr(),
            wide_file.as_ptr(),
            wide_params
                .as_ref()
                .map_or(std::ptr::null(), |v| v.as_ptr()),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        )
    };
    let code = ret as isize;
    if code <= 32 {
        standard_log!("[process] shell_open failed: {} (code {})", file, code);
        return Err(format!("打开失败（错误码 {}）", code));
    }
    Ok(())
}

/// 打开旧版声音控制面板 (mmsys.cpl)
pub fn open_sound_panel(panel: &str) {
    let _ = shell_open(
        "rundll32.exe",
        Some(&format!("shell32.dll,Control_RunDLL mmsys.cpl,,{}", panel)),
    );
}

/// 打开现代 Windows 设置页面 (ms-settings:)
pub fn open_settings_page(page: &str) {
    let _ = shell_open(&format!("ms-settings:{}", page), None);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 每个用例独立的临时路径（沿用仓库既有写法：temp_dir + tag + pid）
    fn tmp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("peritray_logsink_{}_{}", tag, std::process::id()))
    }

    fn lines(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| format!("{}\n", s)).collect()
    }

    /// 丢弃计数只在**首次**丢弃时返回 true ⇒ 告警只打一次，
    /// 不会在日志风暴中把 stderr 刷爆。
    #[test]
    fn note_drop_only_reports_first_drop() {
        let counter = AtomicU64::new(0);
        assert!(note_drop(&counter), "第一次丢弃应返回 true（需要告警）");
        assert!(!note_drop(&counter), "第二次起不应再返回 true");
        assert!(!note_drop(&counter));
        assert_eq!(counter.load(Ordering::Relaxed), 3, "计数须如实累计");
    }

    #[test]
    fn managed_log_name_accepts_only_own_formats() {
        // 三种本应用产出的形态
        assert!(is_managed_log_name("debug_20260918.log"), "按天命名");
        assert!(is_managed_log_name("debug_once_12345.log"), "仅一次模式");
        assert!(is_managed_log_name("debug.log"), "历史根目录命名");

        // 用户自己放进 logs/ 的文件 —— 这是 P2-9 的核心：修复前会被删掉
        assert!(
            !is_managed_log_name("debug_user.log"),
            "不应动用户的 debug_* 文件"
        );
        assert!(
            !is_managed_log_name("debug_2024_notes.log"),
            "非 8 位日期不算本应用格式"
        );
        assert!(
            !is_managed_log_name("debug_once_backup.log"),
            "pid 必须是纯数字"
        );
        assert!(!is_managed_log_name("debug_once_.log"), "空 pid 不算");
        assert!(
            !is_managed_log_name("debug_20260918.txt"),
            "扩展名必须是 .log"
        );
        assert!(
            !is_managed_log_name("mydebug_20260918.log"),
            "前缀必须完全匹配"
        );
        assert!(!is_managed_log_name("debug_2026091.log"), "7 位日期不算");
        assert!(!is_managed_log_name("debug_202609181.log"), "9 位日期不算");
    }

    #[test]
    fn should_delete_log_respects_retention_boundary() {
        use crate::config::LogRetention;
        // 以「今天 = 第 N 天」构造，避免依赖真实日期
        let today = days_from_civil(2026, 9, 18);

        // 三天保留：9/15（差 3 天，恰好到界）删；9/16（差 2 天）留
        assert!(should_delete_log(
            "debug_20260915.log",
            LogRetention::ThreeDays,
            today
        ));
        assert!(
            !should_delete_log("debug_20260916.log", LogRetention::ThreeDays, today),
            "差 2 天 < 3 天，必须保留"
        );
        // 边界另一侧：差 4 天同样删
        assert!(should_delete_log(
            "debug_20260914.log",
            LogRetention::ThreeDays,
            today
        ));

        // 一天保留
        assert!(should_delete_log(
            "debug_20260917.log",
            LogRetention::OneDay,
            today
        ));
        assert!(!should_delete_log(
            "debug_20260918.log",
            LogRetention::OneDay,
            today
        ));

        // 「仅一次」：任何本应用日志都删（活动文件由调用方按文件名排除）
        assert!(should_delete_log(
            "debug_20260918.log",
            LogRetention::Once,
            today
        ));
        assert!(should_delete_log(
            "debug_once_999.log",
            LogRetention::Once,
            today
        ));

        // 非本应用文件：**任何保留策略下都不删**
        for r in [
            LogRetention::Once,
            LogRetention::OneDay,
            LogRetention::ThreeDays,
            LogRetention::OneWeek,
            LogRetention::OneMonth,
        ] {
            assert!(
                !should_delete_log("debug_user.log", r, today),
                "用户的文件在任何保留策略下都不该被删（策略 {:?}）",
                r
            );
        }
    }

    #[test]
    fn should_delete_log_removes_legacy_non_dated_names() {
        use crate::config::LogRetention;
        let today = days_from_civil(2026, 9, 18);
        // 历史格式无日期可比 ⇒ 直接视为过期
        assert!(should_delete_log(
            "debug.log",
            LogRetention::OneMonth,
            today
        ));
        assert!(should_delete_log(
            "debug_once_777.log",
            LogRetention::OneMonth,
            today
        ));
    }

    #[test]
    fn should_delete_log_tolerates_future_dates() {
        use crate::config::LogRetention;
        // 未来日期（时钟回拨/篡改）差值 ≤ 0，不该被判过期而删掉
        let today = days_from_civil(2026, 9, 18);
        assert!(!should_delete_log(
            "debug_20261231.log",
            LogRetention::OneDay,
            today
        ));
    }

    /// 跨批次追加：同一路径多次 `write_batch` 不得截断已有内容
    /// （若误用 `create(true)` 而漏掉 `append(true)`，本用例会红）。
    #[test]
    fn write_batch_appends_across_calls() {
        let path = tmp_path("append").with_extension("log");
        std::fs::remove_file(&path).ok();
        let mut sink = LogSink::default();
        sink.write_batch(&path, &lines(&["first", "second"]));
        sink.write_batch(&path, &lines(&["third"]));
        let content = std::fs::read_to_string(&path).expect("日志文件应存在");
        assert_eq!(content, "first\nsecond\nthird\n");
        std::fs::remove_file(&path).ok();
    }

    /// 路径变化（跨天轮转 / `log_once` 切换）时必须换文件写：
    /// 旧文件保持原样，新行全部落到新文件。
    /// 这条钉的是句柄缓存的轮转逻辑——漏掉轮转会把新日志写进旧文件。
    #[test]
    fn write_batch_switches_file_when_path_changes() {
        let a = tmp_path("switch_a").with_extension("log");
        let b = tmp_path("switch_b").with_extension("log");
        std::fs::remove_file(&a).ok();
        std::fs::remove_file(&b).ok();
        let mut sink = LogSink::default();
        sink.write_batch(&a, &lines(&["into_a"]));
        sink.write_batch(&b, &lines(&["into_b"]));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "into_a\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "into_b\n");
        std::fs::remove_file(&a).ok();
        std::fs::remove_file(&b).ok();
    }

    /// **进程重启后必须追加到当天已有日志，不得截断**。
    /// 新 `LogSink`（等价于新进程）打开已存在的文件时须保留原有内容——
    /// 若把 `append(true)` 漏成 `truncate(true)`，用户重启一次就丢掉当天全部历史，
    /// 而这正是排查「启动期问题」时最需要的那段日志。
    #[test]
    fn fresh_sink_appends_to_existing_file() {
        let path = tmp_path("restart").with_extension("log");
        std::fs::remove_file(&path).ok();
        LogSink::default().write_batch(&path, &lines(&["from_previous_run"]));
        LogSink::default().write_batch(&path, &lines(&["from_this_run"]));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "from_previous_run\nfrom_this_run\n"
        );
        std::fs::remove_file(&path).ok();
    }

    /// 父目录不存在时须自动创建（首启、日志目录被清理后都会走到）
    #[test]
    fn write_batch_creates_missing_parent_dir() {
        let dir = tmp_path("mkdir");
        std::fs::remove_dir_all(&dir).ok();
        let path = dir.join("nested").join("debug.log");
        let mut sink = LogSink::default();
        sink.write_batch(&path, &lines(&["hello"]));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 打不开目标（此处指向一个已存在的目录）时：不得 panic，也不得写坏进程状态。
    /// 旧实现在此处是 `return`，新实现同样如此；本用例防止把 `unwrap` 引回来。
    #[test]
    fn write_batch_failure_does_not_panic() {
        let dir = tmp_path("open_fail");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let mut sink = LogSink::default();
        // 目标是目录 ⇒ OpenOptions 必然失败
        sink.write_batch(&dir, &lines(&["should not land"]));
        // 再写一次，确认失败后 sink 处于可继续使用的状态
        sink.write_batch(&dir, &lines(&["still no panic"]));
        assert!(dir.is_dir(), "失败路径不应破坏目标");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 端到端冒烟：入队 → 写线程落盘 → `flush_log()` 返回后内容必须在文件里。
    ///
    /// 走 `enqueue` 而非 `append_log`，是为了绕开日志级别开关
    /// （`LOG_LEVEL` 是 `config.rs` 的私有静态量，单测里没有初始化配置的入口），
    /// 直接验证 B5 新增的那条链路本身。
    ///
    /// 本用例的判别力在于「写线程确实启动、确实把队列落到 `log_path()`、
    /// `flush_log` 确实能等到排空」——它抓不住「flush 提前返回」这类竞态
    /// （那种情况下断言可能碰巧成立），那属于契约而非断言能覆盖的范围。
    #[test]
    fn enqueued_lines_reach_disk_after_flush() {
        let path = log_path();
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let marker = format!("peritray_b5_probe_{}", std::process::id());
        enqueue(format!("{}\n", marker));
        flush_log();
        let after = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            after.len() > before.len(),
            "flush 返回后日志文件应已增长（写线程未落盘？）"
        );
        assert!(after.contains(&marker), "标记行必须已落盘: {}", marker);
    }

    // ═══════════════════════════════════════════════════════════════
    // 活动性探测（`is_file_in_use` / `clean_log_dir`）
    //
    // 修的是这条真实缺陷：`retention = once` 时，第二个实例启动会删掉
    // 主实例正在写的 `debug_once_{主pid}.log`（主实例的写线程仍缓存着句柄，
    // 后续日志写进「已从目录删除」的文件 ⇒ 磁盘有数据但目录里看不见）。
    // 触发路径是用户双击图标这一最高频操作。
    // ═══════════════════════════════════════════════════════════════

    /// 探测的前提：默认共享模式下打开的 `File` 会让 `share_mode(0)` 打开失败。
    /// 本机实测该失败为 raw=32（`ERROR_SHARING_VIOLATION`），且 `kind()` 是
    /// `Uncategorized` 而非 `PermissionDenied` ⇒ 判据不能只看 `kind()`。
    #[test]
    fn is_file_in_use_detects_held_file() {
        let path = tmp_path("inuse").with_extension("log");
        std::fs::remove_file(&path).ok();
        std::fs::write(&path, b"x\n").unwrap();

        // 无人持有 ⇒ 未占用
        assert!(!is_file_in_use(&path), "无人持有的文件不该判为占用");

        // 持有中（默认共享模式）⇒ 占用
        let mut holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        holder.write_all(b"holding\n").unwrap();
        assert!(is_file_in_use(&path), "被持有的文件必须判为占用");

        // 释放 ⇒ 恢复为未占用（证明探测反映的是实时状态，不是一次性结果）
        drop(holder);
        assert!(!is_file_in_use(&path), "句柄释放后不该再判为占用");

        std::fs::remove_file(&path).ok();
    }

    /// 文件不存在时判为「未占用」：否则首次启动（日志目录为空）会把
    /// 「不存在」当成占用，虽不致命但会让计数与日志误导排查方向。
    #[test]
    fn is_file_in_use_false_for_missing_file() {
        let path = tmp_path("inuse_missing").with_extension("log");
        std::fs::remove_file(&path).ok();
        assert!(!is_file_in_use(&path), "不存在的文件应判为未占用");
    }

    /// **核心回归**：被另一个写入者持有的过期日志不得被删除，同时其余过期日志
    /// 必须照常删掉（后者是正控——否则「没删」可能只是清理压根没跑）。
    #[test]
    fn clean_log_dir_skips_file_held_by_another_writer() {
        use crate::config::LogRetention;
        let dir = tmp_path("clean_held");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let today = days_from_civil(2026, 9, 18);

        // 两个都该被删的文件：一个被持有（模拟另一实例的活动日志），一个无人持有
        let held = dir.join("debug_once_4242.log");
        let idle = dir.join("debug_once_4243.log");
        // 用户的文件：任何策略下都不该被碰（P2-9 在新代码路径上的回归）
        let user = dir.join("debug_user.log");

        let mut holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&held)
            .unwrap();
        holder.write_all(b"still writing\n").unwrap();
        std::fs::write(&idle, b"stale\n").unwrap();
        std::fs::write(&user, b"mine\n").unwrap();

        // current_name = None 即「站在第二实例的视角看主实例的日志」
        let out = clean_log_dir(&dir, LogRetention::Once, None, today);

        // 先断言「文件是否还在」：注入验证失败时，报错信息才能直接指向缺陷本身，
        // 而不是停在「计数不对」这种间接信号上。
        assert!(
            held.exists(),
            "★ 被另一写入者持有的日志不得被删除（修复前此处会红）"
        );
        assert!(
            !idle.exists(),
            "无人持有的过期日志应被删除（证明清理确实在跑）"
        );
        assert!(user.exists(), "用户的 debug_user.log 任何情况下都不该被删");
        assert_eq!(out.in_use, 1, "被持有的文件必须计入跳过计数");
        assert_eq!(out.deleted, 1, "无人持有的过期文件必须被删掉（正控）");

        drop(holder);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 按天命名下同样受益：主实例跨过午夜后仍在写昨天的文件，而第二实例的
    /// `current_name` 已是今天 ⇒ 文件名排除失效，只有探测能挡住。
    #[test]
    fn clean_log_dir_skips_held_previous_day_log() {
        use crate::config::LogRetention;
        let dir = tmp_path("clean_prevday");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let today = days_from_civil(2026, 9, 18);

        let held = dir.join("debug_20260917.log");
        let mut holder = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&held)
            .unwrap();
        holder.write_all(b"crossed midnight\n").unwrap();

        let out = clean_log_dir(
            &dir,
            LogRetention::OneDay,
            Some(std::ffi::OsStr::new("debug_20260918.log")),
            today,
        );

        assert!(held.exists(), "★ 被持有的跨天日志不得被删除");
        assert_eq!(out.in_use, 1, "昨天的活动日志应被探测挡住");
        assert_eq!(out.deleted, 0, "不该删掉任何被持有的文件");

        drop(holder);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 第一道防线：本进程的日志即使尚未建立句柄（写线程惰性启动）也不得删。
    /// 该文件此刻**无人持有** ⇒ 只有文件名判据能保住它，故本用例专测该判据。
    #[test]
    fn clean_log_dir_skips_current_file_by_name() {
        use crate::config::LogRetention;
        let dir = tmp_path("clean_current");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let today = days_from_civil(2026, 9, 18);

        let current = dir.join("debug_once_7777.log");
        std::fs::write(&current, b"mine\n").unwrap();
        assert!(!is_file_in_use(&current), "前提：该文件此刻未被持有");

        let out = clean_log_dir(
            &dir,
            LogRetention::Once,
            Some(std::ffi::OsStr::new("debug_once_7777.log")),
            today,
        );

        assert_eq!(out.deleted, 0, "当前日志不得被删");
        assert!(out.in_use == 0, "它没被占用，不该计入 in_use");
        assert!(current.exists(), "★ 按文件名排除失效时此处会红");

        std::fs::remove_dir_all(&dir).ok();
    }
}
