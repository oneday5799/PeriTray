use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tauri::menu::MenuItem;

use crate::device::Device;

/// 托盘图标位置
pub static TRAY_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 弹窗窗口位置
pub static POPUP_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 托盘图标所在显示器的信息（缩放因子 + 逻辑工作区），
/// 用于混合 DPI 下弹窗定位与高度 clamp。
#[derive(Debug, Clone, Copy)]
pub struct TrayMonitorInfo {
    pub scale_factor: f64,
    pub work_x: f64,
    pub work_y: f64,
    pub work_w: f64,
    pub work_h: f64,
}

/// 托盘所在显示器信息缓存（托盘点击时刷新，None 表示尚未确定）
pub static TRAY_MONITOR: OnceLock<Mutex<Option<TrayMonitorInfo>>> = OnceLock::new();

/// 获取托盘所在显示器信息缓存的引用
pub fn get_tray_monitor() -> &'static Mutex<Option<TrayMonitorInfo>> {
    TRAY_MONITOR.get_or_init(|| Mutex::new(None))
}

/// 弹窗动画状态
pub static ANIMATING: AtomicBool = AtomicBool::new(false);

/// 开机自启状态
pub static AUTO_START: AtomicBool = AtomicBool::new(false);

/// 快捷键录制期间置位，抑制全局快捷键触发，避免录制时误触发动作
pub static SHORTCUT_RECORDING: AtomicBool = AtomicBool::new(false);

/// 开机自启菜单项引用
pub static AUTO_MENU_ITEM: OnceLock<Mutex<Option<MenuItem<tauri::Wry>>>> = OnceLock::new();

/// 容忍 Mutex 中毒的加锁：锁中毒时直接接管内部数据继续使用
/// （本项目各静态量在 panic 后仅需"可用"而非"严格一致"，统一在此表达该语义）
pub fn lock_unpoisoned<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 单飞标志的 RAII 守卫：CAS 抢占，Drop 时复位。
/// 正常返回与 panic 展开均会复位，避免后台任务被永久锁死。
///
/// 用法要点（见 AGENTS.md「RAII 守卫的绑定命名」）：
/// - **外层绑定不得以 `_` 开头**——若守卫需要在线程闭包内被持有，必须以具名绑定
///   `let Some(guard) = ...` 取得，再在闭包体内显式引用（`let _guard = guard;`）触发捕获。
///   写成 `let Some(_guard)` 且闭包内不引用它时，`move` 闭包**不会捕获它**，
///   守卫会在函数返回时立即 Drop，单飞语义退化为无保护。
/// - 同步路径（在本函数内跑完工作再返回）用 `let Some(_guard)` 是正确的：Drop-only 绑定。
pub(crate) struct SingleFlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> SingleFlightGuard<'a> {
    /// CAS 获取标志；成功返回 guard，失败返回 None（已有任务在跑）
    pub(crate) fn new(flag: &'a AtomicBool) -> Option<Self> {
        if flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(Self { flag })
        } else {
            None
        }
    }
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// 单飞 + 合并执行器：同一时刻只跑一轮 `job`，且**尽量不丢更新**。
///
/// 语义（B3 引入，用于托盘菜单重建）：
/// - 抢到单飞权者执行 `job`。每轮开始前先清掉积压的 `pending`（「认领」），
///   于是**启动前**积压的多个请求被合并成一轮；
/// - 执行**期间**到达的请求把 `pending` 置起并**立即返回**（不阻塞事件分发），
///   由持权者在收尾时补跑一轮，因此这类请求不会丢；
/// - 返回实际执行的轮数；`None` = 没抢到单飞权（已代为置 `pending`）。
///
/// 为什么需要它：改造前每个事件都 `spawn` 一个线程独立重建菜单，两个事件撞车时
/// 「后写覆盖」——较慢的那次会用**稍旧的一份**菜单盖掉较新的一份。且
/// `config-changed` 一次会连做图标 + 菜单 + 设备缓存 + tooltip，与
/// `audio-devices-changed` 撞车时整棵菜单（含 COM 枚举）会被重建两次。
///
/// **残留窗口（已知且接受）**：请求的 `pending` 写入若恰好落在持权者
/// 「最后一次 `swap` 返回 false」与「守卫 Drop」之间的纳秒级窗口内，该请求
/// 既不会被补跑、也不会有人再检查。后果与改造前一致（菜单滞后一拍，
/// 下次事件即纠正），但窗口已从「整轮重建耗时（含 COM 枚举，数十 ms）」
/// 缩到「两次原子操作之间」。
pub(crate) fn run_coalesced(
    running: &AtomicBool,
    pending: &AtomicBool,
    mut job: impl FnMut(),
) -> Option<usize> {
    let Some(_guard) = SingleFlightGuard::new(running) else {
        pending.store(true, Ordering::SeqCst);
        return None;
    };
    // 认领：清掉启动前积压的请求——本轮 `job` 读到的就是最新状态，
    // 故这些请求已被本轮满足，无需再补跑。
    pending.store(false, Ordering::SeqCst);
    let mut rounds = 0usize;
    loop {
        job();
        rounds += 1;
        // 收尾检查：本轮执行期间到达的请求 → 再跑一轮
        if !pending.swap(false, Ordering::SeqCst) {
            return Some(rounds);
        }
    }
}

/// 设备缓存，用于托盘 tooltip 显示，避免重复 WMI 查询
static DEVICES_CACHE: OnceLock<Mutex<Vec<Device>>> = OnceLock::new();

/// 获取设备缓存的引用
pub fn get_devices_cache() -> &'static Mutex<Vec<Device>> {
    DEVICES_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    /// 无竞争：只跑一轮，且结束后单飞权必须已释放（否则后续所有重建都会被永久吞掉）
    #[test]
    fn coalesced_runs_once_when_uncontended() {
        let running = AtomicBool::new(false);
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, Some(1));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!running.load(Ordering::SeqCst), "守卫必须已释放单飞权");
        assert!(
            !pending.load(Ordering::SeqCst),
            "正常收尾后不应残留 PENDING"
        );
    }

    /// 已被占用：立即返回 `None`、不执行 job、**但必须置 PENDING**——
    /// 漏了这一步就是「丢更新」，是本条修复最容易写错的方向。
    #[test]
    fn coalesced_defers_when_busy() {
        let running = AtomicBool::new(true); // 模拟另一轮正在进行
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, None, "抢不到单飞权应返回 None");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "抢不到时不得执行 job");
        assert!(
            pending.load(Ordering::SeqCst),
            "抢不到时必须置 PENDING，否则本次请求丢失"
        );
    }

    /// 启动前积压的请求被「认领」合并进本轮：只跑一轮而不是两轮。
    /// 这是合并（coalescing）相对「排队」的价值所在——避免无谓的重复重建。
    #[test]
    fn coalesced_absorbs_backlog_before_start() {
        let running = AtomicBool::new(false);
        let pending = AtomicBool::new(true); // 上一轮遗留 / 抢权失败者置的
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, Some(1), "积压请求应被本轮合并，无需补跑");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(!pending.load(Ordering::SeqCst));
    }

    /// 执行期间到达的请求必须触发**补跑一轮**（这是「不丢更新」的核心断言）
    #[test]
    fn coalesced_reruns_when_request_arrives_during_job() {
        let running = AtomicBool::new(false);
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let rounds = run_coalesced(&running, &pending, || {
            // 模拟「job 执行期间另一个线程抢权失败、置了 PENDING」
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                pending.store(true, Ordering::SeqCst);
            }
        });
        assert_eq!(rounds, Some(2), "执行期间到达的请求应触发补跑");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    /// 并发场景：主线程先占住单飞权，8 个线程同时请求 —— 全部应立即返回 `None`
    /// 并只把 PENDING 置起（不各自开一轮），释放后由持权者合并成一轮。
    /// 这条钉的是「多个事件撞车时不再重建多次」这一 B3 的核心收益。
    #[test]
    fn concurrent_requests_do_not_each_rebuild() {
        let running = AtomicBool::new(true); // 主线程占位，等价于「一轮正在进行」
        let pending = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| {
                    let r = run_coalesced(&running, &pending, || {
                        calls.fetch_add(1, Ordering::SeqCst);
                    });
                    assert_eq!(r, None, "单飞被占用时调用方应立即返回");
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 0, "并发请求期间不得执行 job");
        assert!(pending.load(Ordering::SeqCst));

        // 释放单飞权：由下一轮把 8 个请求合并成 1 轮
        running.store(false, Ordering::SeqCst);
        let rounds = run_coalesced(&running, &pending, || {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(rounds, Some(1), "8 个请求应合并为 1 轮重建");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
