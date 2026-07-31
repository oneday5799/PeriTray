let allDevices = [];
let hiddenDevices = [];
let hiddenGroups = [];
let deviceNames = {};
let deviceGroups = {};
let useSystemBt = false;
let trayDevices = [];

function throttle(fn, delay) {
  let lastCall = 0;
  let timer = null;
  return function(...args) {
    const now = Date.now();
    if (now - lastCall >= delay) {
      lastCall = now;
      fn.apply(this, args);
    } else {
      clearTimeout(timer);
      timer = setTimeout(() => {
        lastCall = Date.now();
        fn.apply(this, args);
      }, delay - (now - lastCall));
    }
  };
}

async function loadDevices() {
  const list = document.getElementById("device-list");
  list.innerHTML = '<div class="loading">加载中...</div>';

  const invoke = getInvoke();
  if (!invoke) {
    list.innerHTML = '<div class="loading">Tauri API 未加载</div>';
    return;
  }

  try {
    allDevices = await invoke("get_devices");
    const config = await invoke("get_config");
    hiddenDevices = config.hidden_devices || [];
    hiddenGroups = config.hidden_groups || [];
    deviceNames = config.device_names || {};
    deviceGroups = config.device_groups || {};
    useSystemBt = config.use_system_bt || false;
    trayDevices = config.tray_devices || [];
    renderDevices();
  } catch (e) {
    list.innerHTML = `<div class="loading">加载失败: ${e}</div>`;
  }
}

function getDeviceGroup(dev) {
  return deviceGroups[dev.name] || dev.dt;
}

function renderDevices() {
  const list = document.getElementById("device-list");
  list.innerHTML = "";

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

  let hasContent = false;
  for (const cat of CATEGORIES) {
    if (hiddenGroups.includes(cat.key)) continue;
    const devs = groups[cat.key];
    if (!devs || devs.length === 0) continue;
    hasContent = true;

    const section = document.createElement("div");
    section.className = "category";

    const header = document.createElement("div");
    header.className = "category-header";
    header.textContent = cat.label;
    section.appendChild(header);

    for (const dev of devs) {
      const card = document.createElement("div");
      card.className = "device-card";

      const infoEl = document.createElement("div");
      infoEl.className = "device-info";

      const nameEl = document.createElement("div");
      nameEl.className = "device-name";
      nameEl.textContent = getDisplayName(dev, deviceNames);
      infoEl.appendChild(nameEl);

      const statusRow = document.createElement("div");
      statusRow.className = "device-status-row";

      if (dev.is_bluetooth || dev.is_wireless_24g) {
        const statusEl = document.createElement("div");
        statusEl.className = "device-status";
        if (dev.status === "已连接") {
          statusEl.classList.add("connected");
        } else if (dev.status === "已配对") {
          statusEl.classList.add("paired");
        }
        statusEl.textContent = dev.status;
        statusRow.appendChild(statusEl);
      }

      if (dev.is_bluetooth) {
        const tagEl = document.createElement("div");
        tagEl.className = "tag-bluetooth";
        tagEl.textContent = "蓝牙";
        statusRow.appendChild(tagEl);
      } else if (dev.is_wireless_24g) {
        const tagEl = document.createElement("div");
        tagEl.className = "tag-24g";
        tagEl.textContent = "2.4G";
        statusRow.appendChild(tagEl);
      }

      if (dev.battery != null) {
        const batteryEl = document.createElement("div");
        batteryEl.className = "device-battery";
        batteryEl.textContent = `${dev.battery}%`;
        statusRow.appendChild(batteryEl);
      }

      infoEl.appendChild(statusRow);
      card.appendChild(infoEl);

      if (dev.is_bluetooth && (dev.status === "已配对" || dev.status === "已连接")) {
        const actionsEl = document.createElement("div");
        actionsEl.className = "device-actions";

        const connectBtn = document.createElement("button");
        connectBtn.className = "connect-btn";
        if (dev.status === "已连接") {
          connectBtn.textContent = "断开";
          connectBtn.dataset.action = "disconnect";
        } else {
          connectBtn.textContent = "连接";
          connectBtn.dataset.action = "connect";
        }
        connectBtn.addEventListener("click", async (e) => {
          e.stopPropagation();
          const invoke = getInvoke();
          if (!invoke) return;

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
          connectBtn.classList.add("loading");

          const statusEl = card.querySelector(".device-status");
          const batteryEl = card.querySelector(".device-battery");
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

          const refreshed = allDevices.find(d => d.name === dev.name);
          if (!statusChanged && refreshed) {
            newStatus = refreshed.status;
          }

          const newStatusEl = card.querySelector(".device-status");
          const newBatteryEl = card.querySelector(".device-battery");
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
          connectBtn.classList.remove("loading");
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
            const invoke = getInvoke();
            showToast(
              `${isConnect ? "连接失败" : "断开失败"}，点击这里跳转到系统设置进行修改`,
              invoke ? () => invoke("open_bt_settings") : null
            );
          }
        });
        actionsEl.appendChild(connectBtn);
        card.appendChild(actionsEl);
      }

      card.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        showContextMenu(e.clientX, e.clientY, dev);
      });

      section.appendChild(card);
    }

    list.appendChild(section);
  }

  if (!hasContent) {
    list.innerHTML = '<div class="loading">未检测到设备</div>';
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

  const groupItem = document.createElement("div");
  groupItem.className = "context-menu-item";
  groupItem.textContent = "更改分组";
  groupItem.addEventListener("click", () => {
    hideAllContextMenus();
    showGroupDialog(dev);
  });
  menu.appendChild(groupItem);

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

function showGroupDialog(dev) {
  const currentGroup = getDeviceGroup(dev);
  const isCustomGroup = deviceGroups[dev.name] !== undefined;

  const groupList = document.createElement("div");
  groupList.className = "group-list";

  for (const cat of CATEGORIES) {
    const item = document.createElement("div");
    item.className = "group-option" + (cat.key === currentGroup ? " selected" : "");
    item.textContent = cat.label;
    item.dataset.group = cat.key;

    item.addEventListener("click", () => {
      groupList.querySelectorAll(".group-option").forEach(el => el.classList.remove("selected"));
      item.classList.add("selected");
    });

    groupList.appendChild(item);
  }

  const buttons = [];

  if (isCustomGroup) {
    buttons.push({
      text: "恢复默认",
      className: "restore",
      onClick: async () => {
        const invoke = getInvoke();
        if (invoke) {
          await invoke("change_device_group", { name: dev.name, group: "" });
          const config = await invoke("get_config");
          deviceGroups = config.device_groups || {};
          renderDevices();
        }
        closeDialog(overlay);
      },
    });
  }

  buttons.push({
    text: "取消",
    className: "cancel",
    onClick: () => closeDialog(overlay),
  });

  buttons.push({
    text: "确定",
    className: "confirm",
    onClick: async () => {
      const selected = groupList.querySelector(".group-option.selected");
      if (selected) {
        const newGroup = selected.dataset.group;
        const invoke = getInvoke();
        if (invoke) {
          await invoke("change_device_group", {
            name: dev.name,
            group: newGroup === dev.dt ? "" : newGroup
          });
          const config = await invoke("get_config");
          deviceGroups = config.device_groups || {};
          renderDevices();
        }
      }
      closeDialog(overlay);
    },
  });

  const overlay = createDialog({
    title: "更改分组",
    content: [groupList],
    buttons,
  });
}

document.getElementById("btn-refresh").addEventListener("click", async () => {
  const activeTab = document.querySelector('.tab-title.active');
  if (activeTab) {
    const tabName = activeTab.dataset.tab;
    if (tabName === 'devices') {
      await loadDevices();
      showToast("已刷新");
    } else if (tabName === 'volume') {
      await loadAudioDevices();
      if (selectedDeviceId) {
        await loadAudioSessions(selectedDeviceId);
      }
      showToast("已刷新");
    }
  }
});

document.getElementById("btn-settings").addEventListener("click", async () => {
  const invoke = getInvoke();
  if (invoke) {
    try { await invoke("open_settings"); } catch (e) { console.error(e); }
  }
});

let lastFocusRefresh = 0;

window.addEventListener("focus", async () => {
  const now = Date.now();
  if (now - lastFocusRefresh < 2000) return;
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    const volumeTab = document.getElementById('tab-volume');
    const deviceTab = document.getElementById('tab-devices');
    const scrollTop = (volumeTab.style.display !== 'none' ? volumeTab : deviceTab).scrollTop;

    const cfg = await invoke("get_config");
    hiddenDevices = cfg.hidden_devices || [];
    hiddenGroups = cfg.hidden_groups || [];
    deviceNames = cfg.device_names || {};
    deviceGroups = cfg.device_groups || {};
    useSystemBt = cfg.use_system_bt || false;
    trayDevices = cfg.tray_devices || [];
    allDevices = await invoke("get_devices");
    renderDevices();
    if (volumeTab.style.display !== 'none') {
      await loadAudioDevices();
      if (selectedDeviceId) {
        await loadAudioSessions(selectedDeviceId);
      }
    }
    (volumeTab.style.display !== 'none' ? volumeTab : deviceTab).scrollTop = scrollTop;
    lastFocusRefresh = Date.now();
  } catch (e) {
    console.error("Failed to refresh on focus:", e);
  }
});

if (window.__TAURI__) {
  loadDevices();
} else {
  window.addEventListener("DOMContentLoaded", () => {
    setTimeout(loadDevices, 100);
  });
}

document.querySelectorAll('.tab-title').forEach(tab => {
  tab.addEventListener('click', async () => {
    document.querySelectorAll('.tab-title').forEach(t => t.classList.remove('active'));
    tab.classList.add('active');
    const tabName = tab.dataset.tab;
    document.getElementById('tab-devices').style.display = tabName === 'devices' ? 'block' : 'none';
    document.getElementById('tab-volume').style.display = tabName === 'volume' ? 'block' : 'none';
    if (tabName === 'volume') {
      await loadAudioDevices();
      if (selectedDeviceId) {
        await loadAudioSessions(selectedDeviceId);
      }
    }
  });
});

function switchToTab(tabName) {
  const target = document.querySelector(`.tab-title[data-tab="${tabName}"]`);
  if (target) target.click();
}

if (location.hash === '#volume') {
  switchToTab('volume');
}

if (window.__TAURI__ && window.__TAURI__.event) {
  window.__TAURI__.event.listen('switch-tab', (e) => {
    switchToTab(e.payload);
  });
}
