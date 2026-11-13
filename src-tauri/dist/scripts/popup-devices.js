/* popup-devices.js — 主窗口·设备信息 tab：设备列表对账式渲染/右键菜单/连接与托盘项/
 *            A2DP 音频接收接入（可接收设备的连接/断开改走 Sink 会话 + 「音频接收」标签）
 * 加载序 3/4（common → popup-audio → 本文件 → popup.js）
 * 提供：loadDevices(fresh24g, opts)（供 popup.js 的刷新按钮/focus 刷新调用，
 *       刷新按钮传 true 走 get_devices_fresh 强制现查 2.4G 电量）；
 *       loadA2dpSnapshot()（a2dp-changed 订阅内部调用）；
 *       快照水合与 24g-battery-updated/devices-changed/a2dp-changed 推送订阅（静默重拉）
 * 依赖：common.js（getInvoke/onTauriEvent/CATEGORIES/getDisplayName/showToast/registerContextMenu/
 *       clampMenuPosition/hideAllContextMenus/showRenameDialog/createSubmenuShell/reconcileCards）
 */
let allDevices = [];
let hiddenDevices = [];
let hiddenGroups = [];
let deviceNames = {};
let deviceGroups = {};
let useSystemBt = false;
let trayDevices = [];
// A2DP 音频接收快照：设备列表（MAC 匹配用）+ 会话状态（标签/按钮文案用）
let a2dpDevices = [];
let a2dpStatus = null;
// ⭐ 右键菜单「钉到任务栏/移出任务栏」的状态源是 **`common.js` 的共用名单**
//   （`isTaskbarPinnedName` / `refreshTaskbarPinnedNames`）——音量页也要用同一份，
//   而 `popup-audio.js` 先于本文件加载，故那份状态**必须**在公共层。
// ⛔ 名单按**显示名**存、只用于**文案**；真正的钉/移一律由后端按身份键裁决。

// ── 配置派生量 → 本页运行态（单一来源）──────────────────
//
// ⭐ 返回「是否真的变了」：**只有变了才需要重渲染**。
//   `config-changed` 发得很频繁（改音量微调/静音锁/主题…都会发），
//   无条件 `renderDevices()` 是白做功，还会打断 hover 态。
function applyDeviceConfig(config) {
  const names = config.device_names || {};
  const groups = config.device_groups || {};
  const hidden = config.hidden_devices || [];
  const hiddenG = config.hidden_groups || [];
  const sysBt = !!config.use_system_bt;
  const tray = config.tray_devices || [];
  const changed =
    JSON.stringify(names) !== JSON.stringify(deviceNames) ||
    JSON.stringify(groups) !== JSON.stringify(deviceGroups) ||
    JSON.stringify(hidden) !== JSON.stringify(hiddenDevices) ||
    JSON.stringify(hiddenG) !== JSON.stringify(hiddenGroups) ||
    sysBt !== useSystemBt ||
    JSON.stringify(tray) !== JSON.stringify(trayDevices);
  if (!changed) return false;
  deviceNames = names;
  deviceGroups = groups;
  hiddenDevices = hidden;
  hiddenGroups = hiddenG;
  useSystemBt = sysBt;
  trayDevices = tray;
  return true;
}

// ── 本地快照水合（页面重载/重启后的秒显数据源）──────────

const SNAPSHOT_KEY = "pm_devices_snapshot";

function saveDevicesSnapshot() {
  try {
    if (!Array.isArray(allDevices) || allDevices.length === 0) return;
    localStorage.setItem(SNAPSHOT_KEY, JSON.stringify({ devices: allDevices }));
  } catch (e) { /* 存储不可用（隐私模式等）时静默 */ }
}

async function hydrateFromSnapshot() {
  const invoke = getInvoke();
  if (!invoke) return false;
  try {
    const raw = localStorage.getItem(SNAPSHOT_KEY);
    if (!raw) return false;
    const snap = JSON.parse(raw);
    if (!Array.isArray(snap.devices) || snap.devices.length === 0) return false;
    // 渲染依赖的配置派生量先行就绪（本地读取，毫秒级）
    const config = await invoke("get_config");
    applyDeviceConfig(config);
    allDevices = snap.devices;
    // ⭐ 快照水合路径同样要拉已钉名单（无真实请求时的快速首屏）
    window.refreshTaskbarPinnedNames();
    renderDevices();
    feLog(`快照水合完成: ${allDevices.length} 台`);
    return !!document.querySelector("#device-list .card.device");
  } catch (e) {
    console.error("Snapshot hydrate failed:", e);
    return false;
  }
}

let loadGen = 0;

// 前端行为埋点：经后端命令写入运行日志（标准级可见）
function feLog(msg) {
  const invoke = getInvoke();
  if (invoke) invoke("frontend_log", { tag: "popup", msg }).catch(() => {});
}

async function loadDevices(fresh24g = false, opts = {}) {
  const gen = ++loadGen;
  if (fresh24g) feLog("强制现查触发");
  const list = document.getElementById("device-list");
  // 已有内容时不清屏；无卡片时优先快照秒显，「加载中」仅剩全新环境首启场景
  const hasCards = !!list.querySelector(".card.device");
  let hydrated = false;
  if (!hasCards) {
    hydrated = await hydrateFromSnapshot();
    if (!hydrated && !opts.silent) {
      list.innerHTML = '<div class="loading">加载中...</div>';
    }
  }
  // 重启水合场景：明示数据正在刷新，完成前保持「正在刷新」态
  const notify = !!opts.notify && hydrated;
  if (notify) showToast("正在刷新...");

  const invoke = getInvoke();
  if (!invoke) {
    if (!hasCards && !hydrated) list.innerHTML = '<div class="loading">Tauri API 未加载</div>';
    return;
  }

  try {
    // 手动刷新走 get_devices_fresh：强制现查 2.4G 电量（绕过缓存，鼠标休眠时较慢）
    allDevices = await invoke(fresh24g ? "get_devices_fresh" : "get_devices");
    const config = await invoke("get_config");
    // A2DP 快照须先于渲染：设备卡的「音频接收」标签与按钮路由读它。
    // 设备列表只在手动刷新时重枚举（COM 成本；能力集仅配对增删时变化），
    // 但出现「不在缓存里的经典蓝牙设备」（刚配对新手机）时补一次枚举，
    // 否则新配对设备的按钮要等下次手动刷新才能路由到 Sink 会话。
    await loadA2dpSnapshot({
      refreshDevices:
        fresh24g ||
        allDevices.some(
          (d) =>
            d.is_bluetooth && !d.is_ble && d.device_id &&
            !a2dpDevices.some((a) => a.mac.toUpperCase() === a2dpMacForDeviceId(d.device_id))
        ),
    });
    applyDeviceConfig(config);
    renderDevices();
    // ⭐ 真实数据就绪后拉一次「已钉到任务栏」名单（右键菜单文案读它）。
    //    不 await：菜单在用户右键时才用，届时早已拿到；不阻塞首屏渲染。
    window.refreshTaskbarPinnedNames();
    saveDevicesSnapshot();
    // 代际校验：期间被更新的拉取取代则不再回写 toast 状态
    if (gen === loadGen && notify) showToast("已刷新");
  } catch (e) {
    console.error("Failed to refresh devices:", e);
    // 失败保留既有渲染（水合快照或上一轮数据）
    if (gen === loadGen) {
      if (notify) showToast("刷新失败", null, true);
      else if (!hasCards && !hydrated) {
        // 错误对象文本不拼进 innerHTML：以 textContent 写入，避免其被当标记解析
        list.innerHTML = "";
        const failEl = document.createElement("div");
        failEl.className = "loading";
        failEl.textContent = `加载失败: ${e}`;
        list.appendChild(failEl);
      }
    }
  }
}

function getDeviceGroup(dev) {
  return deviceGroups[dev.name] || dev.dt;
}

// ── A2DP 音频接收（可接收设备的接入层）────────────────────

// 拉取 A2DP 快照。设备列表缓存规则：⛔ 空结果不覆盖非空缓存——蓝牙栈就绪
// 时序会让单次枚举偶发为空，覆盖住就永远匹配不上了（与设置页同一判据）。
async function loadA2dpSnapshot({ refreshDevices = false } = {}) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    a2dpStatus = await invoke("a2dp_get_status");
    if (refreshDevices || a2dpDevices.length === 0) {
      const list = await invoke("a2dp_get_devices");
      if (list.length > 0) a2dpDevices = list;
    }
  } catch (e) {
    console.error("Failed to load a2dp snapshot:", e);
  }
}

// popup 的 device_id 是 AEP 形态（末段为冒号分隔 MAC）；与后端
// bluetooth::normalize_mac 同口径（取末个 '-' 段、去冒号、大写），勿单边改动。
function a2dpMacForDeviceId(deviceId) {
  if (!deviceId) return "";
  return deviceId.split("-").pop().replace(/:/g, "").toUpperCase();
}

// 该设备卡是否为 A2DP 可接收设备（按 MAC 对 a2dp_get_devices 匹配）
function a2dpDeviceFor(dev) {
  const mac = a2dpMacForDeviceId(dev.device_id);
  if (!mac) return null;
  return a2dpDevices.find((d) => d.mac.toUpperCase() === mac) || null;
}

// 返回该设备当前的 Sink 会话状态（无会话/目标不是本设备 → null）。
// ⭐ 目标匹配只认快照 device_mac：设备枚举缓存可能为空（枚举失败窗口），
// 经缓存找目标再比 MAC 会让「已连接」标签失明。
function a2dpSessionStateFor(dev) {
  if (!a2dpStatus || !a2dpStatus.device_mac) return null;
  if (!A2DP_ACTIVE_STATES.includes(a2dpStatus.state)) return null;
  return a2dpMacForDeviceId(dev.device_id) === a2dpStatus.device_mac.toUpperCase()
    ? a2dpStatus.state
    : null;
}

// 该设备的 Sink 会话是否占用中（决定按钮文案与「音频接收」标签）
function a2dpSessionActiveFor(dev) {
  return a2dpSessionStateFor(dev) !== null;
}

// ── 设备卡构建与原地更新 ────────────────────────────────

// 设备卡唯一键：同名设备的多形态（蓝牙/2.4G/USB/BLE）并存时可区分
function deviceKey(dev) {
  return `${dev.name}|${dev.is_bluetooth ? "bt" : ""}${dev.is_wireless_24g ? "24g" : ""}${dev.is_ble ? "ble" : ""}`;
}

// 填充状态标签行（创建与原地更新共用，保证动态部分单一来源）
function fillStatusRow(row, dev) {
  row.replaceChildren();

  if (dev.is_bluetooth || dev.is_wireless_24g) {
    const statusEl = document.createElement("div");
    statusEl.className = "card-tag status";
    // ⭐ 后端设备查询不感知 PC 侧 Sink 会话（只报蓝牙配置文件状态）：会话
    // link_ready（后端见转发会话流量）即音频链路已通，状态标签必须改判
    // 「已连接」。⛔ armed 只是武装等待——手机离场也停在这（后端 detail「等待手机播放」），
    // 不得改判「已连接」（离场误报）；connecting/backoff 同样保持后端原值。
    const session = a2dpSessionStateFor(dev);
    const effective = session === "link_ready" ? "已连接" : dev.status;
    if (effective === "已连接") {
      statusEl.classList.add("connected");
    } else if (effective === "已配对") {
      statusEl.classList.add("paired");
    }
    statusEl.textContent = effective;
    row.appendChild(statusEl);
  }

  if (dev.is_bluetooth) {
    const tagEl = document.createElement("div");
    tagEl.className = "card-tag bluetooth";
    tagEl.textContent = "蓝牙";
    row.appendChild(tagEl);
  } else if (dev.is_wireless_24g) {
    const tagEl = document.createElement("div");
    tagEl.className = "card-tag wireless";
    tagEl.textContent = "2.4G";
    row.appendChild(tagEl);
  }

  if (dev.battery != null) {
    const batteryEl = document.createElement("div");
    batteryEl.className = "card-tag battery";
    batteryEl.textContent = `${dev.battery}%`;
    row.appendChild(batteryEl);
  }

  // Sink 会话占用中的设备加「音频接收」标签（样式复用蓝牙标签的蓝色基调）
  if (a2dpSessionActiveFor(dev)) {
    const a2dpEl = document.createElement("div");
    a2dpEl.className = "card-tag bluetooth";
    a2dpEl.textContent = "音频接收";
    row.appendChild(a2dpEl);
  }
}

// 创建蓝牙连接/断开按钮区；运行时状态从 card._dev 动态读取，
// 避免闭包捕获的设备数据在刷新后过期
function buildActionsEl(card) {
  const actionsEl = document.createElement("div");
  actionsEl.className = "card-actions";

  const connectBtn = document.createElement("button");
  connectBtn.className = "connect-btn";
  connectBtn.addEventListener("click", async (e) => {
    e.stopPropagation();
    const invoke = getInvoke();
    if (!invoke) return;

    const dev = card._dev;
    const isConnect = connectBtn.dataset.action === "connect";

    if (useSystemBt) {
      try {
        await invoke("open_bt_settings");
      } catch (err) {
        console.error("Failed to open BT settings:", err);
      }
      return;
    }

    // A2DP 音频接收设备：连接/断开改走 Sink 会话。⛔ 不走 Win32
    // connect_bluetooth_device（对手机的服务开关无效——原按钮「一直无效」的原因），
    // 也不轮询 check_bt_connection（它查 ACL 链路，不反映 A2DP 会话）；
    // 状态由 a2dp-changed 事件驱动更新。
    // ⭐ 路由判据 = 枚举命中（a2dpDeviceFor）或会话占用中（a2dpSessionActiveFor）：
    //   枚举缓存为空的窗口里，占用中的设备若不按会话路由，按钮会回落成系统蓝牙「连接」。
    if (a2dpDeviceFor(dev) || a2dpSessionActiveFor(dev)) {
      connectBtn.disabled = true;
      connectBtn.classList.add("btn-loading");
      const statusEl = card.querySelector(".card-tag.status");
      if (statusEl && isConnect) {
        statusEl.textContent = "正在连接...";
        statusEl.classList.remove("connected", "paired");
      }
      try {
        if (isConnect) {
          await invoke("a2dp_connect_from_device", { deviceId: dev.device_id, deviceName: dev.name || "" });
        } else {
          await invoke("a2dp_disconnect");
        }
      } catch (err) {
        showToast(String(err));
      }
      connectBtn.disabled = false;
      connectBtn.classList.remove("btn-loading");
      await loadA2dpSnapshot();
      renderDevices();
      return;
    }

    connectBtn.disabled = true;
    connectBtn.classList.add("btn-loading");

    const statusEl = card.querySelector(".card-tag.status");
    const batteryEl = card.querySelector(".card-tag.battery");
    if (statusEl) {
      statusEl.textContent = isConnect ? "正在连接..." : "正在断开...";
      statusEl.classList.remove("connected", "paired");
    }
    if (batteryEl) batteryEl.style.display = "none";

    const oldStatus = dev.status;

    try {
      if (isConnect) {
        await invoke("connect_bluetooth_device", { deviceId: dev.device_id, isBle: dev.is_ble });
      } else {
        await invoke("disconnect_bluetooth_device", { deviceId: dev.device_id });
      }
    } catch (err) {
      console.error("BT action failed:", err);
    }

    const expectedConnected = isConnect;
    let newStatus = oldStatus;
    let statusChanged = false;
    const isBle = dev.is_ble;
    const initialDelay = isConnect ? (isBle ? 200 : 800) : 100;
    await new Promise(r => setTimeout(r, initialDelay));
    const maxAttempts = 10;
    const pollInterval = isBle ? 200 : 400;
    for (let i = 0; i < maxAttempts; i++) {
      try {
        const connected = await invoke("check_bt_connection", { deviceId: dev.device_id });
        if (connected !== null && connected !== undefined) {
          newStatus = connected ? "已连接" : "已配对";
          if (connected === expectedConnected) {
            statusChanged = true;
            break;
          }
        }
      } catch (err) {
        console.error("Check connection failed:", err);
        break;
      }
      await new Promise(r => setTimeout(r, pollInterval));
    }

    try {
      allDevices = await invoke("get_devices");
    } catch (err) {
      console.error("Refresh failed:", err);
    }

    const refreshed = allDevices.find(d => deviceKey(d) === deviceKey(dev));
    if (!statusChanged && refreshed) {
      newStatus = refreshed.status;
    }

    const newStatusEl = card.querySelector(".card-tag.status");
    const newBatteryEl = card.querySelector(".card-tag.battery");
    if (newStatusEl) {
      newStatusEl.textContent = newStatus;
      newStatusEl.classList.remove("connected", "paired");
      if (newStatus === "已连接") newStatusEl.classList.add("connected");
      else if (newStatus === "已配对") newStatusEl.classList.add("paired");
    }
    if (newBatteryEl && refreshed && refreshed.battery != null) {
      newBatteryEl.textContent = `${refreshed.battery}%`;
      newBatteryEl.style.display = "";
    }
    connectBtn.disabled = false;
    connectBtn.classList.remove("btn-loading");
    if (newStatus === "已连接") {
      connectBtn.textContent = "断开";
      connectBtn.dataset.action = "disconnect";
    } else if (newStatus === "已配对") {
      connectBtn.textContent = "连接";
      connectBtn.dataset.action = "connect";
    } else {
      connectBtn.style.display = "none";
    }

    if (!statusChanged) {
      showToast(
        `${isConnect ? "连接失败" : "断开失败"}，点击这里跳转到系统设置进行修改`,
        invoke ? () => invoke("open_bt_settings") : null
      );
    }
  });
  actionsEl.appendChild(connectBtn);
  return actionsEl;
}

// 按当前设备状态同步按钮区存在性与文案；操作进行中（btn-loading）不打断
function syncActionsEl(card) {
  const dev = card._dev;
  const showBtn = dev.is_bluetooth && (dev.status === "已配对" || dev.status === "已连接");
  let actionsEl = card.querySelector(".card-actions");

  if (!showBtn) {
    if (actionsEl) actionsEl.remove();
    return;
  }
  if (!actionsEl) {
    actionsEl = buildActionsEl(card);
    card.appendChild(actionsEl);
  }
  const connectBtn = actionsEl.querySelector(".connect-btn");
  if (connectBtn.classList.contains("btn-loading")) return;
  // A2DP 可接收设备的按钮文案跟 Sink 会话走（不跟 BT 链路状态走）；
  // 路由判据与 buildActionsEl 同款（枚举命中或会话占用中，防枚举空窗口回落）
  if (a2dpDeviceFor(dev) || a2dpSessionActiveFor(dev)) {
    const active = a2dpSessionActiveFor(dev);
    // 空闲 ∧ 蓝牙链路已通时说「连接」会被状态标签的「已连接」打脸——
    // 这个动作实际是拉起 Sink 会话接收音频（Win32 连接对手机无效，见点击路由注释），
    // 故改说「接收音频」；链路未通（已配对）保持「连接」，两种文案动作相同（connect）。
    connectBtn.textContent = active ? "断开" : (dev.status === "已连接" ? "接收音频" : "连接");
    connectBtn.dataset.action = active ? "disconnect" : "connect";
    connectBtn.style.display = "";
    return;
  }
  const isConnected = dev.status === "已连接";
  connectBtn.textContent = isConnected ? "断开" : "连接";
  connectBtn.dataset.action = isConnected ? "disconnect" : "connect";
  connectBtn.style.display = "";
}

function createDeviceCard(dev) {
  const card = document.createElement("div");
  card.className = "card device";
  card.dataset.deviceId = deviceKey(dev);
  card._dev = dev;

  const infoEl = document.createElement("div");
  infoEl.className = "card-left";

  const nameEl = document.createElement("div");
  nameEl.className = "card-title device-name";
  nameEl.textContent = getDisplayName(dev, deviceNames);
  infoEl.appendChild(nameEl);

  const statusRow = document.createElement("div");
  statusRow.className = "card-tags";
  fillStatusRow(statusRow, dev);
  infoEl.appendChild(statusRow);

  card.appendChild(infoEl);
  syncActionsEl(card);

  // 触发时动态读 _dev，右键菜单始终作用于最新数据
  card.oncontextmenu = (e) => {
    e.preventDefault();
    showContextMenu(e.clientX, e.clientY, card._dev);
  };
  return card;
}

// 已有卡原地更新：名称/状态标签/按钮区，不重建 DOM 节点
function updateDeviceCard(card, dev) {
  card._dev = dev;
  card.querySelector(".device-name").textContent = getDisplayName(dev, deviceNames);
  // 连接/断开操作进行中（btn-loading）时跳过状态标签更新，
  // 避免后端推送或焦点刷新将"正在连接.../正在断开..."覆盖为实际状态
  const connectBtn = card.querySelector(".connect-btn");
  if (!connectBtn || !connectBtn.classList.contains("btn-loading")) {
    fillStatusRow(card.querySelector(".card-tags"), dev);
  }
  syncActionsEl(card);
}

// ── 设备列表渲染（分组 section 与组内卡片两级对账）──────

function renderDevices() {
  const list = document.getElementById("device-list");

  // 查询瞬态失败（后端 Err 在前端表现为空数组）且已有卡片：保留旧渲染等待下一轮
  if (allDevices.length === 0 && list.querySelector(".card.device")) return;

  // 分组 → 组内已连接优先排序
  const groups = {};
  for (const d of allDevices) {
    if (hiddenDevices.includes(d.name)) continue;
    const group = getDeviceGroup(d);
    if (!groups[group]) groups[group] = [];
    groups[group].push(d);
  }
  for (const group of Object.keys(groups)) {
    groups[group].sort((a, b) => {
      const getSortKey = (dev) => {
        if (dev.is_bluetooth || dev.is_wireless_24g) {
          return dev.status === "已连接" ? 0 : 1;
        }
        return 2;
      };
      return getSortKey(a) - getSortKey(b);
    });
  }

  const layout = [];
  for (const cat of CATEGORIES) {
    if (hiddenGroups.includes(cat.key)) continue;
    const devs = groups[cat.key];
    if (!devs || devs.length === 0) continue;
    layout.push({ id: cat.key, label: cat.label, devs });
  }

  if (layout.length === 0) {
    list.innerHTML = '<div class="loading">未检测到设备</div>';
    return;
  }
  list.querySelectorAll(".loading").forEach(el => el.remove());

  // section 级对账：分组增减只增删对应节点
  reconcileCards(
    list,
    ".category",
    "groupId",
    layout,
    (g) => {
      const section = document.createElement("div");
      section.className = "category";
      section.dataset.groupId = g.id;
      const header = document.createElement("div");
      header.className = "section-title";
      header.textContent = g.label;
      section.appendChild(header);
      return section;
    },
    (section, g) => {
      section.querySelector(".section-title").textContent = g.label;
    }
  );

  // 组内卡片对账 + 按目标顺序归位（appendChild 移动既有节点，
  // 连接状态变化引发的排序调整不触发重建）
  // ⛔ 归位只在现有顺序与目标不一致时才移动节点：appendChild 即使落点不变也
  // 是「摘下再插回」，Chromium 会清空子树 :hover 且鼠标不动不重算——顺序没变
  // 时无条件移动 = 每次 render 打断一次 hover（连接后逐秒渲染即持续闪烁）
  for (const g of layout) {
    const section = list.querySelector(`.category[data-group-id="${CSS.escape(g.id)}"]`);
    reconcileCards(section, ".card.device", "deviceId",
      g.devs.map(d => ({ ...d, id: deviceKey(d) })),
      createDeviceCard,
      updateDeviceCard
    );
    const cards = [...section.querySelectorAll(".card.device")];
    const inOrder = cards.length === g.devs.length
      && g.devs.every((d, i) => cards[i].dataset.deviceId === deviceKey(d));
    if (!inOrder) {
      for (const d of g.devs) {
        const card = section.querySelector(`.card.device[data-device-id="${CSS.escape(deviceKey(d))}"]`);
        if (card) section.appendChild(card);
      }
    }
  }

  // section 顺序按 CATEGORIES 声明序归位（同上：顺序没变不移动，防打断 hover）
  const sections = [...list.querySelectorAll(".category")];
  const sectionsInOrder = sections.length === layout.length
    && layout.every((g, i) => sections[i].dataset.groupId === g.id);
  if (!sectionsInOrder) {
    for (const g of layout) {
      const section = list.querySelector(`.category[data-group-id="${CSS.escape(g.id)}"]`);
      list.appendChild(section);
    }
  }

  saveDevicesSnapshot();
}

let activeMenu = null;
registerContextMenu({ get menu() { return activeMenu; }, set menu(v) { activeMenu = v; } });

// ── 后端推送订阅：电量变更/设备增删 → 静默重拉（reconcile 原地更新）──

let silentRefreshTimer = null;

function scheduleSilentRefresh() {
  if (silentRefreshTimer) return;
  silentRefreshTimer = setTimeout(async () => {
    silentRefreshTimer = null;
    feLog("收到后端推送, 静默重拉");
    await loadDevices(false);
  }, 500);
}

// 后端推送（电量变更/设备增删）统一经 common.js 的 onTauriEvent()（P2-3）
onTauriEvent("24g-battery-updated", scheduleSilentRefresh);
onTauriEvent("bt-battery-updated", scheduleSilentRefresh);
onTauriEvent("devices-changed", scheduleSilentRefresh);
// A2DP 会话状态推送：更新命中设备卡的「音频接收」标签与连接/断开按钮
onTauriEvent("a2dp-changed", async () => {
  await loadA2dpSnapshot();
  renderDevices();
});

// ⛔ 本页原先**完全没有** `config-changed` 订阅 ⇒ `deviceNames` 只在
//   `loadDevices` / 快照水合时刷新。而「在音量控制页改名/恢复默认」走的是
//   `rename_device` 命令，它只**发事件**、不回调本页 ⇒ 本页标题停留在旧名，
//   要等下次整表重拉（重开弹窗/手动刷新/设备增删推送）才更新。
//   ⛔ 这正是「只有设备页 → 音量页能同步，反过来不行」的原因：音量页订阅了
//   `audio-devices-changed`，本页没有对应订阅。
// ⚠️ 兜底：无 payload 时**只取 config**（不整表重拉设备）——设备枚举要几百毫秒，
//   而这类事件要的仅仅是名字/分组/隐藏这几项派生量。
onTauriEvent("config-changed", async (event) => {
  let cfg = event && event.payload ? event.payload : null;
  if (!cfg) {
    const fn = getInvoke();
    if (!fn) return;
    cfg = await fn("get_config");
  }
  if (applyDeviceConfig(cfg)) renderDevices();
});

function showContextMenu(x, y, dev) {
  hideAllContextMenus();
  const invoke = getInvoke();
  if (!invoke) return;

  const menu = document.createElement("div");
  menu.className = "context-menu";

  const renameItem = document.createElement("div");
  renameItem.className = "context-menu-item";
  renameItem.textContent = "重命名";
  renameItem.addEventListener("click", () => {
    hideAllContextMenus();
    showRenameDialog({
      deviceName: dev.name,
      // ☽ “”恢复默认”按钮的可见性走两级查找：别名可能只落在长形态键上
      //    （从音量页改名的情形），否则按钮不出现、
      //    用户没法清掉出别名。
      displayName: getDisplayName(dev, deviceNames),
      nameSource: window.lookupDeviceAlias(deviceNames, dev.name),
      onUpdate: (names) => { deviceNames = names; },
      onRender: renderDevices,
    });
  });
  menu.appendChild(renameItem);

  // 「通过本机播放」（A2DP Sink）：仅经典蓝牙设备可见（BLE 无音频接收能力）。
  // 可否接收由后端按 MAC 匹配判定，这里不重复实现设备枚举；
  // 匹配不上时后端报错，由 toast 兜底说明。
  if (dev.is_bluetooth && !dev.is_ble && dev.device_id) {
    const a2dpItem = document.createElement("div");
    a2dpItem.className = "context-menu-item";
    a2dpItem.textContent = "通过本机播放";
    a2dpItem.addEventListener("click", async () => {
      hideAllContextMenus();
      try {
        await invoke("a2dp_connect_from_device", { deviceId: dev.device_id, deviceName: dev.name || "" });
        showToast("已请求通过本机播放，可在设置页「蓝牙音频接收」中查看状态");
      } catch (e) {
        showToast(String(e));
      }
    });
    menu.appendChild(a2dpItem);
  }

  const currentGroup = getDeviceGroup(dev);

  function applyGroup(newGroup) {
    hideAllContextMenus();
    const invoke = getInvoke();
    if (!invoke) return;
    invoke("change_device_group", { name: dev.name, group: newGroup === dev.dt ? "" : newGroup })
      .then(async () => {
        const config = await invoke("get_config");
        deviceGroups = config.device_groups || {};
        renderDevices();
      })
      .catch((e) => showToast(e));
  }

  // 定位策略：沿用本页原实现——锚定父菜单 offset，与 common 缺省的视口矩形策略不同，勿混用
  function positionGroupSubmenu(submenuEl, groupItemEl) {
    const sw = submenuEl.offsetWidth;
    const sh = submenuEl.offsetHeight;
    let left = menu.offsetLeft + menu.offsetWidth - 7;
    let top = menu.offsetTop + groupItemEl.offsetTop;
    if (left + sw > window.innerWidth) left = menu.offsetLeft - sw + 7;
    if (top + sh > window.innerHeight) top = Math.max(0, window.innerHeight - sh - 4);
    submenuEl.style.left = left + "px";
    submenuEl.style.top = top + "px";
  }

  const shell = createSubmenuShell(menu, "更改分组", positionGroupSubmenu);
  for (const cat of CATEGORIES) {
    shell.addItem(cat.label, cat.key === currentGroup, () => applyGroup(cat.key));
  }
  shell.finish();

  const hideItem = document.createElement("div");
  hideItem.className = "context-menu-item";
  hideItem.textContent = "隐藏";
  hideItem.addEventListener("click", async () => {
    await invoke("toggle_device_hidden", { name: dev.name });
    const config = await invoke("get_config");
    hiddenDevices = config.hidden_devices || [];
    renderDevices();
    hideAllContextMenus();
  });
  menu.appendChild(hideItem);

  const isTray = trayDevices.includes(dev.name);
  const trayItem = document.createElement("div");
  trayItem.className = "context-menu-item";
  trayItem.textContent = isTray ? "从托盘移除" : "添加到托盘";
  trayItem.addEventListener("click", async () => {
    try {
      await invoke("toggle_device_tray", { name: dev.name });
      if (trayDevices.includes(dev.name)) {
        trayDevices = trayDevices.filter(n => n !== dev.name);
      } else {
        trayDevices.push(dev.name);
      }
    } catch (e) {
      showToast(e);
    }
    hideAllContextMenus();
  });
  menu.appendChild(trayItem);
  menu.appendChild(trayItem);

  // ⭐ 「钉到任务栏」——与上面「添加到托盘」同一范式。
  //    语义是**切换**（已钉则显示「移出任务栏」）；后端 `toggle_taskbar_device_pin`
  //    返回切换**之后**的状态 ⇒ 直接用它更新本地 Set，不必再查一次。
  // ⛔ 后端按 `name` 自行解析 `key`/`fallback`（`core_name` 在 JS 侧无法等价复现），
  //    前端只管传名字与显示文案。
  const isPinned = window.isTaskbarPinnedName(dev.name);
  const pinItem = document.createElement("div");
  pinItem.className = "context-menu-item";
  pinItem.textContent = isPinned ? "移出任务栏" : "钉到任务栏";
  pinItem.addEventListener("click", async () => {
    try {
      await invoke("toggle_taskbar_device_pin", { name: dev.name });
      await window.refreshTaskbarPinnedNames();
    } catch (e) {
      showToast(e);
    }
    hideAllContextMenus();
  });
  // ⛔ **只在「显示设备信息组件」开启时才追加这一项**。
  //   组件关着时，钉过去也不会显示 ⇒ 入口没有意义。
  // ⚠️ 判据来自 `common.js` 的共用开关，与音量页**同一个函数**
  //   （`isTaskbarWidgetEnabled`）⇒ 两页不会分叉。
  // ⛔ fail-closed：开关取不到时初值为 false ⇒ 宁可不显示，不显示一个点不动的项。
  // 📌 即便组件关闭时仍有已钉设备，**取消入口也没丢**：设置页「任务栏组件」
  //   那张卡可以查看与移除（见 settings.html 的设备来源说明）。
  if (window.isTaskbarWidgetEnabled()) {
    menu.appendChild(pinItem);
  }

  document.body.appendChild(menu);
  clampMenuPosition(menu, x, y);
  activeMenu = menu;
}
