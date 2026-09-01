/* settings-devices.js — 设置页·设备信息 tab：分组设备列表渲染/过滤正则卡/低电量通知卡
 * 加载序 4/7 · 提供：loadDevicesAsync() / renderGroups() / initDeviceFilterTab() /
 *                    initLowBatteryNotifyTab() / renderLowBatteryContent()
 * 依赖：common.js(invoke/CATEGORIES/createCheckIcon/clampMenuPosition/hideAllContextMenus) /
 *       settings.js(config/bindToggle/createExpandableCard/saveConfig/showToast) */

let devices = [];
let expandedGroups = new Set();
let deviceGroups = {};

async function loadDevicesAsync() {
  try {
    config = await invoke("get_config");
    devices = await invoke("get_devices");
    deviceGroups = config.device_groups || {};
    renderGroups();
  } catch (e) {
    console.error("Failed to load devices:", e);
  }
}

function renderGroups() {
  const container = document.getElementById("device-groups");
  container.innerHTML = "";

  const groups = {};
  for (const d of devices) {
    const group = deviceGroups[d.name] || d.dt;
    if (!groups[group]) groups[group] = [];
    groups[group].push(d);
  }

  for (const group of CATEGORIES) {
    const devs = groups[group.key] || [];

    const card = document.createElement("div");
    card.className = "card expandable";

    const left = document.createElement("div");
    left.className = "card-left";

    const title = document.createElement("div");
    title.className = "card-title";
    title.textContent = group.label;
    left.appendChild(title);

    const subtitle = document.createElement("div");
    subtitle.className = "card-desc";
    subtitle.textContent = group.subtitle;
    left.appendChild(subtitle);

    card.appendChild(left);

    const actions = document.createElement("div");
    actions.className = "card-actions";

    const isGroupHidden = config.hidden_groups.includes(group.key);
    const { toggle: groupToggle } = createToggle(
      !isGroupHidden,
      async () => {
        await invoke("toggle_group_hidden", { group: group.key });
        const cfg = await invoke("get_config");
        config.hidden_groups = cfg.hidden_groups || [];
        renderGroups();
      },
      "card-toggle"
    );

    groupToggle.addEventListener("click", (e) => {
      e.stopPropagation();
    });
    actions.appendChild(groupToggle);

    const arrow = document.createElement("div");
    arrow.className = "card-arrow";
    arrow.innerHTML = `<svg width="12" height="12" viewBox="0 0 12 12" fill="none"><path d="M2 4L6 8L10 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
    actions.appendChild(arrow);

    card.appendChild(actions);

    const items = document.createElement("div");
    items.className = "card-items";

    // 展开态记忆经 onChanged 注入；高度用实测值（分组内容高度不受控，固定上限会截断）
    createExpandableCard(items, arrow).bindHeaderClick(card, {
      expandHeight: "content",
      onChanged: (expanded) => {
        if (expanded) expandedGroups.add(group.key);
        else expandedGroups.delete(group.key);
      },
    });

    if (expandedGroups.has(group.key)) {
      items.classList.add("show");
      arrow.classList.add("expanded");
    }

    for (const dev of devs) {
      const item = document.createElement("div");
      item.className = "card-item";

      const nameEl = document.createElement("div");
      nameEl.className = "card-item-name";
      nameEl.textContent = dev.name;

      const isHidden = config.hidden_devices.includes(dev.name);
      if (isHidden) nameEl.classList.add("hidden");

      const { toggle, input } = createToggle(!isHidden, async (input) => {
        await invoke("toggle_device_hidden", { name: dev.name });
        config = await invoke("get_config");
        nameEl.classList.toggle("hidden", !input.checked);
      });

      item.appendChild(nameEl);
      item.appendChild(toggle);
      items.appendChild(item);
    }

    card.appendChild(items);
    container.appendChild(card);

    if (expandedGroups.has(group.key)) {
      requestAnimationFrame(() => {
        items.style.maxHeight = items.scrollHeight + "px";
      });
    }
  }
}
function initDeviceFilterTab() {
  const filterWrap = document.getElementById("filter-regex-wrap");
  const filterArrow = document.getElementById("arrow-filter");
  const filterCard = document.getElementById("filter-card");

  bindToggle("toggle-wireless-only", {
    get: () => config.wireless_only,
    set: (v) => { config.wireless_only = v; },
    onChange: async () => { await loadDevicesAsync(); }
  });

  // Filter regex input
  const regexInput = document.getElementById("filter-regex");
  regexInput.value = config.filter_regex || "";

  function resizeRegexInput() {
    regexInput.style.height = "auto";
    regexInput.style.minHeight = "80px";
    regexInput.style.height = Math.max(regexInput.scrollHeight, 80) + "px";
  }
  resizeRegexInput();

  // 过滤卡展开机制（开关 onChange 与卡片点击双触发共用）
  const filterExpandable = createExpandableCard(filterWrap, filterArrow);

  function resetRegexSize() {
    regexInput.style.height = "auto";
    regexInput.style.minHeight = "80px";
  }

  function setFilterExpanded(expanded) {
    filterExpandable.set(expanded, "999px");
    resetRegexSize();
  }

  bindToggle("toggle-filter", {
    get: () => config.filter_enabled,
    set: (v) => { config.filter_enabled = v; },
    onChange: async (checked) => {
      setFilterExpanded(checked);
      await loadDevicesAsync();
    }
  });

  // 初始化恢复：展开分支免过渡，避免加载时播放收展动画
  filterExpandable.setInstant(config.filter_enabled, "999px");

  if (filterCard) {
    filterExpandable.bindHeaderClick(filterCard, {
      extraGuards: [".toggle"],
      onChanged: resetRegexSize,
    });
  }

  let debounceTimer = null;
  regexInput.addEventListener("input", () => {
    resizeRegexInput();
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(async () => {
      config.filter_regex = regexInput.value;
      await saveConfig();
      await loadDevicesAsync();
    }, 500);
  });

  bindToggle("toggle-dedup", {
    get: () => config.dedup_devices,
    set: (v) => { config.dedup_devices = v; },
    onChange: async () => { await loadDevicesAsync(); }
  });

  bindToggle("toggle-unnamed-bt", {
    get: () => config.show_unnamed_bt,
    set: (v) => { config.show_unnamed_bt = v; },
    onChange: async () => { await loadDevicesAsync(); }
  });

  bindToggle("toggle-use-system-bt", {
    get: () => config.use_system_bt,
    set: (v) => { config.use_system_bt = v; }
  });

  // Open 2.4G device list button
  document.getElementById("btn-add-24g").addEventListener("click", async () => {
    try {
      await invoke("open_24g_device_file");
    } catch (e) {
      console.error("Failed to open file:", e);
    }
  });

  // Help link for 2.4G device
  document.getElementById("help-24g").addEventListener("click", async () => {
    try {
      await invoke("open_url", { url: "https://github.com/oneday5799/PeriTray#24g-%E8%AE%BE%E5%A4%87%E6%94%AF%E6%8C%81" });
    } catch (e) {
      console.error("Failed to open URL:", e);
    }
  });
}

// ── 低电量通知折叠卡 ──

function initLowBatteryNotifyTab() {
  const items = document.getElementById("low-battery-items");
  const arrow = document.getElementById("arrow-low-battery");
  const card = document.getElementById("low-battery-card");
  if (!items || !arrow || !card) return;

  const expandable = createExpandableCard(items, arrow);

  function setExpanded(expanded) {
    expandable.set(expanded, "content");
  }

  bindToggle("toggle-low-battery", {
    get: () => config.low_battery_notify || false,
    set: (v) => { config.low_battery_notify = v; },
    onChange: (checked) => { setExpanded(checked); }
  });

  expandable.setInstant(config.low_battery_notify || false, "999px");

  expandable.bindHeaderClick(card, {
    extraGuards: [".toggle", "button", "input"],
  });

  renderLowBatteryContent();
}

async function renderLowBatteryContent() {
  const items = document.getElementById("low-battery-items");
  if (!items) return;
  items.innerHTML = "";

  // ── 通知设备 ──
  const deviceRow = document.createElement("div");
  deviceRow.className = "card-item";
  const deviceLabel = document.createElement("div");
  deviceLabel.className = "card-item-name";
  deviceLabel.textContent = "通知设备";
  const deviceActions = document.createElement("div");
  deviceActions.className = "card-item-controls";
  const selectBtn = document.createElement("button");
  selectBtn.className = "add-device-btn";
  selectBtn.textContent = "选择设备";
  deviceActions.appendChild(selectBtn);
  deviceRow.appendChild(deviceLabel);
  deviceRow.appendChild(deviceActions);
  items.appendChild(deviceRow);

  selectBtn.addEventListener("click", async (e) => {
    e.stopPropagation();
    let allDevices = [];
    try {
      allDevices = await invoke("get_cached_devices");
      if (!allDevices.length) allDevices = await invoke("get_devices");
    } catch (err) {
      console.error("Failed to load devices for low battery notify:", err);
      return;
    }
    const wireless = allDevices.filter(d => d.is_wireless_24g || d.is_bluetooth);
    const selected = new Set(config.low_battery_devices || []);
    const deviceNames = config.device_names || {};

    createCheckableMenu({
      anchor: selectBtn,
      emptyText: "没有无线设备",
      items: wireless.map(dev => ({
        key: dev.name,
        label: fmtDevName(deviceNames[dev.name] || dev.name),
      })),
      checked: selected,
      onToggle: async (name) => {
        const list = config.low_battery_devices || [];
        const idx = list.indexOf(name);
        if (idx >= 0) {
          list.splice(idx, 1);
          selected.delete(name);
        } else {
          list.push(name);
          selected.add(name);
        }
        config.low_battery_devices = list;
        await saveConfig();
      },
    });
  });

  // ── 电量阈值 ──
  const thresholdRow = document.createElement("div");
  thresholdRow.className = "card-item";
  const thresholdLabel = document.createElement("div");
  thresholdLabel.className = "card-item-name";
  thresholdLabel.textContent = "电量阈值";
  const thresholdActions = document.createElement("div");
  thresholdActions.className = "card-item-controls";
  const thresholdInput = document.createElement("input");
  thresholdInput.type = "text";
  thresholdInput.className = "dialog-input";
  thresholdInput.style.width = "120px";
  thresholdInput.value = (config.low_battery_thresholds || [15, 10, 5]).join(",");
  thresholdInput.placeholder = "15,10,5";
  thresholdActions.appendChild(thresholdInput);
  thresholdRow.appendChild(thresholdLabel);
  thresholdRow.appendChild(thresholdActions);
  items.appendChild(thresholdRow);

  thresholdInput.addEventListener("blur", () => {
    const raw = thresholdInput.value.trim().replace(/，/g, ",");
    const parts = raw.split(",").map(s => s.trim()).filter(s => s !== "");

    if (parts.length === 0) {
      showToast("请输入至少一个阈值", null, true);
      resetThreshold();
      return;
    }
    if (parts.length > 5) {
      showToast("最多5个阈值", null, true);
      resetThreshold();
      return;
    }

    const nums = parts.map(Number);

    const nanIdx = nums.findIndex(n => isNaN(n));
    if (nanIdx >= 0) {
      showToast(`含有非数字："${parts[nanIdx]}"`, null, true);
      resetThreshold();
      return;
    }
    const floatIdx = nums.findIndex(n => !Number.isInteger(n));
    if (floatIdx >= 0) {
      showToast(`须为整数：${nums[floatIdx]}`, null, true);
      resetThreshold();
      return;
    }
    const outIdx = nums.findIndex(n => n < 0 || n > 100);
    if (outIdx >= 0) {
      showToast(`超出范围(0-100)：${nums[outIdx]}`, null, true);
      resetThreshold();
      return;
    }
    if (new Set(nums).size !== nums.length) {
      const dup = nums.find((n, i) => nums.indexOf(n) !== i);
      showToast(`有重复值：${dup}`, null, true);
      resetThreshold();
      return;
    }

    config.low_battery_thresholds = nums;
    saveConfig();
  });

  function resetThreshold() {
    thresholdInput.value = [15, 10, 5].join(",");
    config.low_battery_thresholds = [15, 10, 5];
    saveConfig();
  }

  // ── 刷新间隔 ──
  const refreshRow = document.createElement("div");
  refreshRow.className = "card-item";
  const refreshLabel = document.createElement("div");
  refreshLabel.className = "card-item-name";
  refreshLabel.textContent = "刷新间隔";
  const refreshActions = document.createElement("div");
  refreshActions.className = "card-item-controls";
  const refreshWrap = document.createElement("div");
  refreshWrap.style.position = "relative";
  refreshWrap.style.display = "inline-flex";
  refreshWrap.style.alignItems = "center";
  const refreshInput = document.createElement("input");
  refreshInput.type = "text";
  refreshInput.className = "dialog-input";
  refreshInput.style.width = "120px";
  refreshInput.style.paddingRight = "32px";
  refreshInput.value = String(config.low_battery_refresh_secs || 10);
  const suffix = document.createElement("span");
  suffix.textContent = "秒";
  suffix.style.position = "absolute";
  suffix.style.right = "8px";
  suffix.style.fontSize = "13px";
  suffix.style.color = "var(--text-tertiary)";
  suffix.style.pointerEvents = "none";
  suffix.style.userSelect = "none";
  refreshWrap.appendChild(refreshInput);
  refreshWrap.appendChild(suffix);
  refreshActions.appendChild(refreshWrap);
  refreshRow.appendChild(refreshLabel);
  refreshRow.appendChild(refreshActions);
  items.appendChild(refreshRow);

  refreshInput.addEventListener("blur", () => {
    const raw = refreshInput.value.trim();
    if (raw === "") {
      showToast("请输入刷新间隔", null, true);
      resetRefresh();
      return;
    }
    const n = Number(raw);
    if (isNaN(n)) {
      showToast("须为数字", null, true);
      resetRefresh();
      return;
    }
    if (!Number.isInteger(n)) {
      showToast("须为整数", null, true);
      resetRefresh();
      return;
    }
    if (n < 10 || n > 3600) {
      showToast("须为10-3600的整数", null, true);
      resetRefresh();
      return;
    }
    config.low_battery_refresh_secs = n;
    saveConfig();
  });

  function resetRefresh() {
    refreshInput.value = "10";
    config.low_battery_refresh_secs = 10;
    saveConfig();
  }
}
