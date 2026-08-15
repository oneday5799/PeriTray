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

async function initShutdownVolumeSettings() {
  const toggle = document.getElementById("toggle-shutdown-volume");
  const settingsWrap = document.getElementById("shutdown-volume-settings");
  const deviceList = document.getElementById("shutdown-device-list");
  const shutdownArrow = document.getElementById("arrow-shutdown");

  toggle.checked = config.shutdown_volume_enabled || false;
  settingsWrap.style.display = toggle.checked ? "block" : "none";
  if (shutdownArrow) shutdownArrow.classList.toggle("expanded", toggle.checked);

  toggle.addEventListener("change", async () => {
    config.shutdown_volume_enabled = toggle.checked;
    settingsWrap.style.display = toggle.checked ? "block" : "none";
    if (shutdownArrow) shutdownArrow.classList.toggle("expanded", toggle.checked);
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
        await saveConfig();
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
            await saveConfig();
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
