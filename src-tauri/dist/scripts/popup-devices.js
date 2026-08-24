/* popup-devices.js — 主窗口·设备信息 tab：设备列表对账式渲染/右键菜单/连接与托盘项
 * 加载序 3/4（common → popup-audio → 本文件 → popup.js）
 * 提供：loadDevices(fresh24g)（供 popup.js 的刷新按钮/focus 刷新调用，
 *       刷新按钮传 true 走 get_devices_fresh 强制现查 2.4G 电量）
 * 依赖：common.js（getInvoke/CATEGORIES/getDisplayName/showToast/registerContextMenu/
 *       clampMenuPosition/hideAllContextMenus/showRenameDialog/createSubmenuShell） /
 *       popup-audio.js（reconcileCards 对账式渲染骨架——两页刷新时统一按差集增删、
 *       已有卡原地更新，避免整页 innerHTML 重建导致的闪烁）
 */
let allDevices = [];
let hiddenDevices = [];
let hiddenGroups = [];
let deviceNames = {};
let deviceGroups = {};
let useSystemBt = false;
let trayDevices = [];

async function loadDevices(fresh24g = false) {
  const list = document.getElementById("device-list");
  // 已有内容时不清屏占位：强制现查可能耗时数秒，保持旧列表可见避免闪烁
  const hasCards = !!list.querySelector(".card.device");
  if (!hasCards) {
    list.innerHTML = '<div class="loading">加载中...</div>';
  }

  const invoke = getInvoke();
  if (!invoke) {
    if (!hasCards) list.innerHTML = '<div class="loading">Tauri API 未加载</div>';
    return;
  }

  try {
    // 手动刷新走 get_devices_fresh：强制现查 2.4G 电量（绕过缓存，鼠标休眠时较慢）
    allDevices = await invoke(fresh24g ? "get_devices_fresh" : "get_devices");
    const config = await invoke("get_config");
    hiddenDevices = config.hidden_devices || [];
    hiddenGroups = config.hidden_groups || [];
    deviceNames = config.device_names || {};
    deviceGroups = config.device_groups || {};
    useSystemBt = config.use_system_bt || false;
    trayDevices = config.tray_devices || [];
    renderDevices();
  } catch (e) {
    // 已有内容时保留旧列表，仅空态才显示错误占位
    if (!hasCards) list.innerHTML = `<div class="loading">加载失败: ${e}</div>`;
  }
}

function getDeviceGroup(dev) {
  return deviceGroups[dev.name] || dev.dt;
}

// ── 设备卡构建与原地更新 ────────────────────────────────

// 设备卡唯一键：同名设备的多形态（蓝牙/2.4G/USB）并存时可区分
function deviceKey(dev) {
  return `${dev.name}|${dev.is_bluetooth ? "bt" : ""}${dev.is_wireless_24g ? "24g" : ""}`;
}

// 填充状态标签行（创建与原地更新共用，保证动态部分单一来源）
function fillStatusRow(row, dev) {
  row.replaceChildren();

  if (dev.is_bluetooth || dev.is_wireless_24g) {
    const statusEl = document.createElement("div");
    statusEl.className = "card-tag status";
    if (dev.status === "已连接") {
      statusEl.classList.add("connected");
    } else if (dev.status === "已配对") {
      statusEl.classList.add("paired");
    }
    statusEl.textContent = dev.status;
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
        await invoke("connect_bluetooth_device", { name: dev.name });
      } else {
        await invoke("disconnect_bluetooth_device", { name: dev.name });
      }
    } catch (err) {
      console.error("BT action failed:", err);
    }

    const expectedConnected = isConnect;
    let newStatus = oldStatus;
    let statusChanged = false;
    const initialDelay = isConnect ? 800 : 100;
    await new Promise(r => setTimeout(r, initialDelay));
    const maxAttempts = 10;
    for (let i = 0; i < maxAttempts; i++) {
      try {
        const connected = await invoke("check_bt_connection", { name: dev.name });
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
      await new Promise(r => setTimeout(r, 400));
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
  fillStatusRow(card.querySelector(".card-tags"), dev);
  syncActionsEl(card);
}

// ── 设备列表渲染（分组 section 与组内卡片两级对账）──────

function renderDevices() {
  const list = document.getElementById("device-list");

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
  for (const g of layout) {
    const section = list.querySelector(`.category[data-group-id="${CSS.escape(g.id)}"]`);
    reconcileCards(section, ".card.device", "deviceId",
      g.devs.map(d => ({ ...d, id: deviceKey(d) })),
      createDeviceCard,
      updateDeviceCard
    );
    for (const d of g.devs) {
      const card = section.querySelector(`.card.device[data-device-id="${CSS.escape(deviceKey(d))}"]`);
      if (card) section.appendChild(card);
    }
  }

  // section 顺序按 CATEGORIES 声明序归位
  for (const g of layout) {
    const section = list.querySelector(`.category[data-group-id="${CSS.escape(g.id)}"]`);
    list.appendChild(section);
  }
}

let activeMenu = null;
registerContextMenu({ get menu() { return activeMenu; }, set menu(v) { activeMenu = v; } });

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
      displayName: getDisplayName(dev, deviceNames),
      nameSource: deviceNames[dev.name],
      onUpdate: (names) => { deviceNames = names; },
      onRender: renderDevices,
    });
  });
  menu.appendChild(renameItem);

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

  document.body.appendChild(menu);
  clampMenuPosition(menu, x, y);
  activeMenu = menu;
}
