/* settings-taskbar.js — 设置页·任务栏 tab：任务栏信息窗口的显示设备选择与窗口位置固定
 * 加载序 6/8 · 提供：initTaskbarTab()
 * 依赖：common.js / settings.js(createExpandableCard/initComboBox/createCheckableMenu)
 *
 * ⚠️ 设备选择（`initTaskbarDevicePicker`）已**接线真实数据**（T3-2）：
 *    数据源 = `get_selectable_devices`（设备页 ∪ 输出端点 ∪ 输入端点的**并集**），
 *    勾选 = `toggle_pinned_taskbar_device`。两者都是后端命令，本文件**不读 config**。
 *    「固定任务栏窗口位置」下拉仍是「仅 UI」阶段（`initComboBox(..., null)` 不落盘）。 */
function initTaskbarTab() {
  initTaskbarDevicePicker();
  initTaskbarPinCard();
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
