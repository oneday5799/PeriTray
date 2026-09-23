/* settings-taskbar.js — 设置页·任务栏 tab：任务栏信息窗口的显示设备选择与窗口位置固定
 * 加载序 6/8 · 提供：initTaskbarTab()
 * 依赖：common.js(showToast 未用，见下) /
 *       settings.js(createExpandableCard/initComboBox/createCheckableMenu)
 *
 * ⚠️ 本页当前为「仅 UI」阶段：控件只做前端交互，**不读写 config、不调用 saveConfig**。
 *    因此刻意不复用 bindToggle —— 它内部会 `await saveConfig()` 落盘，与本阶段约束冲突；
 *    这里改为手写 change 监听。接线真实配置时再切回 bindToggle 并补 Config 字段
 *    （新增字段必须同步补 config.rs 的 for_each_config_field!，否则闸门单测转红）。 */
function initTaskbarTab() {
  initTaskbarDevicePicker();
  initTaskbarPinCard();
}

// 「在任务栏显示的设备」——交互范式对齐「强制静音」：点击弹出复选菜单。
// 本阶段设备数据源未接线，故传空 items，仅渲染空态文案（验证菜单壳与定位接线正常）。
function initTaskbarDevicePicker() {
  const btn = document.getElementById("btn-taskbar-devices");
  if (!btn) return;
  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    createCheckableMenu({
      anchor: btn,
      items: [],
      checked: new Set(),
      emptyText: "功能开发中，暂未开放设备选择",
      onToggle: () => {},
    });
  });
}

// 「固定任务栏窗口位置」——带开关的折叠卡，对齐「关机/重启时自动调整音量」。
function initTaskbarPinCard() {
  const card = document.getElementById("taskbar-pin-card");
  const toggle = document.getElementById("toggle-taskbar-pin");
  const items = document.getElementById("taskbar-pin-items");
  const arrow = document.getElementById("arrow-taskbar-pin");
  if (!card || !toggle || !items) return;

  const expandable = createExpandableCard(items, arrow);

  // 开关联动展开。此处 tab 可能不可见，用 "content" 会读到 scrollHeight=0
  // （.tab-content 基础态 display:none）⇒ 交互路径一律显式给固定上限。
  toggle.addEventListener("change", () => {
    expandable.set(toggle.checked, "999px");
  });

  // 初始化落位：开关默认开启（HTML 带 checked）⇒ 卡片展开。
  // ⭐ 必须用固定值而非 "content"：初始化时本 tab 尚未 active，scrollHeight 恒为 0，
  //    用 "content" 会算出 max-height:0 ⇒ 首次切到本页时卡片看起来没展开。
  expandable.setInstant(toggle.checked, "999px");

  expandable.bindHeaderClick(card, {
    extraGuards: [".toggle", "input"],
    expandHeight: "999px",
  });

  // 「任务栏窗口位置」下拉：默认居中。onChange 传 null ⇒ 只切显示，不落盘。
  initComboBox("combo-taskbar-position", "center", null);
}
