const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let config = null;
let devices = [];
let expandedGroups = new Set();
let deviceGroups = {};

async function init() {
  // Tab switching
  document.querySelectorAll(".tab-item").forEach(tab => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".tab-item").forEach(t => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach(c => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById("tab-" + tab.dataset.tab).classList.add("active");
    });
  });

  try {
    config = await invoke("get_config");

    // Auto-start toggle
    const toggle = document.getElementById("toggle-autostart");
    toggle.checked = config.auto_start;
    toggle.addEventListener("change", async () => {
      config.auto_start = toggle.checked;
      await invoke("update_config", { newConfig: config });
    });

    // Filter toggle
    const filterToggle = document.getElementById("toggle-filter");
    const filterWrap = document.getElementById("filter-regex-wrap");
    filterToggle.checked = config.filter_enabled;
    filterWrap.style.display = config.filter_enabled ? "block" : "none";
    filterToggle.addEventListener("change", async () => {
      config.filter_enabled = filterToggle.checked;
      filterWrap.style.display = filterToggle.checked ? "block" : "none";
      await invoke("update_config", { newConfig: config });
      await loadDevicesAsync();
    });

    // Filter regex input
    const regexInput = document.getElementById("filter-regex");
    regexInput.value = config.filter_regex || "";
    let debounceTimer = null;
    regexInput.addEventListener("input", () => {
      clearTimeout(debounceTimer);
      debounceTimer = setTimeout(async () => {
        config.filter_regex = regexInput.value;
        await invoke("update_config", { newConfig: config });
        loadDevicesAsync();
      }, 500);
    });

    // Dedup toggle
    const dedupToggle = document.getElementById("toggle-dedup");
    dedupToggle.checked = config.dedup_devices;
    dedupToggle.addEventListener("change", async () => {
      config.dedup_devices = dedupToggle.checked;
      await invoke("update_config", { newConfig: config });
      await loadDevicesAsync();
    });

    // Show unnamed BT devices toggle
    const unnamedBtToggle = document.getElementById("toggle-unnamed-bt");
    unnamedBtToggle.checked = config.show_unnamed_bt;
    unnamedBtToggle.addEventListener("change", async () => {
      config.show_unnamed_bt = unnamedBtToggle.checked;
      await invoke("update_config", { newConfig: config });
      await loadDevicesAsync();
    });

    // Use system Bluetooth connection toggle
    const useSystemBtToggle = document.getElementById("toggle-use-system-bt");
    useSystemBtToggle.checked = config.use_system_bt;
    useSystemBtToggle.addEventListener("change", async () => {
      config.use_system_bt = useSystemBtToggle.checked;
      await invoke("update_config", { newConfig: config });
    });

    // Logging settings
    const logToggle = document.getElementById("toggle-log");
    logToggle.checked = config.log_enabled;
    logToggle.addEventListener("change", async () => {
      config.log_enabled = logToggle.checked;
      await invoke("update_config", { newConfig: config });
    });

    const logRetentionSelect = document.getElementById("log-retention");
    logRetentionSelect.value = config.log_retention || "one_day";
    logRetentionSelect.addEventListener("change", async () => {
      config.log_retention = logRetentionSelect.value;
      await invoke("update_config", { newConfig: config });
    });

    document.getElementById("btn-log-dir").addEventListener("click", async () => {
      try {
        await invoke("open_log_dir");
      } catch (e) {
        console.error("Failed to open log dir:", e);
      }
    });

    // ── 更新设置 ──────────────────────────────────────────
    const checkUpdatesToggle = document.getElementById("toggle-check-updates");
    checkUpdatesToggle.checked = config.check_updates !== false;
    checkUpdatesToggle.addEventListener("change", async () => {
      config.check_updates = checkUpdatesToggle.checked;
      await invoke("update_config", { newConfig: config });
    });

    const includePrereleaseToggle = document.getElementById("toggle-include-prerelease");
    includePrereleaseToggle.checked = config.include_prerelease || false;
    includePrereleaseToggle.addEventListener("change", async () => {
      config.include_prerelease = includePrereleaseToggle.checked;
      await invoke("update_config", { newConfig: config });
    });

    document.getElementById("btn-check-update").addEventListener("click", async () => {
      const btn = document.getElementById("btn-check-update");
      const originalText = btn.textContent;
      btn.textContent = "检测中...";
      btn.disabled = true;
      const timeoutId = setTimeout(() => {
        btn.textContent = originalText;
        btn.disabled = false;
      }, 30000);

      try {
        const info = await invoke("check_for_update", {
          includePrerelease: config.include_prerelease || false
        });
        clearTimeout(timeoutId);
        if (info.has_update) {
          showToast(
            `发现新版本 ${info.latest_version}（当前 ${info.current_version}）<br>点击前往下载`,
            () => invoke("open_url", { url: info.release_url })
          );
        } else {
          showToast("已是最新版本");
        }
      } catch (e) {
        clearTimeout(timeoutId);
        const err = String(e);
        if (err.includes("超时") || err.includes("timeout")) {
          showToast(
            "检测超时，请检查网络后重试<br>点击前往 Release 页面",
            () => invoke("open_url", { url: "https://github.com/oneday5799/PeriphMonitor/releases" })
          );
        } else if (err.includes("频繁") || err.includes("rate_limited")) {
          showToast("GitHub API 请求过于频繁，请稍后再试");
        } else {
          showToast("检测失败：" + err);
        }
      } finally {
        btn.textContent = originalText;
        btn.disabled = false;
      }
    });

    loadDevicesAsync();
    loadAudioDevicesAsync();
    initShutdownVolumeSettings();
    initShortcutSettings();

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
        await invoke("open_url", { url: "https://github.com/oneday5799/PeriphMonitor#%E6%B7%BB%E5%8A%A0%E8%87%AA%E5%AE%9A%E4%B9%89-24g-%E8%AE%BE%E5%A4%87" });
      } catch (e) {
        console.error("Failed to open URL:", e);
      }
    });
  } catch (e) {
    console.error("Failed to load settings:", e);
  }
}

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
    const groupEl = document.createElement("div");
    groupEl.className = "group";

    const card = document.createElement("div");
    card.className = "group-card";

    const header = document.createElement("div");
    header.className = "group-header";

    const icon = document.createElement("div");
    icon.className = "group-icon";
    icon.textContent = group.icon;
    header.appendChild(icon);

    const textWrap = document.createElement("div");
    textWrap.className = "group-text";

    const title = document.createElement("div");
    title.className = "group-title";
    title.textContent = group.label;
    textWrap.appendChild(title);

    const subtitle = document.createElement("div");
    subtitle.className = "group-subtitle";
    subtitle.textContent = group.subtitle;
    textWrap.appendChild(subtitle);

    header.appendChild(textWrap);

    // Group toggle switch
    const groupToggle = document.createElement("label");
    groupToggle.className = "toggle group-toggle";
    const isGroupHidden = config.hidden_groups.includes(group.key);
    const groupInput = document.createElement("input");
    groupInput.type = "checkbox";
    groupInput.checked = !isGroupHidden;

    groupToggle.addEventListener("click", (e) => {
      e.stopPropagation();
    });
    groupInput.addEventListener("change", async (e) => {
      e.stopPropagation();
      await invoke("toggle_group_hidden", { group: group.key });
      const cfg = await invoke("get_config");
      config.hidden_groups = cfg.hidden_groups || [];
      renderGroups();
    });

    const groupSlider = document.createElement("span");
    groupSlider.className = "slider";
    groupToggle.appendChild(groupInput);
    groupToggle.appendChild(groupSlider);
    header.appendChild(groupToggle);

    // Chevron arrow
    const arrow = document.createElement("div");
    arrow.className = "group-arrow";
    arrow.innerHTML = `<svg width="12" height="12" viewBox="0 0 12 12" fill="none"><path d="M4 2L8 6L4 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
    header.appendChild(arrow);

    const items = document.createElement("div");
    items.className = "group-items";

    header.addEventListener("click", () => {
      const isExpanded = items.classList.toggle("show");
      arrow.classList.toggle("expanded", isExpanded);
      if (isExpanded) {
        expandedGroups.add(group.key);
        items.style.maxHeight = items.scrollHeight + "px";
      } else {
        expandedGroups.delete(group.key);
        items.style.maxHeight = "0px";
      }
    });

    if (expandedGroups.has(group.key)) {
      items.classList.add("show");
      arrow.classList.add("expanded");
    }

    for (const dev of devs) {
      const item = document.createElement("div");
      item.className = "device-item";

      const nameEl = document.createElement("div");
      nameEl.className = "device-item-name";
      nameEl.textContent = dev.name;

      const isHidden = config.hidden_devices.includes(dev.name);
      if (isHidden) nameEl.classList.add("hidden");

      const toggle = document.createElement("label");
      toggle.className = "toggle";

      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = !isHidden;
      input.addEventListener("change", async () => {
        await invoke("toggle_device_hidden", { name: dev.name });
        config = await invoke("get_config");
        nameEl.classList.toggle("hidden", !input.checked);
      });

      const slider = document.createElement("span");
      slider.className = "slider";

      toggle.appendChild(input);
      toggle.appendChild(slider);

      item.appendChild(nameEl);
      item.appendChild(toggle);
      items.appendChild(item);
    }

    card.appendChild(header);
    card.appendChild(items);
    groupEl.appendChild(card);
    container.appendChild(groupEl);

    if (expandedGroups.has(group.key)) {
      requestAnimationFrame(() => {
        items.style.maxHeight = items.scrollHeight + "px";
      });
    }
  }
}

listen("config-changed", async () => {
  config = await invoke("get_config");
  loadDevicesAsync();
  loadAudioDevicesAsync();
});

async function loadAudioDevicesAsync() {
  try {
    config = await invoke("get_config");
    const audioDevices = await invoke("get_audio_devices");
    renderAudioDeviceGroups(audioDevices);
  } catch (e) {
    console.error("Failed to load audio devices:", e);
  }
}

function renderAudioDeviceGroups(audioDevices) {
  const container = document.getElementById("audio-device-groups");
  container.innerHTML = "";

  if (audioDevices.length === 0) {
    container.innerHTML = '<div class="device-item"><div class="device-item-name" style="color:#888">没有检测到音频设备</div></div>';
    return;
  }

  const groupEl = document.createElement("div");
  groupEl.className = "group";

  const card = document.createElement("div");
  card.className = "group-card";

  const items = document.createElement("div");
  items.className = "group-items show";
  items.style.maxHeight = "none";

  for (const dev of audioDevices) {
    const item = document.createElement("div");
    item.className = "device-item";

    const nameEl = document.createElement("div");
    nameEl.className = "device-item-name";
    nameEl.textContent = dev.name;
    if (dev.is_default) {
      const badge = document.createElement("span");
      badge.style.cssText = "font-size:12px;color:#0078d7;margin-left:6px";
      badge.textContent = "(默认)";
      nameEl.appendChild(badge);
    }

    const isHidden = (config.hidden_audio_devices || []).includes(dev.name);
    if (isHidden) nameEl.classList.add("hidden");

    const toggle = document.createElement("label");
    toggle.className = "toggle";

    const input = document.createElement("input");
    input.type = "checkbox";
    input.checked = !isHidden;
    input.addEventListener("change", async () => {
      await invoke("toggle_audio_device_hidden", { name: dev.name });
      config = await invoke("get_config");
      nameEl.classList.toggle("hidden", !input.checked);
    });

    const slider = document.createElement("span");
    slider.className = "slider";

    toggle.appendChild(input);
    toggle.appendChild(slider);

    item.appendChild(nameEl);
    item.appendChild(toggle);
    items.appendChild(item);
  }

  card.appendChild(items);
  groupEl.appendChild(card);
  container.appendChild(groupEl);
}

// ── 关机音量设置 ──────────────────────────────────────────

async function initShutdownVolumeSettings() {
  const toggle = document.getElementById("toggle-shutdown-volume");
  const settingsWrap = document.getElementById("shutdown-volume-settings");
  const deviceList = document.getElementById("shutdown-device-list");

  toggle.checked = config.shutdown_volume_enabled || false;
  settingsWrap.style.display = toggle.checked ? "block" : "none";

  toggle.addEventListener("change", async () => {
    config.shutdown_volume_enabled = toggle.checked;
    settingsWrap.style.display = toggle.checked ? "block" : "none";
    await invoke("update_config", { newConfig: config });
  });

  // Load audio devices for selection
  try {
    const audioDevices = await invoke("get_audio_devices");
    const savedDevices = config.shutdown_volume_devices || {};

    deviceList.innerHTML = "";

    for (const dev of audioDevices) {
      const savedVolume = savedDevices[dev.name];
      const isEnabled = savedVolume !== undefined;
      const volume = isEnabled ? Math.round(savedVolume * 100) : 50;

      const item = document.createElement("div");
      item.className = "shutdown-device-item";

      const nameEl = document.createElement("div");
      nameEl.className = "device-name" + (isEnabled ? "" : " inactive");
      nameEl.textContent = dev.name;

      const controls = document.createElement("div");
      controls.className = "shutdown-device-controls";

      const slider = document.createElement("input");
      slider.type = "range";
      slider.className = "volume-slider";
      slider.min = "0";
      slider.max = "100";
      slider.value = volume;
      updateSliderGradient(slider);

      const valueEl = document.createElement("span");
      valueEl.className = "volume-value" + (isEnabled ? "" : " inactive");
      valueEl.textContent = volume;

      // Click on name to toggle device on/off
      nameEl.style.cursor = "pointer";
      nameEl.addEventListener("click", async () => {
        if (config.shutdown_volume_devices[dev.name] !== undefined) {
          delete config.shutdown_volume_devices[dev.name];
          nameEl.classList.add("inactive");
          valueEl.classList.add("inactive");
        } else {
          config.shutdown_volume_devices[dev.name] = parseInt(slider.value) / 100;
          nameEl.classList.remove("inactive");
          valueEl.classList.remove("inactive");
        }
        await invoke("update_config", { newConfig: config });
      });

      slider.addEventListener("input", () => {
        valueEl.textContent = slider.value;
        updateSliderGradient(slider);
      });

      let debounceTimer = null;
      slider.addEventListener("change", () => {
        clearTimeout(debounceTimer);
        debounceTimer = setTimeout(async () => {
          if (config.shutdown_volume_devices[dev.name] !== undefined) {
            config.shutdown_volume_devices[dev.name] = parseInt(slider.value) / 100;
            await invoke("update_config", { newConfig: config });
          }
        }, 300);
      });

      controls.appendChild(slider);
      controls.appendChild(valueEl);
      item.appendChild(nameEl);
      item.appendChild(controls);
      deviceList.appendChild(item);
    }
  } catch (e) {
    console.error("Failed to load audio devices for shutdown volume:", e);
  }
}

function initShortcutSettings() {
  const actions = [
    { id: "devices", inputId: "shortcut-devices", clearId: "clear-shortcut-devices" },
    { id: "volume", inputId: "shortcut-volume", clearId: "clear-shortcut-volume" },
  ];

  for (const action of actions) {
    const input = document.getElementById(action.inputId);
    const clearBtn = document.getElementById(action.clearId);

    const keyField = action.id === "devices" ? "shortcut_devices" : "shortcut_volume";
    const savedKey = config[keyField];
    if (savedKey) {
      input.value = savedKey;
      clearBtn.style.display = "";
    } else {
      input.value = "";
      clearBtn.style.display = "none";
    }

    let recording = false;
    let keys = new Set();

    function resetRecording() {
      recording = false;
      keys.clear();
      input.classList.remove("recording");
      input.placeholder = "点击录制快捷键";
    }

    input.addEventListener("click", () => {
      if (recording) return;
      recording = true;
      keys.clear();
      input.value = "";
      input.classList.add("recording");
      input.placeholder = "请按下组合键...";
    });

    input.addEventListener("blur", () => {
      resetRecording();
    });

    input.addEventListener("keydown", (e) => {
      if (!recording) return;
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        resetRecording();
        input.value = savedKey || "";
        input.placeholder = "点击录制快捷键";
        clearBtn.style.display = savedKey ? "" : "none";
        return;
      }

      const keyName = e.key === " " ? "Space" : e.key;
      if (keyName === "Control" || keyName === "Shift" || keyName === "Alt" || keyName === "Meta") {
        keys.add(keyName);
        const preview = [...keys].join("+");
        input.value = preview;
        return;
      }

      if (e.ctrlKey) keys.add("Ctrl");
      if (e.shiftKey) keys.add("Shift");
      if (e.altKey) keys.add("Alt");
      if (e.metaKey) keys.add("Meta");

      if (keyName.length === 1 || keyName.startsWith("F") || [
        "Escape", "Tab", "CapsLock", "Space", "Backspace", "Delete",
        "Insert", "Home", "End", "PageUp", "PageDown",
        "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
        "PrintScreen", "ScrollLock", "Pause", "NumLock",
        "MediaPlayPause", "MediaStop", "MediaNextTrack", "MediaPrevTrack",
        "VolumeUp", "VolumeDown", "VolumeMute",
      ].includes(keyName)) {
        keys.add(keyName);
      }

      const shortcut = [...keys].join("+");
      if (shortcut) {
        recording = false;
        input.value = shortcut;
        input.classList.remove("recording");
        input.placeholder = "点击录制快捷键";

        registerShortcut(action.id, shortcut, input, clearBtn);
      }
    });

    clearBtn.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        await invoke("set_hotkey_config", { action: action.id, key: null });
        config[keyField] = null;
        input.value = "";
        clearBtn.style.display = "none";
        input.placeholder = "点击录制快捷键";
      } catch (_) {}
    });

    async function registerShortcut(actionId, shortcut, inputEl, clearEl) {
      try {
        await invoke("set_hotkey_config", {
          action: actionId,
          key: shortcut,
        });
        config[keyField] = shortcut;
        clearEl.style.display = "";
        await invoke("update_config", { newConfig: config });
      } catch (err) {
        const msg = String(err);
        if (msg.includes("已被占用")) {
          alert(`快捷键 "${shortcut}" 已被其他功能占用，请选择其他组合键。`);
        } else {
          alert(`注册快捷键失败: ${msg}`);
        }
        config[keyField] = null;
        inputEl.value = "";
        clearEl.style.display = "none";
      }
    }
  }
}

init();
