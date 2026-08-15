window.__TAURI__.event.listen("config-changed", async () => {
  loadDevicesAsync().catch(console.error);
  loadAudioDevicesAsync().catch(console.error);
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

let audioExpanded = true;

function renderAudioDeviceGroups(audioDevices) {
  const container = document.getElementById("audio-device-items");
  const arrow = document.getElementById("arrow-audio");
  container.innerHTML = "";

  if (audioDevices.length === 0) {
    container.innerHTML = '<div class="device-item"><div class="device-item-name" style="color:#888">没有检测到音频设备</div></div';
    if (arrow) arrow.classList.remove("expanded");
    container.style.maxHeight = "0px";
    return;
  }

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
  const header = document.getElementById("audio-card-header");
  const items = document.getElementById("audio-device-items");
  const arrow = document.getElementById("arrow-audio");

  if (!header || !items || !arrow) return;

  header.addEventListener("click", () => {
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

async function initShutdownVolumeSettings() {
  const toggle = document.getElementById("toggle-shutdown-volume");
  const items = document.getElementById("shutdown-device-items");
  const arrow = document.getElementById("arrow-shutdown");
  const header = document.getElementById("shutdown-card-header");
  const deviceList = document.getElementById("shutdown-device-list");

  toggle.checked = config.shutdown_volume_enabled || false;

  function setShutdownExpanded(expanded) {
    if (expanded) {
      items.classList.add("show");
      items.style.maxHeight = items.scrollHeight + "px";
    } else {
      items.style.maxHeight = "0px";
    }
    if (arrow) arrow.classList.toggle("expanded", expanded);
  }

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

  header.addEventListener("click", (e) => {
    if (e.target.closest(".toggle") || e.target.closest("input")) return;
    const isExpanded = items.classList.toggle("show");
    if (isExpanded) {
      items.style.maxHeight = items.scrollHeight + "px";
    } else {
      items.style.maxHeight = "0px";
    }
    if (arrow) arrow.classList.toggle("expanded", isExpanded);
  });

  toggle.addEventListener("change", async () => {
    config.shutdown_volume_enabled = toggle.checked;
    setShutdownExpanded(toggle.checked);
    await saveConfig();
  });

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
        const val = clamp(Math.round(v / 5) * 5);
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

      const deviceToggle = document.createElement("label");
      deviceToggle.className = "toggle";
      const deviceInput = document.createElement("input");
      deviceInput.type = "checkbox";
      deviceInput.checked = isEnabled;
      const deviceSlider = document.createElement("span");
      deviceSlider.className = "slider";
      deviceToggle.appendChild(deviceInput);
      deviceToggle.appendChild(deviceSlider);

      deviceInput.addEventListener("change", async () => {
        if (deviceInput.checked) {
          config.shutdown_volume_devices[dev.name] = parseInt(input.value) / 100;
          nameEl.classList.remove("inactive");
          input.disabled = false;
          btnUp.disabled = parseInt(input.value) >= 100;
          btnDown.disabled = parseInt(input.value) <= 0;
        } else {
          delete config.shutdown_volume_devices[dev.name];
          nameEl.classList.add("inactive");
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
      deviceList.appendChild(item);
    }
  } catch (e) {
    console.error("Failed to load audio devices for shutdown volume:", e);
  }
}
