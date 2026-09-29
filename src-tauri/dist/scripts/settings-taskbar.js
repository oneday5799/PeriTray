/* settings-taskbar.js — 设置页·任务栏 tab：设备信息组件 + 音乐控制组件（两个独立开关）
 * 加载序 6/8 · 提供：initTaskbarTab()
 * 依赖：common.js / settings.js(config/bindToggle/initComboBox/createExpandableCard/saveConfig)
 *
 * ⭐ 本页控件全部接线真实数据：
 *    · 「显示设备信息组件」开关 → `config.taskbar_widget_enabled`（默认关）
 *    · 「显示音乐控制组件」开关 → `config.taskbar_music_enabled`（默认关，**独立**）
 *    · 已添加设备清单 + 「移除」 → `get_pinned_taskbar_list` / `toggle_pinned_taskbar_device`
 *    · 「固定任务栏窗口位置」开关 → `config.taskbar_position_locked`
 *    · 「任务栏窗口位置」下拉     → `config.taskbar_position`（left/center/right）
 *    · 「任务栏内容缩放大小」下拉 → `config.taskbar_content_scale`（default/follow_system）
 *
 * ⭐ 两个组件开关**互相独立**（用户 2026-09-28）：关掉「显示设备信息组件」**不会**
 *    关掉「显示音乐控制组件」，反之亦然。两个都开时，任务栏组件最右侧出现
 *    「切换」按钮（由后端 `taskbar_widget` 处理，本页不参与）。
 *
 * ⛔ 设备**添加**入口已从本页移除（用户 2026-09-28）：改到**弹出窗口**的设备卡片
 *    右键菜单「钉到任务栏」，与托盘的「添加到托盘」同一范式。本页只保留
 *    「查看已添加 + 移除」，且清单**必须含已断开设备**。
 *
 * ⛔ 落盘**不是可选的**：后端 `config::taskbar_panel_for()` 直接读本页写的字段
 *    决定组件显示哪一块（2026-09-28 起判据由「设备开关 ∧ 已钉设备」升为**三态**：
 *    音乐可用→音乐面板、否则设备可用→设备面板、否则不显示），窗口挂载/拆除/重绘
 *    全由 `config-changed` 驱动 ⇒ 这里不落盘就等于**控件是死的**。 */
function initTaskbarTab() {
  initTaskbarWidgetCard();
  initTaskbarMusicCard();
  initTaskbarPinCard();
  initTaskbarContentScale();
}
// ── 任务栏组件：总开关 + 已添加设备清单 ────────────────────────────
//
// ⭐ 开关落 `config.taskbar_widget_enabled`（**默认关**，用户 2026-09-28 口径）。
// ⛔ **关闭不得清空设备列表** —— 用户要求「关闭后保留设备信息，方便重新打开」；
//    后端 `should_show()` 也据此判为「开关 ∧ 列表非空」，两侧口径必须一致。
// ⭐ 展开区列出**已添加**的设备（含已断开，置灰呈现）+ 每行「移除」按钮；
//    设备本身由弹出窗口右键菜单「钉到任务栏」添加，本页不再提供选择器。
function initTaskbarWidgetCard() {
  const card = document.getElementById("taskbar-widget-card");
  const toggle = document.getElementById("toggle-taskbar-widget");
  const items = document.getElementById("taskbar-widget-items");
  const arrow = document.getElementById("arrow-taskbar-widget");
  const list = document.getElementById("taskbar-widget-devices");
  const empty = document.getElementById("taskbar-widget-empty");
  if (!card || !toggle || !items || !list) return;

  const expandable = createExpandableCard(items, arrow);

  // ⭐ 开关 → `config.taskbar_widget_enabled`。
  // ⚠️ 判据写 `=== true` 而不是 `!!get()`：`!!undefined` 会把「老配置缺键」翻成 true，
  //    与「默认关闭」相悖（后端读入时已把老配置迁移成显式 true）。
  bindToggle("toggle-taskbar-widget", {
    get: () => config.taskbar_widget_enabled === true,
    set: (v) => { config.taskbar_widget_enabled = !!v; },
  });

  // 开关联动展开（与「固定位置」卡同款；用固定高度，理由见那边注释）
  toggle.addEventListener("change", () => {
    expandable.set(toggle.checked, "999px");
  });
  // ⭐ 初始展开态**跟随开关**：开关开 ⇒ 展开（用户要看设备清单）；关 ⇒ 收起。
  expandable.setInstant(toggle.checked, "999px");
  expandable.bindHeaderClick(card, {
    extraGuards: [".toggle", "input"],
    expandHeight: "999px",
  });

  async function refreshList() {
    let rows = [];
    try {
      // ⭐ 后端**已含已断开设备**（`group_taskbar_devices` 的反向补建占位条目），
      //    故前端不需要（也不该）自己按 `connected` 过滤。
      rows = (await invoke("get_pinned_taskbar_list")) || [];
    } catch (err) {
      window.showToast("读取已添加设备失败：" + err);
      return;
    }
    list.textContent = "";
    // ⭐ 空态给一行说明，而不是留白 —— 否则用户会以为卡片坏了。
    if (empty) {
      empty.style.display = rows.length ? "none" : "";
      const txt = document.getElementById("taskbar-widget-empty-text");
      if (txt) {
        txt.textContent = rows.length
          ? ""
          : "尚未添加设备（可在弹出窗口的设备卡片右键菜单中添加）";
      }
    }
    for (const r of rows) {
      const row = document.createElement("div");
      row.className = "card-item";

      const name = document.createElement("div");
      name.className = "card-item-name";
      name.textContent = r.name;
      // ⭐ 已断开**置灰而非隐藏**：用户需要看见「这台我还钉着，只是没连」，
      //    否则会以为添加失败、反复去添加。
      if (!r.connected) {
        name.classList.add("dimmed");
        name.title = "设备当前未连接";
      }

      const controls = document.createElement("div");
      controls.className = "card-item-controls";
      const remove = document.createElement("button");
      remove.className = "add-device-btn";
      remove.textContent = "移除";
      remove.addEventListener("click", async () => {
        remove.disabled = true;
        try {
          // ⛔ 复用 `toggle_pinned_taskbar_device`（语义 = 已钉则取消）：它按
          //    `key` + `fallback` **删净**全部命中项，两层判据与显示侧完全一致。
          //    ⚠️ `alias: null` —— 别名不由本页设置（沿用旧选择器的口径）。
          await invoke("toggle_pinned_taskbar_device", {
            key: r.key,
            fallback: r.fallback ?? null,
            alias: null,
          });
          await refreshList();
        } catch (err) {
          window.showToast(String(err));
        } finally {
          remove.disabled = false;
        }
      });
      controls.appendChild(remove);

      row.appendChild(name);
      row.appendChild(controls);
      list.appendChild(row);
    }
  }

  // ⭐ 首次进入 tab 就拉清单（设备枚举 600ms+，不能放页面加载时）。
  //    再次进入时刷新：设备可能被弹出窗口「钉/移出」，或热插拔。
  refreshList();

  // ⛔ 切到本页时刷新。旧写法是 `if (view.classList.contains("active")) refreshList()`，
  //   **那是死代码**：`[data-tab="taskbar"]` 命中的是**导航项**，而 `.active` 是
  //   **面板**（`.tab-content active`）才有的 class，导航项用的是 `.is-selected`
  //   ⇒ 条件恒假 ⇒ 清单自初始化之后再没刷新过（在别处改名 / 钉移出都看不到）。
  const nav = document.querySelector('.win-nav-item[data-tab="taskbar"]');
  if (nav) {
    nav.addEventListener("click", () => {
      refreshList();
    });
  }

  // ⭐ 对外暴露给设置页的 `config-changed` 处理器：改名发生在**别的窗口**时，
  //   用户正停在任务栏 tab 上、也不会切 tab ⇒ 没有别的刷新时机。
  window.refreshTaskbarWidgetCardList = refreshList;
}


/** 「显示音乐控制组件」单开关卡（用户 2026-09-28 新增）。 */
function initTaskbarMusicCard() {
  const card = document.getElementById("taskbar-music-card");
  const toggle = document.getElementById("toggle-taskbar-music");
  if (!card || !toggle) return;

  // ⚠️ 判据写 `=== true` 而不是 `!!get()`：`!!undefined`（老配置缺键）会得到 false，
  //    而 false 恰是本项默认值 —— 结果对，但**读法有歧义**（分不清「显式关」与「没这个键」）。
  //    与上面那个开关的 `!== false` 写法**不同是有意的**：那边默认 true、本项默认 false，
  //    两边都得按「字段缺键时的默认值」选方向，别互相照抄。
  // ⚠️ 落盘是**必须的**：后端 `config::taskbar_panel_for` 直接读本字段，
  //    不落盘 = 开关点了没反应。
  bindToggle("toggle-taskbar-music", {
    get: () => config.taskbar_music_enabled === true,
    set: (v) => { config.taskbar_music_enabled = !!v; },
  });

  // ⛔ **刻意不加展开区**：音乐组件没有「已添加清单」这类内容。
}

function initTaskbarPinCard() {
  const card = document.getElementById("taskbar-pin-card");
  const toggle = document.getElementById("toggle-taskbar-pin");
  const items = document.getElementById("taskbar-pin-items");
  const arrow = document.getElementById("arrow-taskbar-pin");
  if (!card || !toggle || !items) return;

  const expandable = createExpandableCard(items, arrow);

  // ⭐ 开关落盘（`bindToggle` 内部 `saveConfig()`）——**必须放在读 `toggle.checked`
  //    做初始展开之前**：`bindToggle` 会先把 `checked` 同步成 config 值（覆盖 HTML 里
  //    写死的 `checked`）。若顺序反了，用户上次关掉开关后，卡片每次打开仍是展开的
  //    （HTML 默认值 ≠ 配置值）——正是「UI 看着正常、实际没生效」的静默形态。
  // ⚠️ 判据写 `!== false` 而不是直接取值：本字段语义是「默认 true」，
  //    老配置文件里可能缺键 ⇒ `!!undefined` 会得到 `false`，把默认值反转。
  bindToggle("toggle-taskbar-pin", {
    get: () => config.taskbar_position_locked !== false,
    set: (v) => { config.taskbar_position_locked = v; },
  });

  // 开关联动展开。此处 tab 可能不可见，用 "content" 会读到 scrollHeight=0
  // （.tab-content 基础态 display:none）⇒ 交互路径一律显式给固定上限。
  toggle.addEventListener("change", () => {
    expandable.set(toggle.checked, "999px");
  });

  // 初始化落位：按**配置值**（已由上面的 bindToggle 同步进 `toggle.checked`）决定展开。
  // ⭐ 必须用固定值而非 "content"：初始化时本 tab 尚未 active，scrollHeight 恒为 0，
  //    用 "content" 会算出 max-height:0 ⇒ 首次切到本页时卡片看起来没展开。
  expandable.setInstant(toggle.checked, "999px");

  expandable.bindHeaderClick(card, {
    extraGuards: [".toggle", "input"],
    expandHeight: "999px",
  });

  // 「任务栏窗口位置」下拉：初始值取 config（覆盖 HTML 里写死的「居中」文案），
  // 变更即落盘。范式与 `settings-general.js` 的 `combo-theme-mode` 等一致。
  // ⭐ 语义：贴靠发生在**避让后的视觉空白槽内**，不是整条任务栏 —— 见 config.rs 字段注释。
  initComboBox("combo-taskbar-position", config.taskbar_position || "center", async (val) => {
    config.taskbar_position = val;
    await saveConfig();
  });
}

// 「任务栏内容缩放大小」下拉 → `config.taskbar_content_scale`（default / smaller）。
//
// ⛔ **作用域**（用户 2026-09-25 指定）：只改**内容**（图标边长 / 信息文字字号 /
//    随内容缩放的间距与项宽上限），**底衬恒按系统 DPI**（窗口高度、圆角不受本项影响）。
//    落地在 `taskbar_widget::Metrics`：内容走 `content_dpi`，底衬走 `dpi`，两者分开换算。
//
// ⭐ 为什么单列一张卡而不是塞进上面的折叠卡：本项与「固定位置」开关**无关**，
//    放进折叠卡会在开关关闭时被一起收起 —— 用户会以为这个设置消失了。
function initTaskbarContentScale() {
  // ⭐ 初始值取 config（覆盖 HTML 里写死的「默认」文案）；变更即落盘。
  //    ⚠️ 落盘后由后端 `config-changed` → `apply_from_config` → `FORCE_REPAINT` +
  //       `refresh_async` 重算宽度并重绘 ⇒ **本项不需要重启即生效**（不是「下次启动才变」）。
  //    ⚠️ 缺键时回落 "default"：后端默认档也是它，两侧口径一致（旧配置文件无此键）。
  initComboBox(
    "combo-taskbar-content-scale",
    config.taskbar_content_scale || "default",
    async (val) => {
      config.taskbar_content_scale = val;
      await saveConfig();
    },
  );
}
