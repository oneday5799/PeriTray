/* settings-devices.js — 设置页·设备信息 tab：分组设备列表渲染/过滤正则卡
 * 加载序 4/7 · 提供：loadDevicesAsync() / renderGroups() / initDeviceFilterTab()
 * 依赖：common.js(invoke/CATEGORIES) /
 *       settings.js(config/createToggle/bindToggle/createExpandableCard/saveConfig) */

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
      await invoke("open_url", { url: "https://github.com/oneday5799/PeriphMonitor#24g-%E8%AE%BE%E5%A4%87%E6%94%AF%E6%8C%81" });
    } catch (e) {
      console.error("Failed to open URL:", e);
    }
  });
}
