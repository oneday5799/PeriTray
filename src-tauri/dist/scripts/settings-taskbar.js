/* settings-taskbar.js — 设置页·任务栏 tab：任务栏信息窗口的显示设备选择与窗口位置固定
 * 加载序 6/8 · 提供：initTaskbarTab()
 * 依赖：common.js / settings.js(config/bindToggle/initComboBox/createExpandableCard/saveConfig)
 *
 * ⭐ 本页四个控件**全部接线真实数据**：
 *    · 设备选择（`initTaskbarDevicePicker`）→ `get_selectable_devices` / `toggle_pinned_taskbar_device`
 *      （设备页 ∪ 输出端点 ∪ 输入端点的**并集**；本文件不读 config，勾选态由后端 `pinned` 给出）
 *    · 「固定任务栏窗口位置」开关 → `config.taskbar_position_locked`
 *    · 「任务栏窗口位置」下拉     → `config.taskbar_position`（left/center/right）
 *    · 「任务栏内容缩放大小」下拉 → `config.taskbar_content_scale`（default/follow_system）
 *
 * ⛔ 后三者的落盘**不是可选的**：后端 `taskbar_widget::should_show()` 的判据是
 *    「`pinned_taskbar_devices` 非空」（用户口径 2026-09-24：默认关闭，选了设备才显示），
 *    窗口的挂载/拆除/重定位/重绘全部由 `config-changed` 驱动 ⇒ 这里不落盘就等于**控件是死的**。 */
function initTaskbarTab() {
  initTaskbarDevicePicker();
  initTaskbarPinCard();
  initTaskbarContentScale();
}

// 「在任务栏显示的设备」——交互范式对齐「强制静音」：点击弹出复选菜单。
//
// ⭐ **每次点击都重新拉取**设备并集，而不是在页面加载时缓存一份：
//   · 任务栏（另一个窗口）可能刚改过固定项 ⇒ 缓存会让勾选态过期；
//   · 设备热插拔（插拔耳机/接收器）后菜单要立刻反映，不能等页面刷新；
//   · 设备侧 WMI 取数 600ms+ 是**点击时**才付的成本，页面加载时不必付。
//
// **初始**勾选态一律取后端返回的 `pinned`（它用的是 `pinned_device_matches` 两级判据：
// key 精确命中 + fallback 兜底）。⛔ 不可在前端「只比 key」自行推断 —— 那与后端不等价，
// 会出现「界面没勾、后端其实已固定」⇒ 点一下变成又插一条（与 `67841b0` 同源缺陷）。
// ⚠️ 但 `createCheckableMenu` 要求调用方**在切换后自己维护**它传入的 `checked` Set
//    （见 `onToggle` 内注释），故该 Set 是「后端初始态 + 本次会话内的点击增量」。
async function initTaskbarDevicePicker() {
  const btn = document.getElementById("btn-taskbar-devices");
  if (!btn) return;
  btn.addEventListener("click", async (e) => {
    e.stopPropagation();
    let devices;
    try {
      devices = await invoke("get_selectable_devices");
    } catch (err) {
      window.showToast("读取设备列表失败：" + err);
      return;
    }

    // 后端已按 `pin.alias > resolve_device_name > 短名` 解析好 `name`，前端**原样显示**。
    // ⛔ 不要在此对 name 再做 `simplifyDeviceName`：后端返回的**已经是短名**
    //    （`pick_display_name` 内部过了一次 `core_name`），故再处理**恒等无害但无意义**；
    //    留在这里只会让人误以为「前端也参与名字归一」，将来某侧改了就对不上。
    const items = (devices || []).map((d) => ({ key: d.key, label: d.name }));
    const checked = new Set((devices || []).filter((d) => d.pinned).map((d) => d.key));
    // key -> 该设备的 fallback：取消/新增时都要原样回传，让后端判据与显示侧一致。
    const fallbackOf = new Map((devices || []).map((d) => [d.key, d.fallback]));

    createCheckableMenu({
      anchor: btn,
      items,
      checked,
      emptyText: "未发现可显示的设备",
      onToggle: async (key) => {
        // ⛔⛔ **必须自己维护 `checked` 这个 Set** —— 这是 `createCheckableMenu` 的既有契约：
        //    它只在**构造时**读一次 `checked`（`settings.js:163`），`onToggle` 之后重画时
        //    读的仍是**同一个 Set**（`:176`）。若调用方不改它，图标就**永不变化**
        //    （点第二下仍是勾，用户以为取消不了）。`settings-devices.js:315/318` 即此范式。
        const wasPinned = checked.has(key);
        try {
          // ⭐ `fallback` 由后端在 `get_selectable_devices` 里算好返回，这里**原样回传**。
          // ⛔ 不在前端用 simplifyDeviceName 现算：那与后端 core_name 不等价（JS 不剥协议
          //    后缀、取第一个括号），会让兜底键与显示侧判据对不上。
          // ⚠️ `alias` 刻意**不传**（用户 2026-09-24 决定）：别名只能由用户显式设置，
          //    勾选固定不该顺手写入一个「用户没设过的别名」。
          await invoke("toggle_pinned_taskbar_device", {
            key,
            fallback: fallbackOf.get(key) ?? null,
            alias: null,
          });
          // ⭐ 只在**后端确认成功**后翻转本地勾选态：后端可能因超上限拒绝，
          //    那时界面必须保持原样，否则「看到已勾、实际没固定」。
          if (wasPinned) checked.delete(key);
          else checked.add(key);
        } catch (err) {
          // 上限（PINNED_TASKBAR_LIMIT = 8）等拒绝原因由后端给出，原样展示给用户。
          // ⛔ **不 rethrow**：`createCheckableMenu` 的 onToggle 调用点**没有 try/catch**
          //    （`settings.js:174`），抛出会变成 unhandled rejection 污染控制台。
          //    勾选态**不翻转** —— 与后端状态保持一致（下次点开菜单会重新拉取刷新）。
          window.showToast(String(err));
        }
      },
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

// 「任务栏内容缩放大小」下拉 → `config.taskbar_content_scale`（default / follow_system）。
//
// ⛔ **作用域**（用户 2026-09-25 指定）：只改**内容**（图标边长 / 信息文字字号 /
//    随内容缩放的间距与项宽上限），**底衬恒按系统 DPI**（窗口高度、圆角不受本项影响）。
//    落地在 `taskbar_widget::Metrics`：内容走 `content_dpi`，底衬走 `dpi`，两者分开换算。
//
// ⭐ 为什么单列一张卡而不是塞进上面的折叠卡：本项与「固定位置」开关**无关**，
//    放进折叠卡会在开关关闭时被一起收起 —— 用户会以为这个设置消失了。
function initTaskbarContentScale() {
  // ⭐ 初始值取 config（覆盖 HTML 里写死的「默认大小」文案）；变更即落盘。
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
