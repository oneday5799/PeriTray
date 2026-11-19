/* common.js — 共享基础层（popup/settings 两页最先加载）：Tauri invoke/主题与材质应用/
 *            右键菜单族（默认菜单屏蔽/注册/钳位/关闭/子菜单外壳 createSubmenuShell/勾选图标 createCheckIcon）/
 *            设备显示名解析（simplifyDeviceName/lookupDeviceAlias/formatDeviceName/getDisplayName）/
 *            对话框与 toast/快捷键录制器（码表为本文件内部实现细节，不对外）/
 *            共享数据（CATEGORIES 设备分类/A2DP_ACTIVE_STATES Sink 占用态）
 * 加载序 1/N · 提供：window 全局 API —— CATEGORIES/A2DP_ACTIVE_STATES/initTheme/applyThemeMode/applyMaterialMode/
 *             getInvoke/getDisplayName/simplifyDeviceName/lookupDeviceAlias/formatDeviceName/
 *             attachTooltip/
 *             attachSessionTooltip/showSessionTip/hideSessionTip/reconcileCards/registerContextMenu/
 *             clampMenuPosition/hideAllContextMenus/createSubmenuShell/createCheckIcon/
 *             showDeviceInfoDialog/createComboBox/bindComboBox/
 *             createDialog/closeDialog/showToast/describeShortcutError/
 *             bindShortcutRecorder
 * 依赖：window.__TAURI__（由 Tauri 运行时注入）；被两页全部脚本依赖
 *       ⚠️ 本文件顶层的 `const invoke` 同时是 **settings 页全部脚本的 invoke 来源**
 *       （它们以裸 `invoke(...)` 跨脚本词法绑定使用，见下方注释） */
//
// Tauri API 一律「惰性获取 + 防御式降级」（P2-3）。
// 顶层直接解构 `window.__TAURI__.core` 会在运行时未注入（或注入晚于本文件执行）时
// 抛 TypeError ⇒ **本文件后续所有 `window.*` API 都不再定义**，两页功能整体失效
// ——这不是降级，是全崩。故：
//   · `invoke` 包装为「每次调用重新解析」，未就绪时返回 rejected Promise，
//     交给调用方既有的 `.catch()` / `try-catch` 走降级路径；
//   · 顶层事件监听一律经 `onTauriEvent()`，未就绪时静默跳过并返回 false。
//   · ⚠️ 下面这个 `invoke` 包装**同时是 settings 页的 invoke 来源**：settings*.js
//     以裸 `invoke(...)` 跨脚本词法绑定使用它（顶层 `const` 不是 `window` 属性，
//     但同页后续脚本可见）⇒ **不要搬出本文件、不要改成 `window.invoke`、不要让
//     settings.html 把它排到 common.js 之前**。实测把本文件排到最后：`check.mjs`
//     仍报通过（声明池按页汇总且无序，看不见顺序），而运行时报
//     `ReferenceError: registerContextMenu is not defined`（顶层调用的 `window.*`）、
//     `ReferenceError: invoke is not defined`（词法 `const`——定义脚本执行前该全局
//     绑定根本不存在，是 not defined 而非 TDZ）。两页取用形式的差异见 `AGENTS.md`
//     「前端架构备忘」，**勿统一**。
const invoke = (...args) => {
  const core = window.__TAURI__ && window.__TAURI__.core;
  if (!core || typeof core.invoke !== "function") {
    return Promise.reject(new Error("Tauri API 未就绪"));
  }
  return core.invoke(...args);
};

// 顶层注册 Tauri 事件监听（P2-3）。用 `function` 声明以便被提升，文件内任意位置可用。
function onTauriEvent(name, handler) {
  const ev = window.__TAURI__ && window.__TAURI__.event;
  if (!ev || typeof ev.listen !== "function") {
    return false;
  }
  ev.listen(name, handler);
  return true;
}

window.CATEGORIES = [
  { key: "Audio", label: "音频设备", subtitle: "扬声器、耳机等音频设备", icon: "🔊" },
  { key: "Usb", label: "输入设备", subtitle: "键盘、鼠标等USB设备", icon: "⌨️" },
  { key: "Battery", label: "电池", subtitle: "电池设备", icon: "🔋" },
  { key: "Monitor", label: "显示器", subtitle: "显示器设备", icon: "🖥️" },
  { key: "Other", label: "其他设备", subtitle: "未归类的设备", icon: "📦" },
];

// A2DP Sink「占用中」状态集合：popup 设备卡（标签/按钮）与设置页设备行共用
// 同一判据。⛔ 物理只留这一份——两页各写一份必然漂移（审查实测已分叉）；
// 新增状态时两页行为同步变化，属预期。
window.A2DP_ACTIVE_STATES = ["connecting", "backoff", "armed", "link_ready"];

// 给元素挂载与「设备快捷键共享切换」一致的样式 tooltip（替代原生 title 提示）
window.attachTooltip = function (el, text, position) {
  if (!el || !text || el.dataset.tooltipSetup) return;
  el.dataset.tooltipSetup = "1";
  el.classList.add("tooltip-host");
  const tip = document.createElement("span");
  tip.className = "tooltip-content";
  if (position === "below") tip.classList.add("tooltip-content--below");
  else if (position === "end") tip.classList.add("tooltip-content--end");
  else if (position === "start") tip.classList.add("tooltip-content--start");
  tip.textContent = text;
  el.appendChild(tip);
};

window.getInvoke = function () {
  return window.__TAURI__ && window.__TAURI__.core
    ? window.__TAURI__.core.invoke
    : null;
};

// ── 主题（共享：设置页 + 主窗口） ─────────────────────
let themeMode = "follow_system";

window.applyThemeMode = function (mode) {
  themeMode = mode || "follow_system";
  const html = document.documentElement;
  const isDark = themeMode === "dark" ||
    (themeMode === "follow_system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  html.setAttribute("data-theme", isDark ? "dark" : "light");

  const invoke = getInvoke();
  if (invoke) {
    const theme = themeMode === "follow_system" ? "system" : isDark ? "dark" : "light";
    invoke("set_window_theme", { theme }).catch(() => {});
  }
};

window.initTheme = async function () {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    const config = await invoke("get_config");
    applyThemeMode(config.theme_mode || "follow_system");
  } catch (e) {
    console.error("Failed to init theme:", e);
  }
};

// 跟随系统时实时响应系统主题切换
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  if (themeMode === "follow_system") applyThemeMode("follow_system");
});

// config-changed: 设置页切主题时，主窗口/设置页实时同步
//
// ⭐ **顺带同步「显示设备信息组件」开关**：
//   弹出窗口是**隐藏而非销毁**，而开关值原先**只在加载时取一次**
//   ⇒ 用户在设置页开关之后，popup 里那份**一直是陈旧的**，
//   于是「第一次右键」仍按旧值显示「钉到任务栏」（下一次数据刷新才自愈）。
//   ⇒ 该事件在**每个窗口**都会触发，且 payload 就是**完整 config 快照**
//   ⇒ 在这个**共享** handler 里同步，是唯一需要改的地方（两页都覆盖，且
//   **不新增监听器、不新增取数**）。
//
// ⚠️ `taskbarWidgetEnabled` 用 `let` 声明在下方（约 320 行），而这个监听器
//   在上方注册 —— **函数声明会提升、`let` 不会**，但事件是异步的，
//   派发时整个脚本早已执行完毕 ⇒ 不存在 TDZ。仅此一处依赖该事实，已注记。
onTauriEvent("config-changed", (event) => {
  initTheme();
  const cfg = event && event.payload;
  // ⭐ 开关走 payload **同步**改（没有 await ⇒ 零陈旧窗口）
  if (cfg && typeof cfg.taskbar_widget_enabled === "boolean") {
    window.applyTaskbarWidgetEnabled(cfg);
  }
  // ⭐ 已钉名单：必须另发一次 invoke —— payload 里的 `pinned_taskbar_devices`
  //   只有**身份键**（`DeviceKey`），**没有解析后的显示名**，构不出「钉到/移出」的文案。
  //
  //   触发形态：在**设置页**移除已钉设备 ⇒ popup 的名单
  //   陈旧 ⇒ 右键文案与真实状态相反；而 popup **重新打开并不会刷新**
  //   （只有刷新按钮 / 设备事件才重跑）⇒ 错文案会留到下一次设备事件。
  //
  // ⚠️⚠️ 上面那条同步的与这一条**不是冗余，别「优化」掉其中任何一条**：
  //   · 开关走 payload ⇒ **同步**生效 ⇒ 切换后**第一次**右键就对
  //   · 名单只能走 invoke ⇒ **最终一致**（且 payload 根本构不出它）
  //   而 `refreshTaskbarPinnedNames()` 内部那次 `get_config` 也会顺手同步开关，
  //   但它是 **await 回来的、晚一拍** ⇒ **替代不了**上面那条同步的。
  //
  // ⚠️ 代价：每次 `config-changed` 多一次 `get_pinned_taskbar_list`。
  //   它是**内存读 + 小列表序列化**，且只在用户操作时触发（非紧循环）——
  //   与本仓反复较真的 WMI 数百毫秒不在一个量级，**不值得为省它再加一层判据**。
  window.refreshTaskbarPinnedNames();
});

// ── 窗口材质（共享：设置页 + 主窗口） ─────────────────
window.applyMaterialMode = function (material) {
  const html = document.documentElement;
  if (material && material !== "default") html.setAttribute("data-material", material);
  else html.removeAttribute("data-material");
};

(async () => {
  try {
    const cfg = await invoke("get_config");
    applyMaterialMode(cfg.window_material);
  } catch (e) {}
})();

// 材质变更时由 Rust 发出 material-changed；设置页切换过程中跳过（防闪烁时序由 settings.js 控制）
onTauriEvent("material-changed", (e) => {
  if (!window.__materialChangeInProgress) applyMaterialMode(e.payload);
});

// 设备别名查找：**短名键 → 精确键**两级（与后端 `config::resolve_device_name` 同序同义）。
//
// ⭐ `coreName` 由**调用方从后端数据里取**（`AudioDevice.core_name`）。
//   缺省时才退回 `simplifyDeviceName`（只取最外层括号、不剥蓝牙后缀）——
//   那条退路在 `扬声器 (小爱音箱-9205 Stereo)` 这类名字上与后端 `core_name`
//   不一致，曾导致「设备页改的别名、音量页查不到」。有后端值就别自己推。
//
// ⛔⛔ 为什么必须两级（这是「两页重命名不同步」的根因）：同一台设备在两页的 `name`
//   **不是同一个字符串** —— 音量页是**音频端点名**（「耳机 (小爱音箱-9205)」），
//   设备信息页是**物理设备名**（「小爱音箱-9205」）。后端写入侧
//   `apply_device_rename` 会把**两种形态归并**（长形态键 + 短名键都写），
//   可**从设备页发起时入口键本身就是短名**，长形态键未必已存在 ⇒ 只写短名键。
//   若读取侧只查精确键，音量页就永远查不到 ⇒「设备页改了名、音量页不变」。
//   后端 `resolve_device_name` 本来就是两级查找，前端此前却是单键 ⇒ 两侧口径不一致。
// ⚠️ 边界：此处「短名」= `simplifyDeviceName`（取最外层括号内内容），而后端 `core_name`
//   还会剥蓝牙协议后缀（` Stereo` / ` LE` …）。「类型 (设备名)」这种常见形态两者一致；
//   带后缀的长形态回退会落空（此时退回显示括号内容，与关闭简化时的观感相同）。
window.lookupDeviceAlias = function (customNames, name, coreName) {
  if (!customNames || !name) return undefined;
  // ⚠️ 空白值一律当「无别名」：后端 `resolve_device_name_in` 同样跳过，
  //    两侧必须一致（否则任务栏显示空白、页面显示原名）。
  const usable = (v) => v !== undefined && v !== null && String(v).trim() !== "";
  const short = coreName || window.simplifyDeviceName(name);
  if (short !== name && usable(customNames[short])) return customNames[short];
  if (usable(customNames[name])) return customNames[name];
  // ⭐ 第 3 级：**任一形态**的同源键（与后端第 3 级一一对应）。
  //    少了它，「短名键空白 + 长形态键有效」时设备页回落原名、音量页显示别名
  //    ⇒ 同一设备两套名字。按键名排序取首个，保证同一个配置每次都一样。
  const keys = Object.keys(customNames).sort();
  for (const k of keys) {
    if (!usable(customNames[k])) continue;
    if (window.simplifyDeviceName(k) === short) return customNames[k];
  }
  return undefined;
};

window.getDisplayName = function (dev, deviceNames) {
  // `dev.core_name` 目前只有 `AudioDevice` 带；设备信息页的 `Device` 没有 ⇒ 走退路。
  return window.lookupDeviceAlias(deviceNames, dev.name, dev.core_name) || dev.name;
};

// 简化设备名称：仅保留括号内的内容，如 "耳机 (小爱音箱-9205)" -> "小爱音箱-9205"
window.simplifyDeviceName = function (name) {
  if (!name) return name;
  const open = name.indexOf("(");
  const close = name.lastIndexOf(")");
  if (open >= 0 && close > open) {
    const inner = name.slice(open + 1, close).trim();
    if (inner) return inner;
  }
  return name;
};
// 设备显示名统一解析：有重命名用重命名 -> 可选简化括号名 -> 原名。
// customNames/simplify 由调用方注入数据源（popup 与 settings 的存储位置不同）。
// 注意：getDisplayName 为无简化步骤的另一语义（仅重命名回退原名），勿混淆。
window.formatDeviceName = function (name, customNames, simplify, coreName) {
  const custom = window.lookupDeviceAlias(customNames, name, coreName);
  if (custom) return custom;
  if (simplify) return window.simplifyDeviceName(name);
  return name;
};

// ── 对账式渲染骨架（popup 设备/会话列表共用） ────────────
// 按 id 差集移除失效卡，已有卡走 update，新增项建卡后追加
function reconcileCards(list, cardSelector, idProp, items, createCard, updateCard) {
  const existingCards = new Map();
  list.querySelectorAll(cardSelector).forEach(card => {
    existingCards.set(card.dataset[idProp], card);
  });

  const newIds = new Set(items.map(item => item.id));

  existingCards.forEach((card, id) => {
    if (!newIds.has(id)) {
      card.remove();
    }
  });

  for (const item of items) {
    let card = existingCards.get(item.id);

    if (card) {
      updateCard(card, item);
    } else {
      list.appendChild(createCard(item));
    }
  }
}

// ── 应用名 tooltip（页面内，超出边界自动避让） ────────────
let sessionTipTimer = null;
let sessionTip = null;

function getSessionTip() {
  if (!sessionTip) {
    sessionTip = document.createElement("div");
    sessionTip.className = "session-tip";
    document.body.appendChild(sessionTip);
  }
  return sessionTip;
}

function showSessionTip(el, text) {
  const tip = getSessionTip();
  tip.textContent = text;
  tip.style.visibility = "hidden";
  tip.style.display = "block";
  const tw = tip.offsetWidth;
  const th = tip.offsetHeight;
  const rect = el.getBoundingClientRect();
  let left = rect.left + rect.width / 2 - tw / 2;
  left = Math.max(4, Math.min(left, window.innerWidth - tw - 4));
  let top = rect.top - th - 8;
  if (top < 4) top = rect.bottom + 8;
  if (top + th > window.innerHeight - 4) top = window.innerHeight - th - 4;
  tip.style.left = left + "px";
  tip.style.top = top + "px";
  tip.style.visibility = "visible";
}

window.attachSessionTooltip = function (el, text) {
  el.__sessionTipText = text;
  if (el.dataset.tooltipSetup) return;
  el.dataset.tooltipSetup = "1";
  el.addEventListener("pointerenter", () => {
    if (sessionTipTimer) clearTimeout(sessionTipTimer);
    sessionTipTimer = setTimeout(() => showSessionTip(el, el.__sessionTipText), 800);
  });
  el.addEventListener("pointerleave", () => hideSessionTip());
};

// 仅更新已挂载 tooltip 的文本（供卡片复用时刷新提示内容，避免重复挂监听）
window.setSessionTooltip = function (el, text) {
  el.__sessionTipText = text;
};

// 供页面级事件委托复用（如设置页 [data-tip] 提示）
window.showSessionTip = showSessionTip;
window.hideSessionTip = function () {
  if (sessionTipTimer) {
    clearTimeout(sessionTipTimer);
    sessionTipTimer = null;
  }
  if (sessionTip) sessionTip.style.display = "none";
};

// ── 右键菜单共享工具 ─────────────────────────────────────

const contextMenuHolders = [];

window.registerContextMenu = function (holderRef) {
  contextMenuHolders.push(holderRef);
};

window.clampMenuPosition = function (menu, x, y) {
  const menuW = menu.offsetWidth;
  const menuH = menu.offsetHeight;
  let posX = x;
  let posY = y;
  if (x + menuW > window.innerWidth) posX = x - menuW;
  if (y + menuH > window.innerHeight) posY = y - menuH;
  if (posX < 0) posX = 0;
  if (posY < 0) posY = 0;
  menu.style.left = posX + "px";
  menu.style.top = posY + "px";
};

// ── 任务栏「已钉设备」名单（**设备页与音量页共用**）────────────────────
//
// ⭐ 为什么放 `common.js` 而不是某个 tab 的脚本：两个 tab 的右键菜单都要读它，
// 而 `popup-audio.js`（加载序 2/4）**先于** `popup-devices.js`（3/4）执行
// ⇒ 放在 devices 里会让 audio 在定义时拿到 `undefined`。共用状态必须回到
// 最先加载的公共层——这是加载序决定的，不是风格偏好。
//
// ⛔ 名单按**显示名**存：音量页的卡片名是**音频端点名**、设备页是
//   `PhysicalDevice.name`，两者字符串不同但指向同一台设备。按名判只是**文案**用途，
//   真正的写入一律由后端按身份键裁决，前端不参与判据。
// ⭐ 两份名单：**显示名**（设备页用）与**音频端点 id**（音量页用）。
//
// ⛔ 两者**必须分开**，因为两页卡片的原生标识不同：
//   · 设备信息页卡片是 `Device`，名字就是 `PhysicalDevice.name`；
//   · 音量控制页卡片是 `AudioDevice`，名字是**音频端点名**（`耳机 (小爱音箱-9205)`），
//     与 `PhysicalDevice.name`（`小爱音箱-9205`）**不是同一个字符串**。
//   ⇒ 音量页若按名字判「钉了没」，文案**永远不会**翻成「移出任务栏」（真机现象）。
//   ⇒ 端点 id 由后端一并返回（`PinnedTaskbarEntry.audio_ids`），各页按自己的标识判。
//   判据本身仍全在后端：这里只用于**菜单文案**。
let taskbarPinnedNameSet = new Set();
let taskbarPinnedAudioIdSet = new Set();
// ⭐ 「显示设备信息组件」开关（`config.taskbar_widget_enabled`）。
//   设备卡片的右键菜单里「钉到任务栏/移出任务栏」**只在它开启时出现**
//   ——组件关着时那个入口没有意义。
//
// ⚠️ 初值 `false` 是**刻意 fail-closed**（取不到就当关）：这条规则的方向是
//   「开启时才显示」，所以「未知」必须归到「不显示」，否则会在组件关闭时
//   漏出一个点不动的菜单项。
let taskbarWidgetEnabled = false;

window.isTaskbarPinnedName = function (name) {
  return taskbarPinnedNameSet.has(name);
};

window.isTaskbarPinnedAudioId = function (id) {
  return taskbarPinnedAudioIdSet.has(id);
};

window.isTaskbarWidgetEnabled = function () {
  return taskbarWidgetEnabled;
};

// ⭐ 用**已有的 config 对象**同步开关，供 `config-changed` handler 零取数调用。
// ⛔ 只在字段**确实是布尔**时才写：`emit` 的 payload 理论上可能缺字段
//   （旧版后端传空 payload），那时**保持上一份已知值**而不是回落 false。
window.applyTaskbarWidgetEnabled = function (cfg) {
  if (cfg && typeof cfg.taskbar_widget_enabled === "boolean") {
    taskbarWidgetEnabled = cfg.taskbar_widget_enabled;
  }
};

window.refreshTaskbarPinnedNames = async function () {
  const inv = window.getInvoke ? window.getInvoke() : null;
  if (!inv) return;
  // ⭐ 顺带取「显示设备信息组件」开关 ⇒ 与共用名单**同一处、同一次刷新**。
  //   两页都已在加载时调用本函数，菜单项的可见性因此与菜单文案**同源同新鲜**。
  //
  // ⛔⛔ 两个取数**各自兜底、互不牵连**：若直接 `Promise.all` 而不各自 catch，
  //   任一 reject 会把另一个的结果一起丢掉 ⇒ 开关一次瞬时失败就会拖垮
  //   **名单刷新**（那驱动菜单文案「钉到/移出」的翻转）。两者故障域不同，
  //   必须独立成败。
  const [rows, cfg] = await Promise.all([
    inv("get_pinned_taskbar_list").catch((e) => {
      console.warn("get_pinned_taskbar_list failed", e);
      return null;
    }),
    inv("get_config").catch((e) => {
      console.warn("get_config failed", e);
      return null;
    }),
  ]);
  // ⚠️ 失败（null）时**不清空**已有集合：菜单文案退到「钉到任务栏」比「全部显示已钉」更安全
  //    （最坏是文案不准，点下去仍由后端按身份键翻转真实状态）。
  if (rows) {
    taskbarPinnedNameSet = new Set(rows.map((r) => r.name));
    const ids = [];
    for (const r of rows) {
      for (const id of r.audio_ids || []) ids.push(id);
    }
    taskbarPinnedAudioIdSet = new Set(ids);
  }
  // ⚠️ 同理：取失败时**保持上一份已知值**，而不是回落 `false`。
  //   否则一次瞬时失败就会让「已钉到任务栏」暂时失去取消入口（设置页仍可移除，
  //   但那是另一处、另一条路径）。只有真的读到配置才覆盖。
  window.applyTaskbarWidgetEnabled(cfg);
};

window.hideAllContextMenus = function () {
  for (const holder of contextMenuHolders) {
    if (holder.menu) {
      holder.menu.remove();
      holder.menu = null;
    }
  }
};

document.addEventListener("click", hideAllContextMenus);

// 禁用浏览器默认右键菜单（两页共用）。原先写在 <body oncontextmenu="return false"> 上，
// 属内联事件属性，会被不含 'unsafe-inline' 的 CSP `script-src` 拦掉 —— 改为在此注册。
document.addEventListener("contextmenu", (e) => e.preventDefault());

// 勾选图标（context-menu-check）：各菜单选中态的统一构造入口
window.createCheckIcon = function () {
  const check = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  check.setAttribute("class", "context-menu-check");
  check.setAttribute("width", "12");
  check.setAttribute("height", "12");
  check.setAttribute("viewBox", "0 0 12 12");
  check.setAttribute("fill", "none");
  check.innerHTML = '<path d="M2 6L5 9L10 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
  return check;
};

// 子菜单展开箭头（内部实现细节，不对外）：与 createCheckIcon 同款 createElementNS 组装
function createChevronIcon() {
  const chevron = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  chevron.setAttribute("class", "context-menu-chevron");
  chevron.setAttribute("width", "10");
  chevron.setAttribute("height", "10");
  chevron.setAttribute("viewBox", "0 0 12 12");
  chevron.setAttribute("fill", "none");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", "M4 2L8 6L4 10");
  path.setAttribute("stroke", "currentColor");
  path.setAttribute("stroke-width", "1.5");
  path.setAttribute("stroke-linecap", "round");
  path.setAttribute("stroke-linejoin", "round");
  chevron.appendChild(path);
  return chevron;
}

// 子菜单外壳：悬停展开的二级菜单（分组/输出设备/会话路由/空间音效共用）。
// positionFn(submenu, groupItem, menu) 可注入自定义定位策略；缺省为锚定 groupItem 视口矩形。
window.createSubmenuShell = function (menu, label, positionFn) {
  const groupItem = document.createElement("div");
  groupItem.className = "context-menu-item context-menu-subitem";
  // label 来自调用点（含设备名等外部数据），一律以 textContent 写入，
  // 不拼 innerHTML —— 拼字符串会把设备名里的标记当结构解析（见 P1-10 残留注入面）
  const labelEl = document.createElement("span");
  labelEl.textContent = label;
  groupItem.appendChild(labelEl);
  groupItem.appendChild(createChevronIcon());

  const submenu = document.createElement("div");
  submenu.className = "context-menu context-submenu";
  submenu.style.display = "none";

  function addItem(text, checked, onClick) {
    const item = document.createElement("div");
    item.className = "context-menu-item" + (checked ? " selected" : "");

    const leading = document.createElement("span");
    leading.className = "context-menu-leading";
    if (checked) {
      leading.appendChild(createCheckIcon());
    }
    item.appendChild(leading);

    const textEl = document.createElement("span");
    textEl.textContent = text;
    item.appendChild(textEl);

    item.addEventListener("click", (e) => {
      e.stopPropagation();
      hideAllContextMenus();
      onClick();
    });
    submenu.appendChild(item);
  }

  function positionSubmenu() {
    if (positionFn) { positionFn(submenu, groupItem, menu); return; }
    const sw = submenu.offsetWidth;
    const sh = submenu.offsetHeight;
    const rect = groupItem.getBoundingClientRect();
    let left = rect.right - 7;
    if (left + sw > window.innerWidth) left = rect.left - sw + 7;
    if (left < 4) left = 4;
    let top = rect.top;
    if (top + sh > window.innerHeight) top = rect.top - sh + 7;
    submenu.style.left = left + "px";
    submenu.style.top = top + "px";
  }

  let closeTimer = null;
  let openSubmenuTimer = null;
  function openSubmenu() {
    clearTimeout(openSubmenuTimer);
    clearTimeout(closeTimer);
    submenu.style.display = "block";
    groupItem.classList.add("open");
    positionSubmenu();
  }
  function closeSubmenu() {
    clearTimeout(openSubmenuTimer);
    clearTimeout(closeTimer);
    submenu.style.display = "none";
    groupItem.classList.remove("open");
  }
  function queueCloseSubmenu() {
    clearTimeout(openSubmenuTimer);
    clearTimeout(closeTimer);
    closeTimer = setTimeout(closeSubmenu, 300);
  }
  function queueOpenSubmenu() {
    clearTimeout(openSubmenuTimer);
    clearTimeout(closeTimer);
    openSubmenuTimer = setTimeout(openSubmenu, 500);
  }

  groupItem.addEventListener("pointerenter", queueOpenSubmenu);
  groupItem.addEventListener("pointerleave", () => {
    clearTimeout(openSubmenuTimer);
    queueCloseSubmenu();
  });
  groupItem.addEventListener("click", (e) => {
    e.stopPropagation();
    if (submenu.style.display === "none") openSubmenu();
    else closeSubmenu();
  });
  submenu.addEventListener("pointerenter", () => {
    clearTimeout(openSubmenuTimer);
    clearTimeout(closeTimer);
  });
  submenu.addEventListener("pointerleave", queueCloseSubmenu);

  menu.addEventListener("pointerover", (e) => {
    if (!groupItem.contains(e.target) && !submenu.contains(e.target) && submenu.style.display !== "none") {
      closeSubmenu();
    }
  });

  function finish() {
    menu.appendChild(groupItem);
    menu.appendChild(submenu);
  }

  return { addItem, finish };
}


// ── ComboBox（WinUI 风格下拉的行为核心，设置页与「修改信息」对话框共用） ──

// 绑定一个 .win-combo-box 子树（静态 HTML 或 createComboBox 动态构造皆可）。
// 由 settings.js 的 initComboBox **逐字上移**而来 —— ⛔ 开合/定位/动画判据只此一份，
// 两页不许各写一份（漂移即两页下拉行为不一致）。
// 返回 { getValue }：取当前选中值（对话框「确定」要在收尾时读值）。
window.bindComboBox = function (combo, selectedValue, onChange) {
  const btn = combo.querySelector(".win-combo-btn");
  const flyout = combo.querySelector(".win-combo-flyout");
  const content = btn.querySelector(".win-combo-content");
  const items = combo.querySelectorAll(".win-combo-item");
  const ITEM_HEIGHT = 36;

  let currentValue = selectedValue;
  let overlay = null;
  let flyoutAnimation = null;

  function getSelectedIndex() {
    let idx = 0;
    items.forEach((item, i) => {
      if (item.dataset.value === currentValue) idx = i;
    });
    return idx;
  }

  function selectItem(value) {
    currentValue = value;
    items.forEach(item => {
      const isSelected = item.dataset.value === value;
      item.classList.toggle("selected", isSelected);
      if (isSelected) {
        content.textContent = item.querySelector(".win-combo-item-content").textContent;
      }
    });
  }

  function removeOverlay() {
    if (overlay && overlay.parentElement) {
      overlay.parentElement.removeChild(overlay);
    }
    overlay = null;
  }

  function cancelFlyoutAnimation() {
    if (flyoutAnimation) {
      flyoutAnimation.cancel();
      flyoutAnimation = null;
    }
    flyout.style.clipPath = "";
  }

  function playFlyoutAnimation() {
    cancelFlyoutAnimation();

    const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (prefersReducedMotion) return;

    const rect = flyout.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    // Edge strip at top (where button is), expand downward
    const stripSize = 36;
    const margin = 15;
    const startRect = { left: 0, right: rect.width, top: 0, bottom: stripSize };
    const endRect = { left: -margin, right: rect.width + margin, top: -margin, bottom: rect.height + margin };

    const toPolygon = (r) => `polygon(${r.left}px ${r.top}px, ${r.right}px ${r.top}px, ${r.right}px ${r.bottom}px, ${r.left}px ${r.bottom}px)`;

    flyoutAnimation = flyout.animate(
      [
        { clipPath: toPolygon(startRect) },
        { clipPath: toPolygon(endRect) }
      ],
      { duration: 800, easing: "cubic-bezier(0.092, 1.003, 0.028, 0.997)", fill: "none" }
    );

    flyoutAnimation.onfinish = () => {
      flyoutAnimation = null;
      flyout.style.clipPath = "";
    };
    flyoutAnimation.oncancel = () => {
      flyoutAnimation = null;
    };
  }

  function closeFlyout() {
    flyout.style.display = "none";
    flyout.style.visibility = "hidden";
    btn.setAttribute("aria-expanded", "false");
    combo.classList.remove("is-open");
    cancelFlyoutAnimation();
    removeOverlay();
    document.removeEventListener("pointerdown", onDocPointerDown);
    window.removeEventListener("scroll", onScroll, true);
    window.removeEventListener("resize", onScroll);
  }

  function positionFlyout() {
    const btnRect = btn.getBoundingClientRect();
    const viewportW = window.innerWidth;
    const viewportH = window.innerHeight;
    const itemCount = items.length;
    const selectedIndex = getSelectedIndex();

    flyout.style.visibility = "hidden";
    flyout.style.display = "block";
    flyout.style.top = "0px";
    flyout.style.left = "0px";
    flyout.style.width = "auto";
    flyout.style.maxWidth = viewportW + "px";

    const flyoutRect = flyout.getBoundingClientRect();
    const flyoutW = Math.max(btnRect.width, flyoutRect.width);

    const maxItems = Math.min(9, itemCount);
    const flyoutH = maxItems * ITEM_HEIGHT + 8;
    const selectedItemCenter = selectedIndex * ITEM_HEIGHT + ITEM_HEIGHT / 2 + 4;
    const btnCenter = btnRect.top + btnRect.height / 2;

    let popupTop = btnCenter - selectedItemCenter;
    if (popupTop + flyoutH > viewportH - 4) popupTop = viewportH - flyoutH - 4;
    if (popupTop < 4) popupTop = 4;

    let popupLeft = btnRect.left;
    if (popupLeft + flyoutW > viewportW - 4) popupLeft = Math.max(4, viewportW - flyoutW - 4);

    flyout.style.top = Math.round(popupTop) + "px";
    flyout.style.left = Math.round(popupLeft) + "px";
    flyout.style.width = Math.round(flyoutW) + "px";
    flyout.style.maxHeight = flyoutH + "px";
    flyout.style.visibility = "visible";

    const scrollTop = Math.max(0, selectedItemCenter - btnCenter + popupTop);
    flyout.scrollTop = scrollTop;
  }

  function onDocPointerDown(e) {
    if (!combo.contains(e.target) && !flyout.contains(e.target)) closeFlyout();
  }

  function onScroll() {
    if (combo.classList.contains("is-open")) positionFlyout();
  }

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    // 状态源是类，不是内联样式：`is-open` 由下方开/closeFlyout 完整维护。
    // 原先回读 `flyout.style.display`，而初始态一旦改由样式表给出（CSP 收紧后必须如此），
    // 内联值就是空串、该表达式恒为 true，首点会走 closeFlyout()（关一个本来就关着的浮层）。
    const isOpen = combo.classList.contains("is-open");
    if (isOpen) {
      closeFlyout();
    } else {
      removeOverlay();
      overlay = document.createElement("div");
      overlay.className = "win-combo-overlay";
      document.body.appendChild(overlay);

      document.body.appendChild(flyout);

      btn.setAttribute("aria-expanded", "true");
      combo.classList.add("is-open");
      selectItem(currentValue);
      positionFlyout();
      playFlyoutAnimation();

      window.addEventListener("scroll", onScroll, true);
      window.addEventListener("resize", onScroll);
      setTimeout(() => document.addEventListener("pointerdown", onDocPointerDown), 0);
    }
  });

  items.forEach(item => {
    item.addEventListener("click", (e) => {
      e.stopPropagation();
      const oldValue = currentValue;
      selectItem(item.dataset.value);
      closeFlyout();
      if (onChange) {
        const result = onChange(currentValue);
        if (result && typeof result.then === "function") {
          result.catch(() => selectItem(oldValue));
        }
      }
    });
  });

  selectItem(currentValue);

  return {
    getValue: () => currentValue,
    // ⭐ 宿主关闭时必须调用：开启浮层会把 flyout **搬到 document.body**、并挂上
    //   document 级 pointerdown / scroll / resize 监听 —— 对话框 remove 掉自身
    //   DOM 时够不到这些，不销毁就泄漏（设置页静态下拉与页面同寿命，无需调用）。
    destroy: () => {
      if (combo.classList.contains("is-open")) closeFlyout();
      if (flyout.parentElement) flyout.parentElement.removeChild(flyout);
    },
  };
};

// 造一个 win-combo 下拉（结构与 settings.html 静态标记逐字段一致：btn/content/
// chevron svg/flyout/items-presenter/item-layout/pill/item-content），
// 行为交给 bindComboBox。options = [[value, label], ...]。
window.createComboBox = function ({ options, value, onChange }) {
  const combo = document.createElement("div");
  combo.className = "win-combo-box";

  const btn = document.createElement("button");
  btn.className = "win-combo-btn";
  btn.setAttribute("role", "combobox");
  btn.setAttribute("aria-expanded", "false");

  const content = document.createElement("span");
  content.className = "win-combo-content";
  btn.appendChild(content);

  const chevron = document.createElement("span");
  chevron.className = "win-combo-chevron";
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "12");
  svg.setAttribute("height", "12");
  svg.setAttribute("viewBox", "0 0 8.098 4.598");
  svg.setAttribute("fill", "none");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d",
    "M0.195 0.195C0.39 0 0.708 0 0.903 0.195L4.049 3.342 7.195 0.195C7.39 0 7.708 0 7.903 0.195"
    + "C8.098 0.39 8.098 0.708 7.903 0.903L4.403 4.403C4.208 4.598 3.89 4.598 3.695 4.403"
    + "L0.195 0.903C0 0.708 0 0.39 0.195 0.195Z");
  path.setAttribute("fill", "currentColor");
  svg.appendChild(path);
  chevron.appendChild(svg);
  btn.appendChild(chevron);

  const flyout = document.createElement("div");
  flyout.className = "win-combo-flyout";
  const presenter = document.createElement("div");
  presenter.className = "win-combo-items-presenter";
  for (const [val, label] of options) {
    const item = document.createElement("button");
    item.className = "win-combo-item";
    item.dataset.value = val;
    const layout = document.createElement("span");
    layout.className = "win-combo-item-layout";
    const pill = document.createElement("span");
    pill.className = "win-combo-item-pill";
    const itemContent = document.createElement("span");
    itemContent.className = "win-combo-item-content";
    itemContent.textContent = label;
    layout.appendChild(pill);
    layout.appendChild(itemContent);
    item.appendChild(layout);
    presenter.appendChild(item);
  }
  flyout.appendChild(presenter);

  combo.appendChild(btn);
  combo.appendChild(flyout);

  const binding = window.bindComboBox(combo, value, onChange);
  return { root: combo, getValue: binding.getValue, destroy: binding.destroy };
};

// ── 修改信息对话框 ─────────────────────────────────────────

// 下拉选项与后端白名单（config::DEVICE_TYPES / CONN_KINDS）一一对应；
// 空值 = 自动探测（后端把空值当清除键，恢复自动派生）。
// ⛔ 选项增删必须与后端两个常量数组同步，前端不单方面扩集合。
const DEVICE_TYPE_OPTIONS = [
  ["", "默认"],
  ["phone", "手机"],
  ["tablet", "平板"],
  ["headphones", "耳机"],
  ["speaker", "音箱"],
  ["gamepad", "手柄"],
  ["keyboard", "键盘"],
  ["mouse", "鼠标"],
];

const CONN_KIND_OPTIONS = [
  ["", "默认"],
  ["wired", "有线"],
  ["bluetooth", "蓝牙"],
  ["wireless_24g", "2.4G"],
];

window.showDeviceInfoDialog = function ({
  deviceName,
  displayName,
  nameSource,
  deviceKey,
  deviceType,
  connKind,
  onUpdate,
  onRender,
}) {
  const input = document.createElement("input");
  input.type = "text";
  input.className = "dialog-input";
  input.value = displayName;
  input.placeholder = "输入新名称";

  const typeCombo = createComboBox({ options: DEVICE_TYPE_OPTIONS, value: deviceType || "" });
  const connCombo = createComboBox({ options: CONN_KIND_OPTIONS, value: connKind || "" });

  // 三行「标签 + 控件」复用 .card-item 行样式 —— 与设置页低电量通知·电量阈值
  // 行同一套类（同一视觉口径，不另写一套行样式）
  const makeFieldRow = (labelText, control) => {
    const row = document.createElement("div");
    row.className = "card-item";
    const label = document.createElement("div");
    label.className = "card-item-name";
    label.textContent = labelText;
    const controls = document.createElement("div");
    controls.className = "card-item-controls";
    controls.appendChild(control);
    row.appendChild(label);
    row.appendChild(controls);
    return row;
  };

  const fields = document.createElement("div");
  fields.className = "dialog-fields";
  // 设备命名行不带标签（用户点名只留输入框）：input 直挂 .card-item，width:100% 占满整行
  const nameRow = document.createElement("div");
  nameRow.className = "card-item";
  nameRow.appendChild(input);
  fields.appendChild(nameRow);
  fields.appendChild(makeFieldRow("设备类型", typeCombo.root));
  fields.appendChild(makeFieldRow("连接方式", connCombo.root));

  const buttons = [];

  // 统一关闭：先销毁两个下拉（flyout 在 body 下 + document 级监听，见 destroy 注释）
  const close = () => {
    typeCombo.destroy();
    connCombo.destroy();
    closeDialog(overlay);
  };

  // 「恢复默认」= 三项全部清空：命名清空走后端归并删除，类型/连接清空走覆盖表删键
  buttons.push({
    text: "恢复默认",
    className: "danger",
    onClick: async () => {
      const invoke = getInvoke();
      if (invoke) {
        await invoke("set_device_info", {
          original: deviceName,
          deviceKey: deviceKey || "",
          newName: "",
          deviceType: "",
          connKind: "",
        });
        const config = await invoke("get_config");
        onUpdate(config);
        onRender();
      }
      close();
    },
  });

  buttons.push({
    text: "取消",
    className: "cancel",
    onClick: () => close(),
  });

  buttons.push({
    text: "确定",
    className: "confirm",
    onClick: async () => {
      const newName = input.value.trim();
      const invoke = getInvoke();
      if (invoke) {
        await invoke("set_device_info", {
          original: deviceName,
          deviceKey: deviceKey || "",
          newName,
          deviceType: typeCombo.getValue(),
          connKind: connCombo.getValue(),
        });
        const config = await invoke("get_config");
        onUpdate(config);
        onRender();
      }
      close();
    },
  });

  const overlay = createDialog({
    title: "修改信息",
    content: [fields],
    buttons,
  });

  // Escape 关闭走 createDialog 内置的 overlay.remove()，绕过 close() ⇒ 这里补销毁
  overlay.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      typeCombo.destroy();
      connCombo.destroy();
    }
  });

  // 有任一自定义信息（别名 / 类型 / 连接）才亮「恢复默认」
  const hasOverride = nameSource !== undefined || deviceType || connKind;
  const restoreBtn = overlay.querySelector(".dialog-btn.danger");
  if (restoreBtn) restoreBtn.disabled = !hasOverride;

  input.focus();
  input.select();

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") overlay.querySelector(".dialog-btn.confirm")?.click();
  });
};

// ── 快捷键（共享工具） ───────────────────────────────────

// 快捷键保存失败文案归一：后端错误串 -> 用户可读提示（toast/hint 呈现方式由调用方决定）
window.describeShortcutError = function (err, display) {
  const msg = String(err);
  // 顺序要紧：「已被其他程序占用」不含「已被占用」子串，但仍显式先判，
  // 以免日后有人把文案改成含该子串而落到下面那条更含糊的提示上（P2-12）。
  if (msg.includes("被其他程序占用")) {
    return `"${display}" 已被其他程序占用，请换一个快捷键。`;
  }
  return msg.includes("已被占用")
    ? `"${display}" 已被其他功能占用，请选择其他快捷键。`
    : "暂不支持该快捷键。";
};

// ── 快捷键录制（共享工具） ────────────────────────────────

// 快捷键码表：本文件录制器内部实现细节，不挂载到 window（外部无消费者）
const shortcutCodeMap = {
  "Space": { display: "Space", key: "Space" },
  "Backspace": { display: "Backspace", key: "Backspace" },
  "Delete": { display: "Delete", key: "Delete" },
  "Tab": { display: "Tab", key: "Tab" },
  "CapsLock": { display: "CapsLock", key: "CapsLock" },
  "Escape": { display: "Escape", key: "Escape" },
  "Insert": { display: "Insert", key: "Insert" },
  "Home": { display: "Home", key: "Home" },
  "End": { display: "End", key: "End" },
  "PageUp": { display: "PageUp", key: "PageUp" },
  "PageDown": { display: "PageDown", key: "PageDown" },
  "ArrowUp": { display: "↑", key: "ArrowUp" },
  "ArrowDown": { display: "↓", key: "ArrowDown" },
  "ArrowLeft": { display: "←", key: "ArrowLeft" },
  "ArrowRight": { display: "→", key: "ArrowRight" },
  "PrintScreen": { display: "PrtSc", key: "PrintScreen" },
  "ScrollLock": { display: "ScrLk", key: "ScrollLock" },
  "Pause": { display: "Pause", key: "Pause" },
  "NumLock": { display: "NumLock", key: "NumLock" },
  "Numpad0": { display: "Num0", key: "Numpad0" },
  "Numpad1": { display: "Num1", key: "Numpad1" },
  "Numpad2": { display: "Num2", key: "Numpad2" },
  "Numpad3": { display: "Num3", key: "Numpad3" },
  "Numpad4": { display: "Num4", key: "Numpad4" },
  "Numpad5": { display: "Num5", key: "Numpad5" },
  "Numpad6": { display: "Num6", key: "Numpad6" },
  "Numpad7": { display: "Num7", key: "Numpad7" },
  "Numpad8": { display: "Num8", key: "Numpad8" },
  "Numpad9": { display: "Num9", key: "Numpad9" },
  "NumpadAdd": { display: "Num+", key: "NumpadAdd" },
  "NumpadSubtract": { display: "Num-", key: "NumpadSubtract" },
  "NumpadMultiply": { display: "Num*", key: "NumpadMultiply" },
  "NumpadDivide": { display: "Num/", key: "NumpadDivide" },
  "NumpadDecimal": { display: "Num.", key: "NumpadDecimal" },
  "NumpadEnter": { display: "NumEnter", key: "NumpadEnter" },
  "MediaPlayPause": { display: "MediaPlayPause", key: "MediaPlayPause" },
  "MediaStop": { display: "MediaStop", key: "MediaStop" },
  "MediaNextTrack": { display: "MediaNextTrack", key: "MediaNextTrack" },
  "MediaPrevTrack": { display: "MediaPrevTrack", key: "MediaPrevTrack" },
  "VolumeUp": { display: "VolumeUp", key: "VolumeUp" },
  "VolumeDown": { display: "VolumeDown", key: "VolumeDown" },
  "VolumeMute": { display: "VolumeMute", key: "VolumeMute" },
  "Semicolon": { display: ";", key: "Semicolon" },
  "Equal": { display: "=", key: "Equal" },
  "Comma": { display: ",", key: "Comma" },
  "Period": { display: ".", key: "Period" },
  "Slash": { display: "/", key: "Slash" },
  "Backquote": { display: "`", key: "Backquote" },
  "Backslash": { display: "\\", key: "Backslash" },
  "BracketLeft": { display: "[", key: "BracketLeft" },
  "BracketRight": { display: "]", key: "BracketRight" },
  "Minus": { display: "-", key: "Minus" },
  "Quote": { display: "'", key: "Quote" },
  "Enter": { display: "Enter", key: "Enter" },
};

const shortcutReverseCodeMap = {};
for (const v of Object.values(shortcutCodeMap)) {
  shortcutReverseCodeMap[v.key] = v.display;
}

function shortcutJoinSaved(saved) {
  if (!saved) return "";
  return saved.split("+").map(p => {
    if (shortcutReverseCodeMap[p]) return shortcutReverseCodeMap[p];
    if (p.length === 4 && p.startsWith("Key")) return p[3];
    if (p.length === 6 && p.startsWith("Digit")) return p[5];
    return p;
  }).join("+");
};

// 绑定快捷键输入框录制行为。input/clearBtn 为 DOM 元素，getSavedKey() 返回当前保存的原始快捷键（含 Super）
// onSaved(display, shortcut) 在录制成功或点击清除时回调（清除时 shortcut 为空串）；onError(msg) 在失败时回调
const shortcutRecorders = new Set();
let shortcutRecordListenerReady = false;
function ensureShortcutRecordListener() {
  if (shortcutRecordListenerReady) return;
  shortcutRecordListenerReady = true;
  // 已注册为全局快捷键的组合键，其按键事件可能被系统吞掉而收不到 keydown，
  // 由后端在录制期间直接上报按下的组合键。
  onTauriEvent("shortcut-recorded", (event) => {
    const key = event.payload;
    if (!key) return;
    for (const rec of shortcutRecorders) rec.recordFromBackend(key);
  });
}

window.bindShortcutRecorder = function (input, clearBtn, getSavedKey, onSaved, onError) {
  let recording = false;
  let keys = new Set();

  function setRecordingFlag(on) {
    try {
      invoke("set_shortcut_recording", { recording: on }).catch(() => {});
    } catch (_) {}
  }

  function resetRecording() {
    recording = false;
    keys.clear();
    input.classList.remove("recording");
    input.placeholder = "点击录制快捷键";
    setRecordingFlag(false);
  }

  function restoreSaved() {
    resetRecording();
    const savedKey = getSavedKey();
    input.value = savedKey ? shortcutJoinSaved(savedKey).replace("Super", "Win") : "";
    if (clearBtn) clearBtn.style.display = savedKey ? "" : "none";
  }

  input.addEventListener("click", () => {
    if (recording) return;
    recording = true;
    keys.clear();
    input.value = "";
    input.classList.add("recording");
    input.placeholder = "请按下组合键...";
    setRecordingFlag(true);
  });

  input.addEventListener("blur", () => {
    resetRecording();
  });

  input.addEventListener("keydown", (e) => {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      restoreSaved();
      return;
    }

    keys.clear();
    if (e.ctrlKey) keys.add({ display: "Ctrl", key: "Ctrl" });
    if (e.shiftKey) keys.add({ display: "Shift", key: "Shift" });
    if (e.altKey) keys.add({ display: "Alt", key: "Alt" });
    if (e.metaKey) keys.add({ display: "Win", key: "Super" });

    const code = e.code;
    if (code === "ControlLeft" || code === "ControlRight" ||
        code === "ShiftLeft" || code === "ShiftRight" ||
        code === "AltLeft" || code === "AltRight" ||
        code === "MetaLeft" || code === "MetaRight") {
      const preview = [...keys].map(k => k.display).join("+");
      input.value = preview;
      return;
    }

    if (code.startsWith("Numpad") && /\d/.test(code[6]) && code.length === 7) {
      keys.add({ display: "Num" + code[6], key: code });
    } else if (shortcutCodeMap[code]) {
      const entry = shortcutCodeMap[code];
      keys.add({ display: entry.display, key: entry.key });
    } else if (code.startsWith("F") && code.length >= 2 && code.length <= 3) {
      keys.add({ display: code, key: code });
    } else if (code.startsWith("Digit") && code.length === 6) {
      keys.add({ display: code[5], key: code });
    } else if (code.startsWith("Key") && code.length === 4) {
      keys.add({ display: code[3], key: code });
    } else {
      restoreSaved();
      if (onError) onError("暂不支持该快捷键。");
      return;
    }

    const display = [...keys].map(k => k.display).join("+");
    const shortcut = [...keys].map(k => k.key).join("+");
    if (display) {
      recording = false;
      input.value = display;
      input.classList.remove("recording");
      input.placeholder = "点击录制快捷键";
      // 延迟释放录制标志，确保本次按键的全局快捷键分发已被抑制
      setTimeout(() => setRecordingFlag(false), 300);
      if (onSaved) onSaved(display, shortcut.replace("Win", "Super"));
    }
  });

  if (clearBtn) {
    clearBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (onSaved) onSaved("", "");
    });
  }

  function recordFromBackend(canonicalKey) {
    if (!recording) return;
    if (!canonicalKey) return;
    const display = shortcutJoinSaved(canonicalKey);
    if (!display) return;
    recording = false;
    keys.clear();
    input.value = display;
    input.classList.remove("recording");
    input.placeholder = "点击录制快捷键";
    // 延迟释放录制标志，确保本次按键的全局快捷键分发已被抑制
    setTimeout(() => setRecordingFlag(false), 300);
    if (onSaved) onSaved(display, canonicalKey);
  }

  const self = { recordFromBackend };
  shortcutRecorders.add(self);
  ensureShortcutRecordListener();

  restoreSaved();

  // 返回 dispose 函数，用于从 Set 中移除 recorder，防止内存泄漏
  function dispose() {
    shortcutRecorders.delete(self);
  }
  return { restore: restoreSaved, dispose };
};

// ── 对话框 ───────────────────────────────────────────────

window.createDialog = function ({ title, content = [], buttons = [] }) {
  const overlay = document.createElement("div");
  overlay.className = "dialog-overlay";

  const dialog = document.createElement("div");
  dialog.className = "rename-dialog";

  const contentEl = document.createElement("div");
  contentEl.className = "dialog-content";

  const titleEl = document.createElement("div");
  titleEl.className = "dialog-title";
  titleEl.textContent = title;
  contentEl.appendChild(titleEl);

  for (const el of content) {
    contentEl.appendChild(el);
  }
  dialog.appendChild(contentEl);

  if (buttons.length > 0) {
    const buttonsEl = document.createElement("div");
    buttonsEl.className = "dialog-buttons";
    for (const btn of buttons) {
      const btnEl = document.createElement("button");
      btnEl.className = `dialog-btn ${btn.className || ""}`;
      btnEl.textContent = btn.text;
      btnEl.addEventListener("click", btn.onClick);
      buttonsEl.appendChild(btnEl);
    }
    dialog.appendChild(buttonsEl);
  }

  overlay.appendChild(dialog);
  document.body.appendChild(overlay);

  overlay.addEventListener("keydown", (e) => {
    if (e.key === "Escape") overlay.remove();
  });

  return overlay;
};

window.closeDialog = function (overlay) {
  if (overlay && overlay.parentNode) {
    overlay.remove();
  }
};

// ── Toast 通知 ──────────────────────────────────────────

/**
 * 弹出一条 toast。
 *
 * ⚠️ **文案是纯文本，不是 HTML**：本函数用 `textContent` 写入（防 XSS，必须保持），
 * 所以文案里若写 HTML 换行标签，会被原样显示成字面量的标签文本。
 * 需要换行请写 `\n`——`.toast` 已设 `white-space: pre-line`（见 base.css）。
 * `tools/check.mjs` 会扫描本函数的实参并在出现 HTML 标签时报错（P1-6）。
 */
window.showToast = function (msg, onClick, isError, durationMs) {
  let el = document.querySelector(".toast");
  if (!el) {
    el = document.createElement("div");
    el.className = "toast";
    document.body.appendChild(el);
  }
  // 使用 textContent 防止 XSS
  el.textContent = msg;
  el.classList.toggle("error", !!isError);
  el.classList.add("show");
  el.style.cursor = onClick ? "pointer" : "default";
  el.onclick = onClick || null;
  clearTimeout(el._timer);
  el._timer = setTimeout(() => {
    el.classList.remove("show");
    el.classList.remove("error");
    el.onclick = null;
    el.style.cursor = "default";
  }, durationMs || 5000);
};

// ── 启动时配置解析失败提示（P1-7）───────────────────────
// 后端解析 config.toml 失败时会回退默认值，并把磁盘原文备份为 config.toml.bak。
// 不提示的话，用户只会看到「设置全变回默认」，容易误判成静默丢数据。
// 两个页面都加载 common.js，故提示逻辑放在这里（非清除式，两个窗口都能看到）。
window.addEventListener("DOMContentLoaded", async () => {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    const msg = await invoke("get_config_load_error");
    // 停留时间给足：消息里含备份文件的完整路径，5s 读不完
    if (msg) {
      window.showToast(msg, null, true, 15000);
      // 留一条后端记录：证明提示链路真的走到了前端（否则异常会被 catch 静默吞掉，
      // 而「用户到底有没有被告知」将无从查证）
      invoke("frontend_log", { tag: "config-notice", msg: msg }).catch(() => {});
    }
  } catch (_) {
    // 提示失败不得影响主流程
  }

  // P2-12：注册失败的设备快捷键。**必须主动拉取**——失败事件在启动同步时发出，
  // 那一刻本页面还没加载、监听器尚未注册，事件必然落空。而「开机时快捷键被别的
  // 程序抢走」正是这类失效最常见也最隐蔽的场景（界面显示已设置、按键却无反应）。
  try {
    const keys = await invoke("get_shortcut_register_failed");
    if (Array.isArray(keys) && keys.length) {
      window.showToast(
        `快捷键 ${keys.join("、")} 注册失败：可能已被其他程序占用`,
        null,
        true,
        15000
      );
      invoke("frontend_log", {
        tag: "shortcut-register-failed",
        msg: `启动拉取 ${keys.join(",")}`,
      }).catch(() => {});
    }
  } catch (_) {
    // 同上：提示失败不得影响主流程
  }
});

// ── 设备快捷键注册失败（P2-12）─────────────────────────
// 被**其他程序**占用的键，只有真正调注册 API 时才会失败——本进程的注册表查不到外部占用。
// 后端在失败时广播本事件，避免「界面显示已设置、按键却毫无反应」这种无从察觉的失效。
// 注：用户主动设键走 `set_device_shortcut`，那条路径会返回错误并自行提示；
// 本监听覆盖的是启动同步、关闭共享开关、删除设备等没有直接返回值的路径。
onTauriEvent("shortcut-register-failed", (event) => {
  const keys = Array.isArray(event.payload) ? event.payload : [];
  if (!keys.length) return;
  window.showToast(
    `快捷键 ${keys.join("、")} 注册失败：可能已被其他程序占用`,
    null,
    true,
    8000
  );
  // 留一条后端记录：证明提示链路真的走到了前端（否则异常会被静默吞掉，
  // 而「用户到底有没有被告知」将无从查证）——与 P1-7 的 config 提示同一手法。
  const invoke = getInvoke();
  if (invoke) {
    invoke("frontend_log", {
      tag: "shortcut-register-failed",
      msg: keys.join(","),
    }).catch(() => {});
  }
});

// ── 启动时更新检测（全局监听） ─────────────────────────
onTauriEvent("update-available", (event) => {
  const info = event.payload;
  const isStore = info.release_url && info.release_url.startsWith("ms-windows-store://");
  if (isStore) {
    window.showToast(
      "Microsoft Store 有新版本可用\n点击前往更新",
      () => invoke("open_url", { url: info.release_url })
    );
  } else {
    window.showToast(
      `发现新版本 ${info.latest_version}（当前 ${info.current_version}）\n点击前往下载`,
      () => invoke("open_url", { url: info.release_url })
    );
  }
});
