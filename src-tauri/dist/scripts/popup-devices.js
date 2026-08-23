/* popup-devices.js — 主窗口·设备信息 tab：设备列表渲染/右键菜单/连接与托盘项
 * 加载序 3/4（common → popup-audio → 本文件 → popup.js）
 * 提供：loadDevices()（供 popup.js 的刷新按钮/focus 刷新调用）
 * 依赖：common.js（getInvoke/CATEGORIES/getDisplayName/showToast/registerContextMenu/
 *       clampMenuPosition/hideAllContextMenus/showRenameDialog/createSubmenuShell）
 */
let allDevices = [];
let hiddenDevices = [];
let hiddenGroups = [];
let deviceNames = {};
let deviceGroups = {};
let useSystemBt = false;
let trayDevices = [];

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
    header.className = "section-title";
    header.textContent = cat.label;
    section.appendChild(header);

    for (const dev of devs) {
      const card = document.createElement("div");
      card.className = "card device";

      const infoEl = document.createElement("div");
      infoEl.className = "card-left";

      const nameEl = document.createElement("div");
      nameEl.className = "card-title device-name";
      nameEl.textContent = getDisplayName(dev, deviceNames);
      infoEl.appendChild(nameEl);

      const statusRow = document.createElement("div");
      statusRow.className = "card-tags";

      if (dev.is_bluetooth || dev.is_wireless_24g) {
        const statusEl = document.createElement("div");
        statusEl.className = "card-tag status";
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
        tagEl.className = "card-tag bluetooth";
        tagEl.textContent = "蓝牙";
        statusRow.appendChild(tagEl);
      } else if (dev.is_wireless_24g) {
        const tagEl = document.createElement("div");
        tagEl.className = "card-tag wireless";
        tagEl.textContent = "2.4G";
        statusRow.appendChild(tagEl);
      }

      if (dev.battery != null) {
        const batteryEl = document.createElement("div");
        batteryEl.className = "card-tag battery";
        batteryEl.textContent = `${dev.battery}%`;
        statusRow.appendChild(batteryEl);
      }

      infoEl.appendChild(statusRow);
      card.appendChild(infoEl);

      if (dev.is_bluetooth && (dev.status === "已配对" || dev.status === "已连接")) {
        const actionsEl = document.createElement("div");
        actionsEl.className = "card-actions";

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

          const refreshed = allDevices.find(d => d.name === dev.name);
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
