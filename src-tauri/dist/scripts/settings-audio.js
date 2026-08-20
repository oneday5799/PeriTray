async function loadAudioDevicesAsync() {
  try {
    config = await invoke("get_config");
    const audioDevices = await invoke("get_audio_devices");
    renderAudioDeviceGroups(audioDevices);
  } catch (e) {
    console.error("Failed to load audio devices:", e);
  }
}

let audioExpanded = true;

function renderAudioDeviceGroups(audioDevices) {
  const container = document.getElementById("audio-device-items");
  const arrow = document.getElementById("arrow-audio");
  container.innerHTML = "";

  if (audioDevices.length === 0) {
    container.innerHTML = '<div class="card-item"><div class="card-item-name" style="color:#888">没有检测到音频设备</div></div>';
    if (arrow) arrow.classList.remove("expanded");
    container.style.maxHeight = "0px";
    return;
  }

  for (const dev of audioDevices) {
    const item = document.createElement("div");
    item.className = "card-item";

    const nameEl = document.createElement("div");
    nameEl.className = "card-item-name";
    nameEl.textContent = dev.name;
    if (dev.is_default) {
      const badge = document.createElement("span");
      badge.style.cssText = "font-size:12px;color:#0078d7;margin-left:6px";
      badge.textContent = "(默认)";
      nameEl.appendChild(badge);
    }

    const isHidden = (config.hidden_audio_devices || []).includes(dev.name);
    if (isHidden) nameEl.classList.add("hidden");

    const { toggle, input } = createToggle(!isHidden, async (input) => {
      await invoke("toggle_audio_device_hidden", { name: dev.name });
      config = await invoke("get_config");
      nameEl.classList.toggle("hidden", !input.checked);
    });

    item.appendChild(nameEl);
    item.appendChild(toggle);
    container.appendChild(item);
  }

  if (audioExpanded) {
    container.classList.add("show");
    container.style.transition = "none";
    container.style.maxHeight = "999px";
    if (arrow) arrow.classList.add("expanded");
    requestAnimationFrame(() => {
      container.style.transition = "";
    });
  } else {
    container.classList.remove("show");
    container.style.maxHeight = "0px";
    if (arrow) arrow.classList.remove("expanded");
  }
}

function initAudioCardToggle() {
  const card = document.getElementById("audio-card");
  const items = document.getElementById("audio-device-items");
  const arrow = document.getElementById("arrow-audio");

  if (!card || !items || !arrow) return;

  card.addEventListener("click", (e) => {
    if (e.target.closest('.card-items')) return;
    const isExpanded = items.classList.toggle("show");
    arrow.classList.toggle("expanded", isExpanded);
    audioExpanded = isExpanded;
    if (isExpanded) {
      items.style.maxHeight = items.scrollHeight + "px";
    } else {
      items.style.maxHeight = "0px";
    }
  });

  arrow.classList.toggle("expanded", audioExpanded);
}

function initMuteLockSettings() {
  bindToggle("toggle-mute-lock", {
    get: () => config.mute_lock || false,
    set: (v) => { config.mute_lock = v; },
  });
}

function initFineAdjustSettings() {
  bindToggle("toggle-fine-adjust", {
    get: () => config.volume_fine_adjust || false,
    set: (v) => { config.volume_fine_adjust = v; },
  });
}

function initForceMuteSettings() {
  const btn = document.getElementById("btn-force-mute");
  if (!btn) return;

  btn.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (activeSettingsMenu) {
      hideAllContextMenus();
      return;
    }
    let audioDevices = [];
    try {
      audioDevices = await invoke("get_audio_devices");
    } catch (err) {
      console.error("Failed to load audio devices for force mute:", err);
      return;
    }
    const hidden = config.hidden_audio_devices || [];
    const selected = new Set(config.force_mute_devices || []);
    const deviceNames = config.device_names || {};

    const menu = document.createElement("div");
    menu.className = "context-menu";
    menu.style.maxHeight = "360px";
    menu.style.overflowY = "auto";

    for (const dev of audioDevices) {
      if (hidden.includes(dev.name)) continue;
      const item = document.createElement("div");
      item.className = "context-menu-item";
      item.style.display = "flex";
      item.style.alignItems = "center";

      const leading = document.createElement("span");
      leading.className = "context-menu-leading";
      const isChecked = selected.has(dev.name);
      if (isChecked) {
        const check = document.createElementNS("http://www.w3.org/2000/svg", "svg");
        check.setAttribute("class", "context-menu-check");
        check.setAttribute("width", "12");
        check.setAttribute("height", "12");
        check.setAttribute("viewBox", "0 0 12 12");
        check.setAttribute("fill", "none");
        check.innerHTML = '<path d="M2 6L5 9L10 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
        leading.appendChild(check);
      }
      item.appendChild(leading);

      const label = document.createElement("span");
      label.textContent = deviceNames[dev.name] || dev.name;
      item.appendChild(label);

      item.addEventListener("click", async (ev) => {
        ev.stopPropagation();
        const list = config.force_mute_devices || [];
        const idx = list.indexOf(dev.name);
        if (idx >= 0) {
          list.splice(idx, 1);
        } else {
          list.push(dev.name);
        }
        config.force_mute_devices = list;
        await saveConfig();
        leading.innerHTML = "";
        if (list.indexOf(dev.name) >= 0) {
          const check = document.createElementNS("http://www.w3.org/2000/svg", "svg");
          check.setAttribute("class", "context-menu-check");
          check.setAttribute("width", "12");
          check.setAttribute("height", "12");
          check.setAttribute("viewBox", "0 0 12 12");
          check.setAttribute("fill", "none");
          check.innerHTML = '<path d="M2 6L5 9L10 3" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>';
          leading.appendChild(check);
        }
      });

      menu.appendChild(item);
    }

    if (menu.childElementCount === 0) {
      const empty = document.createElement("div");
      empty.className = "context-menu-item";
      empty.textContent = "没有可用的音频设备";
      menu.appendChild(empty);
    }

    document.body.appendChild(menu);
    const rect = btn.getBoundingClientRect();
    clampMenuPosition(menu, rect.left, rect.bottom + 4);
    activeSettingsMenu = menu;
  });
}

async function initShutdownVolumeSettings() {
  const toggle = document.getElementById("toggle-shutdown-volume");
  const items = document.getElementById("shutdown-device-items");
  const arrow = document.getElementById("arrow-shutdown");
  const card = document.getElementById("shutdown-card");

  function setShutdownExpanded(expanded) {
    if (expanded) {
      items.classList.add("show");
      items.style.maxHeight = items.scrollHeight + "px";
    } else {
      items.style.maxHeight = "0px";
    }
    if (arrow) arrow.classList.toggle("expanded", expanded);
  }

  bindToggle("toggle-shutdown-volume", {
    get: () => config.shutdown_volume_enabled || false,
    set: (v) => { config.shutdown_volume_enabled = v; },
    onChange: async (checked) => { setShutdownExpanded(checked); }
  });

  if (toggle.checked) {
    items.classList.add("show");
    items.style.transition = "none";
    items.style.maxHeight = "999px";
    if (arrow) arrow.classList.add("expanded");
    requestAnimationFrame(() => {
      items.style.transition = "";
    });
  } else {
    items.style.maxHeight = "0px";
  }

  card.addEventListener("click", (e) => {
    if (e.target.closest('.card-items')) return;
    if (e.target.closest(".toggle") || e.target.closest("input")) return;
    const isExpanded = items.classList.toggle("show");
    if (isExpanded) {
      items.style.maxHeight = items.scrollHeight + "px";
    } else {
      items.style.maxHeight = "0px";
    }
    if (arrow) arrow.classList.toggle("expanded", isExpanded);
  });

  try {
    const audioDevices = await invoke("get_audio_devices");
    const savedDevices = config.shutdown_volume_devices || {};

    items.innerHTML = "";

    for (const dev of audioDevices) {
      const savedVolume = savedDevices[dev.name];
      const isEnabled = savedVolume !== undefined;
      const volume = isEnabled ? Math.round(savedVolume * 100) : 50;

      const item = document.createElement("div");
      item.className = "card-item";

      const nameEl = document.createElement("div");
      nameEl.className = "card-item-name" + (isEnabled ? "" : " hidden");
      nameEl.textContent = dev.name;

      const controls = document.createElement("div");
      controls.className = "card-item-controls";

      const numberbox = document.createElement("div");
      numberbox.className = "win-numberbox";

      const numberboxBorder = document.createElement("div");
      numberboxBorder.className = "win-numberbox-border";

      const focusBorder = document.createElement("div");
      focusBorder.className = "win-numberbox-focus-border";

      const input = document.createElement("input");
      input.type = "text";
      input.className = "win-numberbox-input";
      input.value = volume;
      input.inputMode = "numeric";

      const spin = document.createElement("div");
      spin.className = "win-numberbox-spin";

      const btnUp = document.createElement("button");
      btnUp.type = "button";
      btnUp.className = "win-numberbox-spin-btn";
      btnUp.innerHTML = '<svg viewBox="0 0 12 12" fill="none"><path d="M2 8L6 4L10 8" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';

      const btnDown = document.createElement("button");
      btnDown.type = "button";
      btnDown.className = "win-numberbox-spin-btn";
      btnDown.innerHTML = '<svg viewBox="0 0 12 12" fill="none"><path d="M2 4L6 8L10 4" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>';

      function clamp(v) { return Math.max(0, Math.min(100, v)); }
      function setNumberBoxValue(v) {
        const val = clamp(Math.round(v));
        input.value = val;
        btnUp.disabled = val >= 100;
        btnDown.disabled = val <= 0;
        return val;
      }

      function updateConfig(val) {
        if (isEnabled) {
          config.shutdown_volume_devices[dev.name] = val / 100;
          saveConfig();
        }
      }

      setNumberBoxValue(volume);
      if (!isEnabled) {
        input.disabled = true;
        btnUp.disabled = true;
        btnDown.disabled = true;
      }

      btnUp.addEventListener("click", () => {
        const val = setNumberBoxValue(parseInt(input.value) + 5);
        updateConfig(val);
      });

      btnDown.addEventListener("click", () => {
        const val = setNumberBoxValue(parseInt(input.value) - 5);
        updateConfig(val);
      });

      let debounceTimer = null;
      input.addEventListener("input", () => {
        const raw = parseInt(input.value);
        if (!isNaN(raw)) {
          clearTimeout(debounceTimer);
          debounceTimer = setTimeout(() => {
            const val = setNumberBoxValue(raw);
            updateConfig(val);
          }, 300);
        }
      });

      input.addEventListener("blur", () => {
        const raw = parseInt(input.value);
        if (!isNaN(raw)) {
          const val = setNumberBoxValue(raw);
          updateConfig(val);
        }
      });

      input.addEventListener("keydown", (e) => {
        if (e.key === "ArrowUp") {
          e.preventDefault();
          const val = setNumberBoxValue(parseInt(input.value) + 5);
          updateConfig(val);
        } else if (e.key === "ArrowDown") {
          e.preventDefault();
          const val = setNumberBoxValue(parseInt(input.value) - 5);
          updateConfig(val);
        }
      });

      spin.appendChild(btnUp);
      spin.appendChild(btnDown);
      numberboxBorder.appendChild(focusBorder);
      numberboxBorder.appendChild(input);
      numberboxBorder.appendChild(spin);
      numberbox.appendChild(numberboxBorder);

      const { toggle: deviceToggle, input: deviceInput } = createToggle(isEnabled, async (deviceInput) => {
        if (deviceInput.checked) {
          config.shutdown_volume_devices[dev.name] = parseInt(input.value) / 100;
          nameEl.classList.remove("hidden");
          input.disabled = false;
          btnUp.disabled = parseInt(input.value) >= 100;
          btnDown.disabled = parseInt(input.value) <= 0;
        } else {
          delete config.shutdown_volume_devices[dev.name];
          nameEl.classList.add("hidden");
          input.disabled = true;
          btnUp.disabled = true;
          btnDown.disabled = true;
        }
        await saveConfig();
      });

      controls.appendChild(numberbox);
      controls.appendChild(deviceToggle);
      item.appendChild(nameEl);
      item.appendChild(controls);
      items.appendChild(item);
    }
  } catch (e) {
    console.error("Failed to load audio devices for shutdown volume:", e);
  }
}
