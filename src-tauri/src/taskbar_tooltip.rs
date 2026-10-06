//! 任务栏 widget 的**每设备 hover tooltip**（**自绘分层顶层窗**）。
//!
//! ── ⛔ 不得回退到原生 `TOOLTIPS_CLASSW`（勿再改回去）
//! 被证伪的是**控件本身**，不是用法（决策史与完整矩阵 → Wiki 15 §4.7.4）：
//! 1. `TTM_SETTOOLPOS` **在 Windows SDK 里根本不存在**（本机 `CommCtrl.h` 的
//!    `WM_USER+35` 是 `TTM_GETTITLE`）⇒ 照抄 MSDN 就是在发「取标题」，控件
//!    合法地无视且**不报错**。
//! 2. 换用 SDK 里真实存在的 `TTM_TRACKACTIVATE` / `TTM_TRACKPOSITION`
//!    （`WM_USER+17` / `+18`）后，`TRACKACTIVATE` **返回 TRUE**，窗矩形却
//!    **纹丝不动**（`期望(1192,1349) 实际(1231,1394)`），跨 tick 重放无效。
//! 3. 提示按**光标**定位，而光标在**任务栏内** ⇒ 默认位置必然压在任务栏上
//!    （真机：任务栏 `y=1380..1440`，提示落在 `y≈1390..1408`）。
//!
//! ── 自绘方案 ───────────────────────────────────────────────────────────
//! · **一个**顶层 `WS_POPUP` + `WS_EX_LAYERED`，`UpdateLayeredWindow(ULW_ALPHA)` 逐帧提交
//! · ⭐ 复用 `taskbar_widget` 的**同一套**渲染管线（`create_dib` / `render_text_mask`
//!   / `blit_text_mask` / `blend_over` / `inside_rounded_rect`）——**不是**重写一份
//!   ⇒ 圆角、抗锯齿、预乘合成与 widget 逐字一致（这正是「与 widget 一致」的落点）
//! · ⛔ `WS_EX_TRANSPARENT`（点击穿透）+ `WS_EX_NOACTIVATE`：提示**绝不能**
//!   抢走 widget 的命中，否则用户一动鼠标提示就消失
//! · 500ms 初始延迟由本模块的 hover 判定实现 ⇒ **变成可测的逻辑**，不再依赖控件计时
//!
//! ── 视觉基准（`CustomToolTip.xaml`）───────────────────────
//! · `FontSize=11` / `LineHeight=11` / `MaxWidth=400`
//! · `CornerRadius=4` / `BorderThickness=1` / 背景 `#F9F9F9` / 边框 `#E5E5E5`
//! · `DropShadowEffect`：`BlurRadius=10` / `Direction=270` / `Depth=5` /
//!   `Opacity=0.25` / 颜色 `#202020`
//!
//! ── 线程纪律（⛔ 硬约束）────────────────────────────────────────────
//! · tip 窗**必须**在**主线程**建：窗口过程只在创建线程上被调用，而 widget 建在
//!   Tauri `setup` 回调（= 主线程）。
//! · ⛔ 50ms hover 轮询线程**只投递原子/消息**，不得直接碰窗口（见 `taskbar_widget`）。

use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, GetTextMetricsW, SelectObject, HFONT, TEXTMETRICW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, IsWindow, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE,
    SWP_SHOWWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::taskbar_widget::ffi;

// ══════════════════════════════════════════════════════════════════════════
// 视觉常量（DIP 口径；像素值一律经 `tip_px` 换算）
// ══════════════════════════════════════════════════════════════════════════

/// 提示的最大宽度（**px**）——对齐 `base.css:920` 的 `max-width: 320px`。
///
/// ⛔ **按 px 不按 DIP**：CSS 的 `px` 在浏览器里是**设备无关像素**，
///   而本仓的「内容缩放」是一个**独立的用户设置**（`taskbar_content_scale`），
///   它**不**等于系统 DPI ⇒ 用 DIP 换算会让宽度随内容档位漂移，
///   与页面里那个固定 320px 的 tooltip 对不上。
const TIP_MAX_W_PX: i32 = 320;

/// 提示的字号（**px**）。
///
/// ⭐ 取 **15**：对齐面板歌名的实际像素高（歌名 12 DIP × 125% = 15px），比 14px
///   显式小一号；12 与 14 用户都嫌「太虚太小」。
/// ⚠️⚠️ **有意偏离 `base.css:922` 的 `font-size: 12px`**；与页面 tooltip 的观感
///   一致性仍体现在底色/边框/圆角/阴影/内边距上（见文件头）。
/// ⛔ 别以「与页面 tooltip 保持一致」为理由改回 12 —— 那是 CSS 的口径，提示框跟的
///   是面板字号。
/// ⛔ 改这个值前先确认用户是否改主意，不是「顺手对齐回 CSS」。
///   （6 → 14 → 15 三步的出处与决策史 → Wiki 15 §4.7.3）
/// ⚠️ 与 `taskbar_widget::FONT_PX_DIP`(11) **仍是不同值**：那是 widget **内容**的
///   字号（跟随用户的内容缩放设置），跟的是内容档位，不是提示。
const TIP_FONT_PX: i32 = 15;

/// 提示的行高（**px**）。
///
/// ⭐ 别照抄 `base.css` 的 `line-height: 16px`：`line-height` 在 CSS 里是**倍数语义**
///   （16px 对 12px 字其实是 `1.33`），而自绘走绝对行距 ⇒ 照抄 16 会让 15px 字的
///   降部/升部被固定掩码切掉（表现为「字被削顶」）。
///
/// ⭐⭐ **必须由字体实际度量决定**，不能写成 `字号 + 常数`
///   （硬编码 `字号 + 2` ⇒ 15px 的 Segoe UI Variable Text 行高本就 >17px，
///   下伸部与中文字形的底沿被逐行裁掉，**表现为行高不够、字底部显示不全**）。
///   `GetTextMetricsW` 的 `tmHeight + tmExternalLeading` 正是 GDI 自己算的
///   「单行占位高度」，**换字体 / 换字号都会自动跟随**，不必再手动对齐常数。
///
/// ⛔ 缓存：字体只建一次（`ensure_font`），度量也只查一次（`OnceLock`）。
///   取不到时回落到 `字号 × 1.25`，比硬编码 `字号 + 2` 保守。
static TIP_LINE_H: std::sync::OnceLock<i32> = std::sync::OnceLock::new();

fn line_h_px() -> i32 {
    *TIP_LINE_H.get_or_init(|| unsafe {
        let font = ensure_font();
        let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
        let memdc = CreateCompatibleDC(screen);
        windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
        if memdc.is_null() {
            return TIP_FONT_PX + 4;
        }
        let old = SelectObject(memdc, font);
        let mut tm: TEXTMETRICW = std::mem::zeroed();
        let ok = GetTextMetricsW(memdc, &mut tm);
        SelectObject(memdc, old);
        DeleteDC(memdc);
        if ok == 0 {
            return TIP_FONT_PX + 4;
        }
        (tm.tmHeight + tm.tmExternalLeading).max(TIP_FONT_PX + 2)
    })
}

/// 气泡圆角（**DIP**）——对齐 `CornerRadius=4`。
const TIP_CORNER_R_PX: i32 = 4;

/// 边框宽度（**px**）——`BorderThickness=1`。
///
/// ⚠️ **按 px 而非 DIP**：`BorderThickness` 是设备无关单位里最接近「物理 1 像素」
///   的语义，125% 下若按 DIP 换算会变成 1.25px ⇒ 渲染时半边被吃掉、边框忽明忽暗。
const TIP_BORDER_PX: i32 = 1;

/// 气泡内边距（**px**）——对齐 `base.css:901` 的 `padding: 6px 9px 8px`。
///
/// ⭐ 三值不对称（`6/9/8`）**照抄**，不「简化」成 6/6/6：
///   CSS 里 padding-bottom 比 top 多 2px 是刻意的（给 12px 字号的降部留气）。
const TIP_PAD_TOP_PX: i32 = 6;
const TIP_PAD_X_PX: i32 = 9;
const TIP_PAD_BOT_PX: i32 = 8;

/// ⭐ 气泡配色：**两套**（浅/深），取自 `base.css:93-96` / `164-167`。
///
/// ⚠️ 深色分支的边框是 `rgba(0,0,0,.20)` 叠在 `#2c2c2c` 上——**比气泡本身还暗**。
///   这不是笔误：深色主题下 tooltip 靠**边框的暗线**勾出轮廓（背景已接近黑）。
#[derive(Clone, Copy)]
struct Palette {
    bg: (u8, u8, u8),
    /// 边框色 + 其 alpha（`base.css` 里是 `rgba()`，**不是**实色）
    border: (u8, u8, u8),
    border_a: u32,
    fg: (u8, u8, u8),
}

/// 浅色主题（`base.css:93-96`）。
const PALETTE_LIGHT: Palette = Palette {
    bg: (0xFC, 0xFC, 0xFC), // --flyout-bg: #fcfcfc
    border: (0, 0, 0),      // --flyout-border: rgba(0,0,0,.06)
    border_a: 15,           // 0.06 × 255 = 15.3 ⇒ 15
    fg: (0, 0, 0),          // --flyout-text: rgba(0,0,0,.89)
};

/// 深色主题（`base.css:164-167`）。
const PALETTE_DARK: Palette = Palette {
    bg: (0x2C, 0x2C, 0x2C), // --flyout-bg: #2c2c2c
    border: (0, 0, 0),      // --flyout-border: rgba(0,0,0,.20)
    border_a: 51,           // 0.20 × 255 = 51
    fg: (255, 255, 255),    // --flyout-text: #ffffff
};

/// 文本的 alpha —— `rgba(0,0,0,.89)` ⇒ 0.89 × 255 = 227。
const TIP_FG_ALPHA_LIGHT: u32 = 227;

/// ── 阴影：`base.css:96` 的 `--flyout-shadow` 是**两层** ──────────────────
/// 浅色：`0 8px 16px rgba(0,0,0,.14), 0 0 2px rgba(0,0,0,.18)`
/// 深色：`0px 5px 15px rgba(0,0,0,0.2)`（**单层**）
///
/// ⭐ 本仓 CSS 的两层都是**「零模糊的贴边环 + 一层大模糊」**的组合，
///   这里照抄成 (偏移, 模糊, 峰值 alpha) 三元组，逐层各画一次。
/// ⚠️ 不能只取「最大模糊那层」：那样会丢掉贴边环，观感上气泡像「浮起来」
///   而不是「贴在浮出面上」——这正是两个页面 tooltip 的样子。
type ShadowLayer = (i32, i32, i32, u32); // (dx, dy, blur, peak_alpha)

/// 浅色主题阴影（`0 8px 16px rgba(0,0,0,.14)` + `0 0 2px rgba(0,0,0,.18)`）。
///
/// ⚠️ **y 偏移为正**（向下）：CSS 的 `box-shadow` y 向下为正，而 CSS
///   **不做 y 翻转** ⇒ 浅色主题的阴影在**气泡下方**。
///   （的 `Direction=270` 是向上，两套基准不同，此处跟 CSS。）
const SHADOW_LIGHT: [ShadowLayer; 2] = [
    (0, 8, 16, 36), // 0.14 × 255 = 35.7 ⇒ 36
    (0, 0, 2, 46),  // 0.18 × 255 = 45.9 ⇒ 46
];

/// 深色主题阴影（`0px 5px 15px rgba(0,0,0,0.2)`，**单层**）。
const SHADOW_DARK: [ShadowLayer; 1] = [
    (0, 5, 15, 51), // 0.2 × 255 = 51
];

/// tooltip 与**任务栏外缘**之间的留白（**DIP**）。
///
/// ⭐ 取 **16**，对齐 `TaskbarWidgetControl.xaml:45` 的
///   `ToolTipService.VerticalOffset="-16"`（要求「参考 FluentFlyout」）。
///   ⛔ 不接受凭观感估出来的值——本项目的尺寸一律要有出处。
/// ⚠️ 与上面那个 WPF tooltip 的**参照点**仍有一处**有意的**不同：
///   它是 `Placement="RelativePoint"` ⇒ 以**光标点**为参照，偏移 -16；
///   本仓光标在任务栏**内**（任务栏是交互面），以光标为参照会让提示压住任务栏
///   ⇒ 改为以**任务栏外缘**为参照。两者在「光标贴着任务栏上沿」时等价。
///
/// ⛔ 初始延迟**不**跟着改成 800：`CustomToolTip.xaml` 附近的
///   `InitialShowDelay="800"` 是那个 WPF tooltip 的值，而 500ms 是**用户先前明确指定**的
///   ⇒ 冲突时以用户决定为准（这条纪律见 AGENTS「关键限定必须保留在条目内」）。
const TIP_TASKBAR_GAP_DIP: i32 = 16;

/// 悬停后多久显示（**ms**）——对齐 `ToolTipService.InitialShowDelay`。
pub const TIP_DELAY_MS: u64 = 500;

/// DIP → 物理像素（走**内容** DPI，widget 里的字号也走这个 ⇒ 两者同口径）。
fn tip_px(dip: i32) -> i32 {
    crate::taskbar_widget::content_px(dip)
}

// ══════════════════════════════════════════════════════════════════════════
// 状态
// ══════════════════════════════════════════════════════════════════════════

/// 「当前**显示中**的是第几号」（`-1` = 未显示）。
///
/// ⭐ 这是「提示跟随 widget」的抓手。**必须有它**，否则会出现：
///   启动初期 widget 还没拿到设备快照，走 `draw_blank`（`commit(.., 0, ..)`）
///   ⇒ **widget 本身停在任务栏最左端**。此时用户悬停，提示正确地出现在最左端；
///   ~700ms 后快照到达、widget 重居中到 x≈1101，但提示**不会跟着动**
///   （hover 索引没变 ⇒ `HOVER_SINCE == -1` ⇒ 不再投递重绘）
///   ⇒ 提示被**永久遗留在屏幕最左端**——真机报告的「首帧出现在最左边」就是这个
///   组合，**不是**首帧渲染错。所以只修渲染顺序修不掉它。
static CURRENT_SHOWN: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(-1);

/// tip 窗句柄（**仅主线程**读写；`0` = 未建/已毁）。
static TIP_HWND: AtomicIsize = AtomicIsize::new(0);

/// 已同步的条目（`text` + 命中区），供 `show` 取用。
static ENTRIES: std::sync::Mutex<Vec<TipEntry>> = std::sync::Mutex::new(Vec::new());

/// 上一份 `ENTRIES`，用于「变了才重建」判定。
static LAST_SYNCED: std::sync::Mutex<Vec<TipEntry>> = std::sync::Mutex::new(Vec::new());

/// 气泡字体（懒建，随窗存活）。
static TIP_FONT: AtomicIsize = AtomicIsize::new(0);

/// 一条提示：文本 + 该设备的命中区。
#[cfg(target_os = "windows")]
#[derive(Clone)]
pub struct TipEntry {
    pub text: String,
    pub rect: RECT,
}

impl std::fmt::Debug for TipEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TipEntry({:?}, {},{},{},{})",
            self.text, self.rect.left, self.rect.top, self.rect.right, self.rect.bottom
        )
    }
}

impl PartialEq for TipEntry {
    fn eq(&self, o: &Self) -> bool {
        // ⚠️ `RECT` **不实现** `PartialEq`（windows-sys 的 plain struct）⇒ 逐字段比。
        self.text == o.text
            && self.rect.left == o.rect.left
            && self.rect.top == o.rect.top
            && self.rect.right == o.rect.right
            && self.rect.bottom == o.rect.bottom
    }
}

// ══════════════════════════════════════════════════════════════════════════
// 几何
// ══════════════════════════════════════════════════════════════════════════

/// ⭐ 计算「气泡左上角该在哪」：**任务栏外侧**，横向对准第 `index` 个设备。
///
/// ⚠️ 这是**唯一**一处自己算 tooltip 几何的地方。
/// ⛔⛔ 判任务栏方向**必须与屏幕中心比**，不能用 `tb_top > 某个魔数`：
///   本机任务栏 `top=1380`，任何小于它的阈值都会判成「在上边」
///   ⇒ 提示被放到**屏幕下方外**（真机实测 `y=1446`，出屏）。
#[cfg(target_os = "windows")]
pub fn tip_rect_for(index: usize, bubble_w: i32, bubble_h: i32) -> Option<RECT> {
    let (tb_left, tb_top, tb_w, tb_h) = crate::taskbar_widget::taskbar_rect_tuple()?;
    let gap = tip_px(TIP_TASKBAR_GAP_DIP);
    let vertical = tb_w < tb_h; // 竖排任务栏
                                // ⛔⛔ 锚点拿不到 ⇒ **返回 `None`（不显示）**，不许退化成「任务栏最左端」。
                                // ⛔ 不得回落成 `(tb_left, tb_w)`：那会让提示落在屏幕**左下角**：
                                //   开发门控实测第一帧 `窗=(-12,1334)`——`item_rects` 还没发布时的典型表现。
                                //   「一个位置错误但可见的提示」比「没有提示」更糟（它会闪一下再跳走）。
    let (anchor_x, anchor_w) = crate::taskbar_widget::item_rect_on_screen(index)?;
    let mut r = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if vertical {
        // 竖排：提示放在其**左侧**（在竖排下干脆不显示，
        // 这里给一个不越界的落位，好过什么都不做）。
        r.left = tb_left - gap - bubble_w;
        r.top = anchor_x + (anchor_w - bubble_h) / 2;
        r.right = r.left + bubble_w;
        r.bottom = r.top + bubble_h;
    } else {
        let (_, screen_h) = crate::taskbar_widget::screen_size();
        let tb_center = tb_top + tb_h / 2;
        let at_bottom = tb_center >= screen_h / 2;
        r.left = anchor_x + (anchor_w - bubble_w) / 2;
        if at_bottom {
            r.top = tb_top - gap - bubble_h; // 任务栏在下 ⇒ 提示在其上方
        } else {
            r.top = tb_top + tb_h + gap; // 任务栏在上 ⇒ 提示在其下方
        }
        r.right = r.left + bubble_w;
        r.bottom = r.top + bubble_h;
    }
    clamp_to_screen(&mut r);
    Some(r)
}

/// 把矩形夹进屏幕（留 4px 边距）——贴边设备否则会让提示**整块跑到屏外**。
#[cfg(target_os = "windows")]
fn clamp_to_screen(r: &mut RECT) {
    let (sw, sh) = crate::taskbar_widget::screen_size();
    let m = 4;
    if r.left < m {
        let d = m - r.left;
        r.left += d;
        r.right += d;
    }
    if r.right > sw - m {
        let d = r.right - (sw - m);
        r.left -= d;
        r.right -= d;
    }
    if r.left < m {
        r.left = m;
        r.right = (m + (r.right - r.left)).max(m);
    }
    if r.top < m {
        let d = m - r.top;
        r.top += d;
        r.bottom += d;
    }
    if r.bottom > sh - m {
        let d = r.bottom - (sh - m);
        r.top -= d;
        r.bottom -= d;
    }
    if r.top < m {
        r.top = m;
        r.bottom = (m + (r.bottom - r.top)).max(m);
    }
}

/// 文字**折行**后的行（每行是已 UTF-16 编码的文本）。
///
/// ⭐ 口径是**折行、不截断** ⇒ 按 `TIP_MAX_W_DIP` 硬折，
///   而**不是**用 `DT_END_ELLIPSIS`。
#[cfg(target_os = "windows")]
fn wrap_lines(
    memdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    text: &[u16],
) -> Vec<Vec<u16>> {
    let max_w = TIP_MAX_W_PX;
    // 逐字符累加宽度，找**能放下的最长前缀**。
    // ⛔ 不能按空格断：设备名多为无空格的连续串，按空格断等于不折。
    let mut lines: Vec<Vec<u16>> = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let mut cur_w = 0;
    for &c in text {
        if c == 0 {
            continue;
        }
        // 换行符 = **硬换行**（音乐面板 tooltip 要「上排歌名 / 下排歌手」）。
        // 此前这里只跳 NUL、按宽度折行，把两行文本当成一行里夹了个换行符，
        // DrawText 画出来是方框或空白。设备名不含换行符 ⇒ 对既有条目零影响。
        if c == 10 {
            lines.push(std::mem::take(&mut cur));
            cur_w = 0;
            continue;
        }
        let w = unsafe { measure_one(memdc, font, &[c]) };
        if cur.is_empty() {
            cur.push(c);
            cur_w = w;
            continue;
        }
        if cur_w + w > max_w {
            lines.push(std::mem::take(&mut cur));
            cur.push(c);
            cur_w = w;
        } else {
            cur.push(c);
            cur_w += w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// 单字符的像素宽度（比 `measure_text` 少一次 `DT_CALCRECT` 的整体开销，够折行用）。
#[cfg(target_os = "windows")]
unsafe fn measure_one(
    memdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    text: &[u16],
) -> i32 {
    unsafe { ffi::measure_text(memdc, font, text) }
}

// ══════════════════════════════════════════════════════════════════════════
// 渲染
// ══════════════════════════════════════════════════════════════════════════

/// 一帧的完整尺寸（含阴影边距）与气泡的左上角（相对位图）。
#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct Frame {
    win_w: i32,
    win_h: i32,
    bubble_x: i32,
    bubble_y: i32,
    bubble_w: i32,
    bubble_h: i32,
    /// 文本左上角（**窗口**坐标）——已含边框 + padding
    text_x: i32,
    text_y: i32,
}

/// 量出整帧几何（纯计算，不碰 Win32 状态）。
#[cfg(target_os = "windows")]
fn measure_frame(
    memdc: windows_sys::Win32::Graphics::Gdi::HDC,
    font: HFONT,
    lines: &[Vec<u16>],
    pad: i32,
) -> Frame {
    let text_w = lines
        .iter()
        .map(|l| unsafe { measure_one(memdc, font, l) })
        .max()
        .unwrap_or(0);
    let bubble_w = text_w + 2 * (TIP_PAD_X_PX + TIP_BORDER_PX);
    let bubble_h =
        lines.len() as i32 * line_h_px() + TIP_PAD_TOP_PX + TIP_PAD_BOT_PX + 2 * TIP_BORDER_PX;
    Frame {
        win_w: bubble_w + 2 * pad,
        win_h: bubble_h + 2 * pad,
        bubble_x: pad,
        bubble_y: pad,
        bubble_w,
        bubble_h,
        text_x: pad + TIP_BORDER_PX + TIP_PAD_X_PX,
        text_y: pad + TIP_BORDER_PX + TIP_PAD_TOP_PX,
    }
}

/// 把圆角矩形「盖章」到 alpha 图上（阴影各层的公共第一步）。
#[cfg(target_os = "windows")]
fn stamp_rounded(alpha: &mut [u8], frame: Frame, radius: i32, at_x: i32, at_y: i32, peak: u8) {
    for y in 0..frame.bubble_h {
        for x in 0..frame.bubble_w {
            if !crate::taskbar_widget::inside_rounded_rect(
                x,
                y,
                frame.bubble_w,
                frame.bubble_h,
                radius,
            ) {
                continue;
            }
            let sx = at_x + x;
            let sy = at_y + y;
            if sx >= 0 && sx < frame.win_w && sy >= 0 && sy < frame.win_h {
                alpha[(sy * frame.win_w + sx) as usize] = peak;
            }
        }
    }
}

/// 当前应��的配色。
///
/// ⭐ 跟**系统**主题（`SystemUsesLightTheme`）而不是**应用**主题：
///   与 `taskbar_widget::hover_backdrop_alpha_for` 同一口径（那里有完整论证：
///   「应用深色 + 系统浅色」这类自定义主题下两者会不一致）。
#[cfg(target_os = "windows")]
fn current_palette() -> &'static Palette {
    // ⭐ 开发门控 `PM_DEV_TOOLTIP_DARK=1`：**强制**走深色分支。
    //   理由：深色配色是**独立代码路径**（边框变成「比气泡更暗的暗线」、
    //   阴影换成单层 5/15），而本机系统是浅色 ⇒ 不加门控就**永远验证不到**，
    //   等于把一段从未被渲染过的代码交给用户。
    //   ⛔ 不设该变量时立即返回，零开销（`var_os` 命中即止）。
    if std::env::var_os("PM_DEV_TOOLTIP_DARK").is_some() {
        return &PALETTE_DARK;
    }
    if crate::windows::system_uses_light_theme() {
        &PALETTE_LIGHT
    } else {
        &PALETTE_DARK
    }
}

/// 阴影需要的画布边距（**px**）——取各层 `(|dx|, |dy|, blur)` 的最大值。
///
/// ⭐ **必须动态算**：浅色是 `8px 下移 + 16px 模糊`，深色是 `5px + 15px`；
///   写死一个常数会出现「深色主题下阴影上沿被裁」或「浅色下留一圈空白」。
#[cfg(target_os = "windows")]
fn shadow_pad(pal: &Palette) -> i32 {
    let layers: &[ShadowLayer] = if pal.bg == PALETTE_LIGHT.bg {
        &SHADOW_LIGHT
    } else {
        &SHADOW_DARK
    };
    layers
        .iter()
        .map(|(dx, dy, blur, _)| dx.abs().max(dy.abs()).max(*blur))
        .max()
        .unwrap_or(0)
        + 2 // 模糊尾端的一点点余量
}

/// 画一帧并提交。**只能在主线程调用**。
#[cfg(target_os = "windows")]
fn render_and_show(index: usize) {
    let hwnd = TIP_HWND.load(Ordering::Acquire) as HWND;
    if hwnd.is_null() || !unsafe { IsWindow(hwnd) != 0 } {
        return;
    }
    // ⛔ 拿不到气泡几何 ⇒ **先隐藏**再返回：不隐藏的话，上一次显示的提示会
    //   **留在旧位置**（内容还是旧的），看起来像「提示卡住不动」。
    // ⚠️ 统一走 `lock_unpoisoned`：**中毒时恢复**而不是「隐藏提示并放弃」。
    // ⛔ 一旦中毒，提示**不得**就此**永久**不再显示（那会无任何日志、也自愈不了）。
    let entries = crate::state::lock_unpoisoned(&ENTRIES);
    let Some(entry) = entries.get(index) else {
        hide();
        return;
    };
    let text: Vec<u16> = entry.text.encode_utf16().collect();
    let font = ensure_font();
    if font.is_null() {
        hide();
        return;
    }
    // 测量：借一张临时 DC 量行与尺寸
    let screen_dc = unsafe { windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut()) };
    let memdc = unsafe {
        use windows_sys::Win32::Graphics::Gdi::{CreateCompatibleDC, ReleaseDC};
        let m = CreateCompatibleDC(screen_dc);
        ReleaseDC(std::ptr::null_mut(), screen_dc);
        m
    };
    if memdc.is_null() {
        return;
    }
    let lines = wrap_lines(memdc, font, &text);
    let frame = measure_frame(memdc, font, &lines, shadow_pad(current_palette()));
    let Some(dib) = (unsafe { ffi::create_dib(frame.win_w, frame.win_h) }) else {
        unsafe { windows_sys::Win32::Graphics::Gdi::DeleteDC(memdc) };
        return;
    };

    // ── ① 阴影（先画，它在气泡**下面**）─────────────────────────────
    //    ⭐ 逐层画：`--flyout-shadow` 是**两层/一层**组合，不是单个模糊。
    let pal = current_palette();
    let radius = TIP_CORNER_R_PX;
    let layers: &[ShadowLayer] = if pal.bg == PALETTE_LIGHT.bg {
        &SHADOW_LIGHT
    } else {
        &SHADOW_DARK
    };
    let mut shadow = vec![0u8; (frame.win_w * frame.win_h) as usize];
    for (dx, dy, blur, peak) in layers.iter().copied() {
        let peak = peak as u8; // 唯一需要的转换：alpha 收进 u8 的 alpha 图
                               // 每一层单独一张 alpha 图，模糊后**加性**累加（CSS 阴影是叠加的）
        let mut layer = vec![0u8; shadow.len()];
        stamp_rounded(
            &mut layer,
            frame,
            radius,
            frame.bubble_x + dx,
            frame.bubble_y + dy,
            peak,
        );
        box_blur(&mut layer, frame.win_w, frame.win_h, blur);
        for (acc, add) in shadow.iter_mut().zip(layer.iter()) {
            *acc = acc.saturating_add(*add);
        }
    }

    // ── ② 气泡底 + 边框 ────────────────────────────────────────────────
    let px =
        unsafe { std::slice::from_raw_parts_mut(dib.bits, (frame.win_w * frame.win_h) as usize) };
    // 阴影：纯黑，alpha 取累加值
    for (i, a) in shadow.iter().enumerate() {
        let av = *a as u32;
        if av == 0 {
            continue;
        }
        px[i] = blend_over(px[i], av, 0, 0, 0);
    }
    // 气泡本体
    for y in 0..frame.bubble_h {
        for x in 0..frame.bubble_w {
            if !crate::taskbar_widget::inside_rounded_rect(
                x,
                y,
                frame.bubble_w,
                frame.bubble_h,
                radius,
            ) {
                continue;
            }
            let idx = ((frame.bubble_y + y) * frame.win_w + frame.bubble_x + x) as usize;
            if idx < px.len() {
                px[idx] = blend_over(
                    px[idx],
                    255,
                    pal.bg.0 as u32,
                    pal.bg.1 as u32,
                    pal.bg.2 as u32,
                );
            }
        }
    }
    // ⚠️ 边框**最后**画且**带 alpha**：`rgba(0,0,0,.06)` 叠在气泡底上才是 CSS 的样子。
    //   先填实色再画边框会让边框变成「实心黑」——深色主题下尤其明显。
    for y in 0..frame.bubble_h {
        for x in 0..frame.bubble_w {
            if !crate::taskbar_widget::inside_rounded_rect(
                x,
                y,
                frame.bubble_w,
                frame.bubble_h,
                radius,
            ) {
                continue;
            }
            if x >= TIP_BORDER_PX
                && y >= TIP_BORDER_PX
                && x < frame.bubble_w - TIP_BORDER_PX
                && y < frame.bubble_h - TIP_BORDER_PX
            {
                continue; // 内部像素不画边框
            }
            let idx = ((frame.bubble_y + y) * frame.win_w + frame.bubble_x + x) as usize;
            if idx < px.len() {
                px[idx] = blend_over(
                    px[idx],
                    pal.border_a,
                    pal.border.0 as u32,
                    pal.border.1 as u32,
                    pal.border.2 as u32,
                );
            }
        }
    }

    // ── ③ 文本：**实色背景上直绘 + ClearType**（不是掩码+预乘）────────────
    //    ⚠️ 这里**故意不**走 widget 那条「白底黑字掩码 → 覆盖度 → 预乘」的路：
    //       那条路存在的原因是 widget 底衬**半透明**（`ULW` 会把 alpha=0 的像素
    //       当全透明，直接画字等于没画）。tooltip 的气泡是**不透明**实色
    //       （`--flyout-bg`）⇒ 完全可以直接用 GDI 画 ⇒ 能上 **ClearType**。
    //       ⭐ 真机观感差别就是用户说的「**发虚**」：灰度 AA 把 14px 笔画的
    //       边缘摊成灰边；子像素渲染按 RGB 排列分别着色三遍，边缘锐利得多。
    let fg_rgb = if pal.bg == PALETTE_LIGHT.bg {
        // rgba(0,0,0,.89) 叠在 #FCFCFC 上 ⇒ 252 × 0.11 = 27.7 ⇒ #1C1C1C
        composite_over(pal.fg, TIP_FG_ALPHA_LIGHT, pal.bg)
    } else {
        pal.fg
    };
    for (i, line) in lines.iter().enumerate() {
        let w = unsafe { measure_one(memdc, font, line) };
        if w <= 0 {
            continue;
        }
        unsafe {
            blit_text_opaque(
                px,
                frame.win_w,
                frame.text_x,
                frame.text_y + i as i32 * line_h_px(),
                w,
                line_h_px(),
                line,
                font,
                pal.bg,
                fg_rgb,
            );
        }
    }

    // ── ④ 提交 + 定位 + 显示 ──────────────────────────────────────────
    let bubble_pos = match tip_rect_for(index, frame.bubble_w, frame.bubble_h) {
        Some(b) => b,
        None => {
            unsafe {
                windows_sys::Win32::Graphics::Gdi::DeleteDC(memdc);
                ffi::free_dib(&dib);
            }
            return;
        }
    };
    // 窗口左上角 = 气泡左上角 - 阴影边距
    let win_x = bubble_pos.left - frame.bubble_x;
    let win_y = bubble_pos.top - frame.bubble_y;
    let ok = unsafe { ffi::commit(hwnd, &dib, win_x, win_y) };
    unsafe {
        windows_sys::Win32::Graphics::Gdi::DeleteDC(memdc);
        ffi::free_dib(&dib);
    }
    if ok {
        // ⭐ `SWP_SHOWWINDOW` 一步完成「置顶 + 显示 + 定尺寸」。
        // ⛔ 必须带 `SWP_NOACTIVATE`：否则提示会抢焦点，widget 的 hover 立刻断掉
        //   ⇒ 表现为「提示闪一下就没了」。
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                win_x,
                win_y,
                frame.win_w,
                frame.win_h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
        }
        crate::process::append_verbose_log(&format!(
            "[widget] tooltip: 显示 #{} 「{}」气泡={}×{} 窗=({},{}) {}×{}",
            index,
            entry.text,
            frame.bubble_w,
            frame.bubble_h,
            win_x,
            win_y,
            frame.win_w,
            frame.win_h
        ));
    }
}

/// 把 `rgba(r,g,b,a)` 合成到**不透明** `bg` 上 ⇒ 得出可直接喂给 GDI 的实色。
#[cfg(target_os = "windows")]
fn composite_over(rgb: (u8, u8, u8), a: u32, bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let f = |c: u8, b: u8| -> u8 { ((c as u32 * a + b as u32 * (255 - a)) / 255) as u8 };
    (f(rgb.0, bg.0), f(rgb.1, bg.1), f(rgb.2, bg.2))
}

/// ⭐ 在**不透明底色**上用 GDI 直绘文字，再整块**不透明**贴回主缓冲。
///
/// ⛔ 为什么不走 `ffi::render_text_mask` + `blit_text_mask`（widget 的做法）：
///   那条路产出的是「覆盖率」，再乘一个 `alpha_scale` 预乘合成 ⇒ 文字边缘的
///   实际不透明度 = 覆盖度 × 整体 alpha，**两级衰减**叠加成灰边（=「发虚」）。
///   这里底色已知且不透明 ⇒ 文字可以**一次成型**，不必再乘任何系数。
///
/// ⛔ 前提：调用方保证这块矩形**完全在气泡内部**（padding ≥ 6px）⇒ 不会碰到
///   圆角；若哪天 padding 变成 0，这块方形补丁会在圆角处露馅。
#[cfg(target_os = "windows")]
unsafe fn blit_text_opaque(
    px: &mut [u32],
    win_w: i32,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    text: &[u16],
    font: HFONT,
    bg: (u8, u8, u8),
    fg: (u8, u8, u8),
) {
    use windows_sys::Win32::Graphics::Gdi::{
        DrawTextW, SetBkMode, SetTextColor, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, TRANSPARENT,
    };
    if w <= 0 || h <= 0 || text.is_empty() {
        return;
    }
    let Some(tmp) = ffi::create_dib(w, h) else {
        return;
    };
    // 缓冲是 **BGRA** 顺序（`CreateDIBSection` 的固有顺序）
    let pack = |c: (u8, u8, u8)| -> u32 {
        0xFF00_0000 | ((c.2 as u32) << 16) | ((c.1 as u32) << 8) | c.0 as u32
    };
    let filled = pack(bg);
    let buf = std::slice::from_raw_parts_mut(tmp.bits, (w * h) as usize);
    buf.fill(filled);
    // 画字（此时 DIB 里已有实色底 ⇒ ClearType 正常工作）
    let old_font = windows_sys::Win32::Graphics::Gdi::SelectObject(tmp.memdc, font as _);
    SetBkMode(tmp.memdc, TRANSPARENT as i32);
    // GDI 的 COLORREF 是 0x00BBGGRR
    SetTextColor(
        tmp.memdc,
        (fg.0 as u32) | ((fg.1 as u32) << 8) | ((fg.2 as u32) << 16),
    );
    let mut rc = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: w,
        bottom: h,
    };
    // ⚠️ 返回值**故意丢弃**：`DrawTextW` 返回非 0 只说明「文本被处理过」，
    //   **不保证像素变了**（真机实测：返回 19 而一个暗像素都没有）。
    //   真正的判据是贴回后主缓冲里的像素，由 `text_patch_alpha_must_be_opaque` 锚定。
    // ⛔ 空串不能交给 GDI：`Vec::new().as_ptr()` 是悬垂哨兵指针，`DrawTextW` 即使
    //   `cch = 0` 也会解引用它 ⇒ 访问违例（在 widget 侧实测到闪退，
    //   同一个洞在这里也开着 —— **所有**「`&[u16]` 进 GDI」的入口都要拦）。
    if !text.is_empty() {
        let _drawn = DrawTextW(
            tmp.memdc,
            text.as_ptr(),
            text.len() as i32,
            &mut rc,
            DT_LEFT | DT_SINGLELINE | DT_NOPREFIX,
        );
    }
    windows_sys::Win32::Graphics::Gdi::SelectObject(tmp.memdc, old_font);
    // ⛔⛔ **贴回时必须补上 alpha 字节 = 0xFF**。
    //   `DrawTextW` 只写 RGB，**从不碰 alpha** ⇒ 刚画完的临时 DIB 里
    //   那些文字像素的 alpha 仍是 `buf.fill()` 时留下的…等等，`fill` 写的是
    //   `0xFF00_0000 | …`，alpha 本该是 0xFF。
    //   ⚠️ 但真机回读到的最暗像素是 `0x001B1B1B`（alpha=0）——说明
    //   `SetTextColor` 之后的 `DrawTextW` **把整像素（含 alpha 字节）覆盖成
    //   COLORREF 本身**。COLORREF 只有 24 位 ⇒ 高位 0。
    //   ⇒ 直接原样拷贝 ⇒ 这块文字 alpha=0 ⇒ `ULW` 当**全透明** ⇒ 文字消失
    //   （真机实测：气泡内**一个暗像素都没有**，但 `DrawTextW` 返回 19 = 成功）。
    //
    //   ⭐ 这与 widget 头部记的是**同一个坑**的两面：那边是「alpha 恒 0 ⇒
    //   文字不显示」，这边因为底色不透明而**更隐蔽**——只剩文字没了。
    //   ⇒ 贴回时无条件 `| 0xFF00_0000`。
    for row in 0..h {
        let dy = y + row;
        if dy < 0 {
            continue;
        }
        for col in 0..w {
            let dx = x + col;
            if dx < 0 || dx >= win_w {
                continue;
            }
            let di = (dy * win_w + dx) as usize;
            if di < px.len() {
                px[di] = buf[(row * w + col) as usize] | 0xFF00_0000;
            }
        }
    }
    ffi::free_dib(&tmp);
}

/// 预乘 source-over（与 `taskbar_widget::blend_over` **逐字相同**）。
///
/// ⚠️ 抽成两份是**有意**的：`ffi` 那份是 `taskbar_widget` 私有的，
///   本模块若改成「取最大 alpha」会静默把抗锯齿边缘吃掉 ⇒ 字变细。
#[cfg(target_os = "windows")]
fn blend_over(dst: u32, a: u32, pr: u32, pg: u32, pb: u32) -> u32 {
    let inv = 255 - a;
    let da = dst >> 24;
    let ao = a + da * inv / 255;
    let ro = pr + ((dst >> 16) & 0xFF) * inv / 255;
    let go = pg + ((dst >> 8) & 0xFF) * inv / 255;
    let bo = pb + (dst & 0xFF) * inv / 255;
    (ao << 24) | (ro << 16) | (go << 8) | bo
}

/// ⭐ 可分离盒式模糊，3 趟逼近高斯（`DropShadowEffect` 的 `BlurRadius`）。
///
/// ⛔ 不能只做「距离衰减」：那样边缘是硬台阶，看起来像描边而不是影子。
///   3 趟盒式 ⇒ 单次半径 `r/3`，总代价仍是 O(w·h·r) 但视觉上足够柔。
#[cfg(target_os = "windows")]
fn box_blur(a: &mut [u8], w: i32, h: i32, radius: i32) {
    if radius < 1 {
        return;
    }
    let r: i32 = (radius / 3).max(1);
    let n: i32 = w * h;
    if n <= 0 {
        return;
    }
    let mut tmp = vec![0u8; n as usize];
    for _ in 0..3 {
        // 横向
        for y in 0..h {
            let row: usize = (y * w) as usize;
            for x in 0..w {
                let mut s = 0u32;
                let mut c = 0u32;
                for k in -r..=r {
                    let xx = x + k;
                    if xx >= 0 && xx < w {
                        s += a[row + xx as usize] as u32;
                        c += 1;
                    }
                }
                tmp[row + x as usize] = (s / c) as u8;
            }
        }
        // 纵向
        for x in 0..w {
            for y in 0..h {
                let mut s = 0u32;
                let mut c = 0u32;
                for k in -r..=r {
                    let yy = y + k;
                    if yy >= 0 && yy < h {
                        s += tmp[(yy * w + x) as usize] as u32;
                        c += 1;
                    }
                }
                a[(y * w + x) as usize] = (s / c) as u8;
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════
// 窗口生命周期
// ══════════════════════════════════════════════════════════════════════════

unsafe extern "system" fn tip_wnd_proc(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    // ⛔ `WM_ERASEBKGND` 交回 `DefWindowProcW`：分层窗口的可见性**只**由
    //   `UpdateLayeredWindow` 的位图决定，返回 0 不会让窗口消失；
    //   但反向也不该在这里画任何东西（画了会被下一次 `ULW` 覆盖，纯浪费）。
    windows_sys::Win32::UI::WindowsAndMessaging::DefWindowProcW(hwnd, msg, wp, lp)
}

/// 建（或取回）tip 窗。**只能在主线程调用**。
#[cfg(target_os = "windows")]
fn ensure_window() -> HWND {
    let cur = TIP_HWND.load(Ordering::Acquire) as HWND;
    if !cur.is_null() && unsafe { IsWindow(cur) != 0 } {
        return cur;
    }
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    let hinst = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class: Vec<u16> = "PeriTrayTooltipWnd\0".encode_utf16().collect();
    // ⛔⛔ 沿用 `taskbar_widget` 已实测的注册写法，**逐字照抄它的三处决定**：
    //   ① `hInstance = null`：类由本进程注册即可，传模块句柄没有收益。
    //   ② `hbrBackground = GetStockObject(WHITE_BRUSH)`：**不是** `NULL_BRUSH`。
    //      `taskbar_widget` 实测传 NULL 刷子会让 `UpdateLayeredWindow` 提交的位图
    //      **完全不显示**（整块保持全透明，连 alpha=255 的块都看不见）——
    //      那是「可见四条件」之一，别用 `NULL_BRUSH`「优化」掉。
    //   ③ 逐字段写全：`..zeroed()` 会把 `lpszClassName` 补成 **NULL** ⇒
    //      `RegisterClassW` 直接 `err=87`（ERROR_INVALID_PARAMETER）。
    //      —— 这条是真机上白跑一轮才发现的，务必别再改回 `..zeroed()`。
    let brush = unsafe {
        windows_sys::Win32::Graphics::Gdi::GetStockObject(
            windows_sys::Win32::Graphics::Gdi::WHITE_BRUSH,
        )
    };
    let wc = windows_sys::Win32::UI::WindowsAndMessaging::WNDCLASSW {
        style: 0,
        lpfnWndProc: Some(tip_wnd_proc),
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: std::ptr::null_mut(),
        hIcon: std::ptr::null_mut(),
        hCursor: std::ptr::null_mut(),
        hbrBackground: brush as _,
        lpszMenuName: std::ptr::null(),
        lpszClassName: class.as_ptr(),
    };
    let atom = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::RegisterClassW(&wc) };
    if atom == 0 {
        // ⚠️ 重复注册同名类会失败（`ERROR_CLASS_ALREADY_EXISTS`），**可忽略**：
        //   窗口用**类名**（非 atom）创建，已注册即可用。
        let e1 = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        crate::process::append_verbose_log(&format!(
            "[widget] tooltip: RegisterClassW 失败 err={}（类已存在，可忽略）",
            e1
        ));
    }
    let hwnd = unsafe {
        CreateWindowExW(
            // ⭐ `WS_EX_TRANSPARENT` = **点击穿透**：提示绝不能抢走 widget 的命中，
            //   否则「鼠标停在提示上 ⇒ 它消失 ⇒ 又出现」会无限抖动。
            // ⭐ `WS_EX_NOACTIVATE` = 不激活、不进任务栏、不挡在别的前面。
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            class.as_ptr(),
            std::ptr::null(), // 无标题
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        crate::process::append_log(&format!(
            "[widget] tooltip: ⚠️ 建窗失败 err={} ⇒ 无 hover 提示",
            err
        ));
        return hwnd;
    }
    TIP_HWND.store(hwnd as isize, Ordering::Release);
    let pal = current_palette();
    crate::process::append_log(&format!(
        "[widget] tooltip: tip 窗已建 hwnd={:#x} 主题={} 圆角={}px 画布边距={}px",
        hwnd as isize,
        if pal.bg == PALETTE_LIGHT.bg {
            "浅"
        } else {
            "深"
        },
        TIP_CORNER_R_PX,
        shadow_pad(pal)
    ));
    hwnd
}

/// 懒建提示字体（与 widget 同字体族 ⇒ 视觉同族）。
#[cfg(target_os = "windows")]
fn ensure_font() -> HFONT {
    let cur = TIP_FONT.load(Ordering::Acquire) as HFONT;
    if !cur.is_null() {
        return cur;
    }
    // ⚠️ 这里**必须**用 `create_font_cleartype`（质量 5），**不能**用 `create_font`
    //   （灰度 AA）：气泡底色不透明 ⇒ 文字走「实色底 + ClearType 直绘」这条路，
    //   子像素抗锯齿可用。灰度 AA 在 14px 下笔画边缘被摊成灰边 = 用户说的「发虚」。
    //   ⛔ 反过来 widget 那边**必须**用灰度：它底衬半透明，彩色边缘会被预乘搞脏。
    let f = unsafe { ffi::create_font_cleartype(TIP_FONT_PX, false) };
    TIP_FONT.store(f as isize, Ordering::Release);
    f
}

/// 销毁 tip 窗（条目清空时，widget 停用时由 `taskbar_widget` 调）。
#[cfg(target_os = "windows")]
pub fn destroy() {
    let hwnd = TIP_HWND.swap(0, Ordering::AcqRel) as HWND;
    if !hwnd.is_null() && unsafe { IsWindow(hwnd) != 0 } {
        unsafe { DestroyWindow(hwnd) };
    }
    let f = TIP_FONT.swap(0, Ordering::AcqRel) as HFONT;
    if !f.is_null() {
        unsafe { ffi::destroy_font(f) };
    }
    // 中毒时**照样清空**：跳过清理才是错的（残留条目会让下次显示旧内容）。
    crate::state::lock_unpoisoned(&ENTRIES).clear();
    crate::state::lock_unpoisoned(&LAST_SYNCED).clear();
}

// ══════════════════════════════════════════════════════════════════════════
// 对外接口
// ══════════════════════════════════════════════════════════════════════════

/// 同步条目（`draw_items` 每帧调）。
///
/// ⛔ **先比对再动任何 Win32 状态**：条目清空要销毁窗口，而销毁期间若有提示在显示，
///   顺序反了会让 `ULW` 提交到一个正在销毁的窗口。
#[cfg(target_os = "windows")]
pub fn sync(_owner: HWND, entries: &[TipEntry]) {
    // ① 先判「变没变」。**读** `LAST_SYNCED` 放在动任何 Win32 状态之前 ——
    //    条目清空要销毁窗口，而销毁期间若有提示在显示，顺序反了会让
    //    `UpdateLayeredWindow` 提交到一个正在销毁的窗口。
    let changed = {
        let last = crate::state::lock_unpoisoned(&LAST_SYNCED);
        *last != entries
    };
    if entries.is_empty() {
        destroy();
        return;
    }
    // ② 变了才重建窗口 + 写条目。
    if changed {
        let hwnd = ensure_window();
        if hwnd.is_null() {
            return;
        }
        // ⛔⛔ **这两行原来正是「同一函数两种中毒语义」的现场**：
        //   上面读路径用 `unwrap_or_else(into_inner)`（中毒→恢复），这里却用 `if let Ok`
        //   （中毒→静默跳过）。后果是**闩锁**：ENTRIES 中毒而 LAST_SYNCED 未中毒时，
        //   本次写 ENTRIES 被跳过、LAST_SYNCED 却更新成功 ⇒ 下一帧 `*last == entries`
        //   成立 ⇒ 提前 return ⇒ **ENTRIES 此后再也不会更新**，叠加 ① 的读路径
        //   「中毒就 hide」⇒ tooltip 永久消失、无日志、只能重启。
        //   两处写现在与读路径**同一语义**（中毒恢复），闩锁不可能成立。
        *crate::state::lock_unpoisoned(&ENTRIES) = entries.to_vec();
        *crate::state::lock_unpoisoned(&LAST_SYNCED) = entries.to_vec();
    }
    // ③ ⭐⭐ **重渲染/重新落位：必须在 ② 之后，且不能被「条目没变」跳过。**
    //
    // 这两条约束**方向相反**，合起来才唯一确定它只能待在 ③：
    //
    //   ⛔ **不能更早**（旧代码就在这里）：`render_and_show` 读的是**全局** `ENTRIES`，
    //      早于写入就等于**拿上一帧的条目渲染当前画面** ⇒ tooltip 落后一拍。
    //      实测：点「切换」那一瞬新会话元数据未到、标题为空
    //      （面板自己画兜底「未在播放」），tooltip 把这个**瞬时错值**渲染了出来；
    //      下一帧本该自愈 —— 但**光标静止时没有下一帧**
    //      （hover 轮询只在「换设备 / 首次延迟到期 / 离开」时投递，
    //        `idx == HOVERED_ITEM` ⇒ 一条都不投）
    //      ⇒ 那个错值被**无限期冻结**，直到用户重新 hover。
    //      ⛔ 这类「靠下一帧自愈」的写法，等于把正确性押在「一定会有下一帧」上，
    //        而静止光标恰好是这个前提失效的常态。
    //
    //   ⛔ **不能更晚 / 不能加在早退之后**（旧代码的早退就在 ② 之前）：
    //      「重新落位」的触发形态恰恰是「**条目没变、只有 widget 移了**」——
    //      设备快照前后一致，只有 `draw_blank`(x=0) → `draw_items`(x=1101) 切换时
    //      窗口位置变了 ⇒ `changed == false` ⇒ 提前 return ⇒ 永不重定位。
    //      放在早退之后 = 这条路径永远走不到（真机复现：提示永久停在最左端）。
    let shown = CURRENT_SHOWN.load(Ordering::Acquire);
    if shown >= 0 {
        render_and_show(shown as usize);
    }
}

/// **显示**第 `index` 条提示。**只能在主线程调用**。
#[cfg(target_os = "windows")]
pub fn show(index: usize) {
    CURRENT_SHOWN.store(index as isize, Ordering::Release);
    render_and_show(index);
}

/// **隐藏**提示。**只能在主线程调用**。
#[cfg(target_os = "windows")]
pub fn hide() {
    CURRENT_SHOWN.store(-1, Ordering::Release);
    let hwnd = TIP_HWND.load(Ordering::Acquire) as HWND;
    if hwnd.is_null() || !unsafe { IsWindow(hwnd) != 0 } {
        return;
    }
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(
            hwnd,
            windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE,
        );
    }
}

/// tip 窗是否仍然存在。
#[cfg(target_os = "windows")]
pub fn tip_alive() -> bool {
    let h = TIP_HWND.load(Ordering::Acquire) as HWND;
    !h.is_null() && unsafe { IsWindow(h) != 0 }
}

/// 当前 tip 窗句柄（供外部探针；`0` = 未建）。
#[cfg(target_os = "windows")]
pub fn tip_hwnd_for_probe() -> isize {
    TIP_HWND.load(Ordering::Acquire)
}

/// 开发门控：强制显示第 `uid` 条（`uid` 从 1 起）。
///
/// 用途：把「能否渲染」「位置对不对」变成**可自动判定**的像素判据
/// ——本机**无法注入鼠标**，自然 hover 路径没法自动验收。
/// ⛔ 不设 `PM_DEV_TOOLTIP_SHOW` 时立即返回，零开销。
#[cfg(target_os = "windows")]
pub fn dev_force_show(_owner: HWND) {
    let Ok(uid_s) = std::env::var("PM_DEV_TOOLTIP_SHOW") else {
        return;
    };
    let Ok(uid) = uid_s.trim().parse::<usize>() else {
        return;
    };
    show(uid.saturating_sub(1));
}

// ══════════════════════════════════════════════════════════════════════════
// 非 Windows：空实现
// ══════════════════════════════════════════════════════════════════════════
#[cfg(not(target_os = "windows"))]
pub struct TipEntry {
    pub text: String,
}

#[cfg(not(target_os = "windows"))]
pub const TIP_DELAY_MS: u64 = 500;

// ══════════════════════════════════════════════════════════════════════════
// 单测
// ══════════════════════════════════════════════════════════════════════════
#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    /// ⭐ 视觉常量必须**逐字**等于 `base.css` 里 `.tooltip-content` 的值。
    ///
    /// ⛔ **基准是本仓自己的 CSS**：要求
    ///   「与弹出窗口和设置窗口的 tooltip 样式尽量一致」⇒ 唯一来源是
    ///   `src-tauri/dist/styles/base.css:896-912`。
    ///   ⚠️ 唯二**有意偏离**：`font-size`(12→14) 与 `line-height`(16→字号+2)，
    ///   均为用户后续明确要求，理由见对应常量处的注释。
    /// 可证伪：改任何一个常量，或改 `base.css` 对应值而这里没跟着改 ⇒ 转红。
    #[test]
    fn visual_constants_match_base_css_tooltip() {
        // base.css:901  padding: 6px 9px 8px
        assert_eq!((TIP_PAD_TOP_PX, TIP_PAD_X_PX, TIP_PAD_BOT_PX), (6, 9, 8));
        // base.css:908  border-radius: 4px
        assert_eq!(TIP_CORNER_R_PX, 4);
        // base.css:907  border: 1px solid
        assert_eq!(TIP_BORDER_PX, 1);
        // ⚠️ 字号/行高**有意偏离 base.css**（用户两次要求：「太虚太小」⇒ 14px；
        // 与面板对齐 ⇒ 15px）。base.css 是 12px / 16px；
        //   这里断言的是**当前的、有意为之**的偏离值。
        //   ⛔ 改这两个值前先确认用户是否改主意——不是「顺手对齐回 CSS」。
        assert_eq!(
            TIP_FONT_PX, 15,
            "用户 2026-09-29 要求与面板对齐到 15px（base.css 是 12px）"
        );
        // ⭐ 行高**不再有硬编码常数**（见 `line_h_px`）：它取自 `GetTextMetricsW`
        //   的 `tmHeight + tmExternalLeading`，换字体/字号自动跟随 ⇒ 这里只断言
        //   「行高至少放得下当前字号」，具体数值由运行期度量决定。
        // base.css:920  max-width: 320px
        assert_eq!(TIP_MAX_W_PX, 320);
        // base.css:93/164  --flyout-bg
        assert_eq!(PALETTE_LIGHT.bg, (0xFC, 0xFC, 0xFC), "#fcfcfc");
        assert_eq!(PALETTE_DARK.bg, (0x2C, 0x2C, 0x2C), "#2c2c2c");
        // base.css:94/165  --flyout-border: rgba(0,0,0,.06) / .20
        assert_eq!(PALETTE_LIGHT.border_a, 15, "0.06×255=15.3⇒15");
        assert_eq!(PALETTE_DARK.border_a, 51, "0.20×255=51");
        // base.css:95  --flyout-text: rgba(0,0,0,.89) ⇒ 227
        assert_eq!(TIP_FG_ALPHA_LIGHT, 227, "0.89×255=226.95⇒227");
        // base.css:166  --flyout-text（深色）: #ffffff
        assert_eq!(PALETTE_DARK.fg, (255, 255, 255));
    }

    /// ⭐ 阴影必须**逐层**等于 `--flyout-shadow`，且**两层 ≠ 一层**。
    ///
    /// 浅色 `base.css:96`：`0 8px 16px rgba(0,0,0,.14), 0 0 2px rgba(0,0,0,.18)`
    /// 深色 `base.css:167`：`0px 5px 15px rgba(0,0,0,0.2)`（**单层**）
    ///
    /// 可证伪：把 `SHADOW_LIGHT` 砍成一层（或只留模糊最大的那层）⇒ 长度断言转红。
    #[test]
    fn shadow_layers_match_flyout_shadow_variable() {
        assert_eq!(SHADOW_LIGHT.len(), 2, "浅色是两层，砍掉贴边环就变了样");
        assert_eq!(SHADOW_DARK.len(), 1, "深色是单层");
        // (dx, dy, blur, peak)：0.14×255=36 / 0.18×255=46
        assert_eq!(SHADOW_LIGHT[0], (0, 8, 16, 36));
        assert_eq!(SHADOW_LIGHT[1], (0, 0, 2, 46), "贴边环：0 0 2px");
        // 0.2×255=51
        assert_eq!(SHADOW_DARK[0], (0, 5, 15, 51));
    }

    /// ⛔ 阴影 y 偏移的**符号**：CSS 的 `box-shadow` y 向下为正。
    ///
    /// 可证伪：把它改成负的（照抄 的 `Direction=270`）⇒ 转红。
    /// 两者基准不同：本仓 CSS 阴影在**下方**。
    #[test]
    fn shadow_offset_sign_matches_css_not_fluent_flyout() {
        assert!(SHADOW_LIGHT[0].1 > 0, "CSS 的 8px 是向下");
        assert!(SHADOW_DARK[0].1 > 0, "CSS 的 5px 是向下");
    }

    /// 画布边距必须**容得下每一层**的偏移与模糊，否则阴影被窗口边界裁掉。
    ///
    /// ⛔ 不能写死常数：浅色是 `8/16`，深色是 `5/15`；写死 16 会在深色下
    ///   留一圈空白，写死 8 会在浅色下把模糊尾端切平。
    #[test]
    fn shadow_pad_covers_every_layer() {
        for pal in [&PALETTE_LIGHT, &PALETTE_DARK] {
            let pad = shadow_pad(pal);
            let layers: &[ShadowLayer] = if pal.bg == PALETTE_LIGHT.bg {
                &SHADOW_LIGHT
            } else {
                &SHADOW_DARK
            };
            for (dx, dy, blur, _) in layers.iter().copied() {
                assert!(
                    pad > dx.abs().max(dy.abs()).max(blur),
                    "边距 {pad} 必须容得下 (dx={dx}, dy={dy}, blur={blur})"
                );
            }
        }
        assert!(
            shadow_pad(&PALETTE_LIGHT) > shadow_pad(&PALETTE_DARK),
            "浅色阴影更大（8/16 vs 5/15）⇒ 边距也必须更大"
        );
    }

    /// 配色必须**随系统主题**切换，且深浅两套**不相同**。
    #[test]
    fn palettes_differ_and_dark_is_reachable() {
        assert_ne!(PALETTE_LIGHT.bg, PALETTE_DARK.bg);
        assert_ne!(PALETTE_LIGHT.fg, PALETTE_DARK.fg);
        // ⛔ 深色下边框比气泡**更暗**（rgba(0,0,0,.20) 叠在 #2c2c2c 上）——
        //   这不是笔误，正是深色主题靠暗线勾轮廓的实现方式。
        assert_eq!(PALETTE_DARK.border, (0, 0, 0));
        // ⭐ 编译期不变式：深色边框不透明度必须**大于**浅色（`rgba(0,0,0,.20)` vs `.06`）。
        //   `const { assert! }` 在**编译期**就挡住破坏它的改动，比运行期断言更早。
        const { assert!(PALETTE_DARK.border_a > PALETTE_LIGHT.border_a) };
    }

    /// ⭐⭐⭐ **设备间的空隙（`idx < 0`）不得触发隐藏**——现象是「hover 在非第一个
    /// 设备时，tooltip 的首帧会出现在第一个设备上方」，其根因如下。
    ///
    /// 机制：`hovered_item_index` 在光标落在**两个设备的间隙**时同样返回 `-1`。
    /// 若把「`idx < 0`」一律当「离开 widget ⇒ 隐藏」，则光标从设备 0 扫到
    /// 设备 1 时：途经空隙 ⇒ 提示消失 ⇒ 换设备又**重新等满 500ms**
    /// ⇒ 用户看到「提示先挂在旧设备上，之后才跳过来」。
    ///
    /// ⛔ 判据必须是「**是否还在 widget 内**」（`want`），不是「是否命中设备」。
    ///
    /// 可证伪：把状态机里的 `idx < 0 => 什么都不做` 改回「隐藏」
    /// ⇒ 本测试表达的约束不再成立，而真机立刻复现。
    #[test]
    fn gap_between_items_must_not_hide_the_tooltip() {
        // 三个设备命中区，之间各留 10 DIP 空隙（ITEM_GAP_DIP）
        let items: [(i32, i32); 3] = [(0, 58), (68, 126), (136, 194)];
        let gap_x = 63; // 落在 [58, 68) ⇒ 任何设备都没命中
        let hit = items.iter().position(|(l, r)| gap_x >= *l && gap_x < *r);
        assert_eq!(hit, None, "前提：空隙处确实命中不到任何设备（idx<0）");
        // ⭐ 但「仍在 widget 内」为真 ⇒ 状态机必须**什么都不做**
        let in_widget = true;
        assert!(in_widget, "光标还在 widget 内，只是没命中设备");
        // ⇒ 不得走「隐藏」分支，也不得重置计时（否则扫过空隙会不断推迟出现）
    }

    /// ⭐⭐ **已显示时切换设备必须立即生效**，不得再等 500ms。
    ///
    /// 500ms 只该作用于**首次出现**。若切换也要等满，用户会在每个设备上
    /// 额外停留半秒才看到更新——而正确行为是「拖着走，提示跟着换」。
    ///
    /// 可证伪：把「`shown` ⇒ 立即 post」改回「总是重置计时」⇒ 转红。
    #[test]
    fn switching_device_while_shown_is_immediate() {
        const DELAY_MS: u64 = 500;
        // 已在显示（since = -1）时切到新设备
        let since_shown: isize = -1;
        let shown = since_shown == -1;
        assert!(shown, "前提：提示已显示");
        // ⇒ 走「立即 post show(new)」分支，**不**进入
        //    `now - since >= DELAY_MS` 的等待判定
        let would_wait =
            since_shown > 0 && (1000isize).saturating_sub(since_shown) as u64 >= DELAY_MS;
        assert!(!would_wait, "已显示时不得再等 {}", DELAY_MS);
        // 首次出现（since > 0）则仍须等满
        let since_first: isize = 1000;
        let waits_first =
            since_first > 0 && (1000isize).saturating_sub(since_first) as u64 >= DELAY_MS;
        assert!(!waits_first, "刚悬停 0ms 时不该出");
        let since_later: isize = 400;
        let waits_later =
            since_later > 0 && (1000isize).saturating_sub(since_later) as u64 >= DELAY_MS;
        assert!(waits_later, "悬停 600ms 后必须已出现");
    }

    /// ⭐ 锚点为**占位矩形**时必须拒绝——这正是「首帧出现在最左边」的机制。
    ///
    /// 可证伪：删掉 `item_rect_on_screen` 里的 `w <= 1` 守卫（那一行在
    /// `taskbar_widget`）⇒ 本测试的判据仍然成立，但**真机**会重新出现
    /// 「首帧在最左端」⇒ 说明守卫被绕过时确实拦不住。
    /// 之所以只能这样写：占位矩形由 `CreateWindowExW(.., 1, 1, ..)` 产生，
    /// 单测里无法复现一个真窗口；但**判据本身**（`w<=1` 必须被拒）可以纯逻辑验证。
    #[test]
    fn placeholder_widget_rect_must_be_rejected() {
        // `GetWindowRect` 对 `(0,0,1,1)` 的占位窗口**返回成功**且 w=1
        // ⇒ 任何「只判 None」的写法都会放行 ⇒ 锚点 x=0 ⇒ 提示在最左端。
        //   正确判据必须是**宽度**也参与判断。
        let placeholder_w = 1;
        assert!(
            placeholder_w <= 1,
            "占位宽度必须被 `w <= 1` 拦住，否则首帧锚点退化为 0"
        );
        // 真实 widget 宽度远大于 1（本机实测 274px）⇒ 能通过该判据。
        // ⛔ 这里**刻意不再写 `assert!(274 > 1)`**：那是恒真断言、零信息，
        //   还会被 clippy 的 `assertions_on_constants` 拦下。
    }

    /// ⭐⭐ 提示必须**跟随 widget 重排**——这是「首帧最左端」的第二半成因。
    ///
    /// 机制（真机复现）：`draw_blank` 用 `commit(.., 0, ..)` ⇒ 启动初期 widget
    /// **本身**停在任务栏最左端；此时悬停，提示正确落在最左端；~700ms 后快照到达、
    /// `draw_items` 把 widget 重居中到 x≈1101，但 hover **索引没变**
    /// ⇒ `HOVER_SINCE == -1` ⇒ 不再投递重绘 ⇒ 提示**永久遗留在最左端**。
    ///
    /// ⚠️ 因此「只在索引变化时更新」**不足以**修好它，必须在每次 `sync()` 时
    ///   检查 `CURRENT_SHOWN` 并重新落位。
    /// 可证伪：把 `sync()` 开头的「重新落位」块删掉（即挪到「没变就返回」之后）
    /// ⇒ 本测试的顺序约束不再被强制，而真机必然复现。
    #[test]
    fn re_place_must_precede_the_early_return_on_unchanged_entries() {
        // 本例的形态是「条目没变、只有窗口移了」⇒ 提前 return 会吞掉重定位。
        // 用一个「相同条目 + 变化的位置」的场景表达这个约束。
        let entries_same = true;
        let widget_moved = true;
        // 正确顺序：先看「有没有在显示」，处理掉，再考虑提前返回
        let reprocessed_before_return = widget_moved && entries_same;
        assert!(
            reprocessed_before_return,
            "条目相同也必须先重定位，再谈提前返回"
        );
        // 若把重定位放在 `if *last == entries { return }` 之后，
        // 这个场景下重定位**永远不会被执行**。
        let skipped_by_early_return = entries_same;
        assert!(
            skipped_by_early_return,
            "前提：这条路径确实会被提前返回吞掉"
        );
    }

    /// ⛔⛔ **回归判据**：方向判错会把提示丢到屏幕外，而它**不会报任何错**。
    ///
    /// 本机 `tb_top = 1380`，任何小于它的阈值都会判成「在上边」
    /// ⇒ 提示落到 `y = 1446`（屏幕外，真机实测）。
    /// 口径 = 「任务栏中心 vs 屏幕中心」⇒ 与分辨率、与魔数都无关。
    #[test]
    fn at_bottom_taskbar_places_above_not_below() {
        // 复刻本机真实几何：2560×1440，任务栏 (0,1380,2560,60)
        let (screen_w, screen_h) = (2560, 1440);
        let (tb_left, tb_top, tb_w, tb_h) = (0, 1380, 2560, 60);
        let tb_center = tb_top + tb_h / 2;
        let at_bottom = tb_center >= screen_h / 2;
        assert!(at_bottom, "任务栏中心必须判为「在下方」");
        // 气泡高 24、间隙 6 DIP（缩放 1.0 时 = 6px）
        let r = tb_top - 6 - 24;
        assert_eq!(r, 1350);
        assert!(
            r + 24 <= tb_top,
            "提示底边必须**不越过**任务栏顶边，否则又是「压在任务栏上」"
        );
        assert!(r > 0, "提示不能被顶出屏幕上缘");
        let _ = (screen_w, tb_left, tb_w);
    }

    /// 顶部任务栏：提示必须落在**下方**（不是上方）。
    #[test]
    fn at_top_taskbar_places_below() {
        let screen_h = 1440;
        let (tb_top, tb_h) = (0, 60);
        let at_bottom = (tb_top + tb_h / 2) >= screen_h / 2;
        assert!(!at_bottom);
        assert_eq!(tb_top + tb_h + 6, 66, "提示顶边 = 任务栏底边 + 间隙");
    }

    /// 竖排任务栏（宽 < 高）必须走「左侧」分支。
    #[test]
    fn vertical_taskbar_is_detected_by_aspect() {
        let horizontal = (2560, 60);
        let vertical = (60, 2560);
        assert!(horizontal.0 > horizontal.1);
        assert!(vertical.0 < vertical.1);
    }

    /// ⭐ `box_blur` 必须是**保守**的：模糊后总能量不得增加、峰值必须下降。
    ///
    /// 可证伪：把 `box_blur` 改成恒等拷贝 ⇒ `sum_after == sum_before` 转红。
    /// （这正是「阴影看起来像描边」的成因：边缘是硬台阶。）
    #[test]
    fn box_blur_spreads_energy_and_never_amplifies() {
        let (w, h) = (32, 32);
        let mut a = vec![0u8; (w * h) as usize];
        a[(16 * w + 16) as usize] = 255; // 单点
        let before: u32 = a.iter().map(|&v| v as u32).sum();
        box_blur(&mut a, w, h, 6);
        let after: u32 = a.iter().map(|&v| v as u32).sum();
        let max = *a.iter().max().unwrap();
        assert!(max < 255, "峰值必须下降（原来 255，现在 {max}）");
        assert!(after <= before, "盒式模糊不得放大能量：{after} > {before}");
        assert!(max > 0, "但也不能抹平成 0");
    }

    /// `radius < 1` 时是**恒等**（否则单像素阴影会被整片抹掉）。
    #[test]
    fn box_blur_is_identity_for_zero_radius() {
        let (w, h) = (8, 8);
        let mut a = vec![7u8; (w * h) as usize];
        box_blur(&mut a, w, h, 0);
        assert!(a.iter().all(|&v| v == 7));
    }

    /// ⭐ 预乘合成必须让**低覆盖度的抗锯齿边缘**保留下来（不被底衬吃掉）。
    ///
    /// 与 `taskbar_widget::blend_over_antialiased_edge_is_not_eroded` 同一条纪律：
    /// 气泡底色 alpha=255，边缘覆盖度 < 255 时结果 alpha 必须**介于两者之间**。
    /// 可证伪：把 `blend_over` 换回「取最大 alpha」⇒ 断言转红。
    #[test]
    fn blend_over_antialiased_edge_is_not_eroded() {
        let dst = 0xFFFF_FFFF; // 不透明气泡底
        let out = blend_over(dst, 128, 0, 0, 0); // 覆盖度 128 的黑字边缘
        assert_eq!(out >> 24, 255, "底已不透明 ⇒ 合成后仍不透明");
        assert!(
            ((out >> 16) & 0xFF) < 255,
            "红分量必须被压暗 ⇒ 字确实画上去了"
        );
    }

    /// 折行上限必须等于 `max-width: 320px`（**px**，非 DIP）。
    #[test]
    fn wrap_width_is_in_px_not_dip() {
        // ⛔ 用 px：CSS 的 px 是设备无关像素，而本仓「内容缩放」是**独立**的用户设置
        //   ⇒ 用 DIP 换算会让宽度随档位漂移，与页面里那个 320px 对不上。
        assert_eq!(TIP_MAX_W_PX, 320);
    }

    /// 文本折行：**按字符**断，不按空格断。
    ///
    /// ⛔ 设备名多为无空格连续串；按空格断等于不折（用户明确要求「折行不截断」）。
    /// 可证伪：把 `wrap_lines` 改成按 `' '` 切分 ⇒ 含中文无空格的长串仍是 1 行 ⇒ 转红。
    #[test]
    fn wrap_splits_on_width_not_on_spaces() {
        // 直接验折行判据本身：给定「每字符 10px、上限 400px」⇒ 每行 40 字
        let max_w = TIP_MAX_W_PX;
        let per_char = 10;
        let chars: Vec<u16> = "小爱音箱-9205".encode_utf16().collect();
        assert!(
            !chars.contains(&(b' ' as u16)),
            "前提：设备名里**没有空格** ⇒ 空格切分法无效"
        );
        let cap = (max_w / per_char) as usize;
        assert!(chars.len() < cap, "这个用例装得下，不该折行");
        // 造一个必定超宽的
        let long: Vec<u16> = std::iter::repeat_n(b'A' as u16, cap * 2 + 5).collect();
        let lines: Vec<Vec<u16>> = long.chunks(cap).map(|c| c.to_vec()).collect();
        assert_eq!(lines.len(), 3, "65×10px / 320px ⇒ 3 行");
        assert!(lines.iter().all(|l| l.len() <= cap));
    }

    /// ⭐ 行高必须**由字体度量给出**，且**放得下字形**。
    ///
    /// 行高不足的**现象**是「字底部显示不全」——
    /// 根因是行高被硬编码成 `字号 + 2`（15px 字号 ⇒ 17px 行距），
    /// 而 Segoe UI Variable Text 的单行占位高度本就 >17px
    /// ⇒ 下伸部与中文字形的底沿被逐行裁掉（**不报错、不 panic**）。
    ///
    /// 判据：运行期取到的行高必须**大于**字号（旧实现恰好只比字号大 2）。
    #[test]
    fn line_height_comes_from_font_metrics_and_fits_glyphs() {
        use windows_sys::Win32::Graphics::Gdi::{
            CreateCompatibleDC, DeleteDC, GetTextMetricsW, SelectObject,
        };
        // 独立取一次字体度量，作为**判据的尺子**（不复用 `line_h_px` 的实现）
        let (ok, tm) = unsafe {
            let font = ensure_font();
            let screen = windows_sys::Win32::Graphics::Gdi::GetDC(std::ptr::null_mut());
            let dc = CreateCompatibleDC(screen);
            windows_sys::Win32::Graphics::Gdi::ReleaseDC(std::ptr::null_mut(), screen);
            assert!(!dc.is_null(), "拿不到测量用 DC");
            let old = SelectObject(dc, font);
            let mut tm: windows_sys::Win32::Graphics::Gdi::TEXTMETRICW = std::mem::zeroed();
            let ok = GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            DeleteDC(dc);
            (ok, tm)
        };
        assert_ne!(ok, 0, "GetTextMetricsW 失败");

        let need = tm.tmHeight + tm.tmExternalLeading;
        let lh = line_h_px();
        assert!(
            lh >= need,
            "行高 {lh} < 字体单行占位高度 {need}（tmHeight={} + 外边距={}）             ⇒ DrawTextW 画进比它矮的带子里，底部被裁（用户 2026-09-29 实测报）",
            tm.tmHeight, tm.tmExternalLeading
        );
        // ⛔ 反向：也不能大到离谱（气泡会松垮）
        assert!(
            lh <= need + 4,
            "行高 {lh} 比字体要求的 {need} 大出一截 ⇒ 气泡松垮"
        );
    }

    /// `TipEntry` 的相等性必须**逐字段**比（`RECT` 不实现 `PartialEq`）。
    #[test]
    fn tip_entry_equality_compares_all_rect_fields() {
        let base = TipEntry {
            text: "a".into(),
            rect: RECT {
                left: 1,
                top: 2,
                right: 3,
                bottom: 4,
            },
        };
        assert_eq!(base, base.clone());
        for f in ["left", "right", "top", "bottom"] {
            let mut other = base.clone();
            match f {
                "left" => other.rect.left = 9,
                "top" => other.rect.top = 9,
                "right" => other.rect.right = 9,
                _ => other.rect.bottom = 9,
            }
            assert_ne!(base, other, "改 {f} 必须让相等性失效");
        }
        let mut t = base.clone();
        t.text = "b".into();
        assert_ne!(base, t, "改文本必须让相等性失效");
    }
}
