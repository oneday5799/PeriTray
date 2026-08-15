let audioDevices = [];
let audioSessions = [];
let selectedDeviceId = null;
let hiddenAudioDevices = [];
let audioDeviceNames = {};
let deviceShortcuts = {};
let activeAudioMenu = null;
registerContextMenu({ get menu() { return activeAudioMenu; }, set menu(v) { activeAudioMenu = v; } });

document.addEventListener("mouseup", () => {
  document.querySelectorAll('input[type="range"]').forEach(s => { s._isDragging = false; });
});

function updateSessionCard(session) {
  const cards = document.querySelectorAll('.audio-session-card');
  for (const card of cards) {
    if (card.dataset.sessionId === session.id) {
      updateSliderValue(card.querySelector('.volume-slider'), session.volume);
      updateMuteButton(card.querySelector('.mute-btn'), session.is_muted);
      break;
    }
  }
}

if (window.__TAURI__ && window.__TAURI__.event) {
  window.__TAURI__.event.listen('volume-changed', (event) => {
    const changes = event.payload;
    if (Array.isArray(changes)) {
      for (const change of changes) {
        const device = audioDevices.find(d => d.id === change.device_id);
        if (device) {
          device.volume = change.volume;
          device.is_muted = change.is_muted;
          updateDeviceCard(device);
        }
        if (change.session_id) {
          const session = audioSessions.find(s => s.id === change.session_id);
          if (session) {
            session.volume = change.volume;
            session.is_muted = change.is_muted;
            updateSessionCard(session);
          }
        }
      }
    }
  });

  window.__TAURI__.event.listen('audio-devices-changed', () => {
    loadAudioDevices();
  });
}

function updateDeviceCard(device) {
  const cards = document.querySelectorAll('.audio-device-card');
  let targetCard = null;
  for (const card of cards) {
    if (card.dataset.deviceId === device.id) {
      targetCard = card;
      break;
    }
  }
  if (!targetCard) return;

  updateSliderValue(targetCard.querySelector('.volume-slider'), device.volume);
  updateMuteButton(targetCard.querySelector('.mute-btn'), device.is_muted);
}

function updateSliderValue(slider, volume) {
  if (slider && document.activeElement !== slider) {
    slider.value = Math.round(volume * 100);
    updateSliderGradient(slider);
  }
}

function updateMuteButton(muteBtn, isMuted) {
  if (muteBtn) {
    muteBtn.className = "mute-btn" + (isMuted ? " muted" : "");
    muteBtn.innerHTML = isMuted ? getMuteIcon() : getVolumeIcon();
  }
}

function createSliderTooltip(slider) {
  const tooltip = document.createElement("div");
  tooltip.className = "slider-tooltip";
  tooltip.textContent = slider.value;
  tooltip.style.display = "none";
  slider.parentElement.style.position = "relative";
  slider.parentElement.appendChild(tooltip);

  let hideTimer = null;

  function positionTooltip() {
    const min = parseFloat(slider.min);
    const max = parseFloat(slider.max);
    const val = parseFloat(slider.value);
    const pct = (val - min) / (max - min);
    const trackWidth = slider.offsetWidth;
    const thumbWidth = 18;
    const center = thumbWidth / 2 + pct * (trackWidth - thumbWidth);
    tooltip.textContent = slider.value;
    tooltip.style.left = `${center}px`;
  }

  function showTooltip() {
    tooltip.style.display = "";
    positionTooltip();
    clearTimeout(hideTimer);
    hideTimer = setTimeout(() => { tooltip.style.display = "none"; }, 2000);
  }

  function hideTooltip() {
    clearTimeout(hideTimer);
    tooltip.style.display = "none";
  }

  function isOverThumb(e) {
    const rect = slider.getBoundingClientRect();
    const mouseX = e.clientX - rect.left;
    const trackWidth = rect.width;
    const min = parseFloat(slider.min);
    const max = parseFloat(slider.max);
    const val = parseFloat(slider.value);
    const pct = (val - min) / (max - min);
    const thumbWidth = 18;
    const thumbCenter = thumbWidth / 2 + pct * (trackWidth - thumbWidth);
    return Math.abs(mouseX - thumbCenter) <= thumbWidth / 2 + 4;
  }

  slider.addEventListener("mousemove", (e) => {
    if (slider._isDragging || isOverThumb(e)) {
      showTooltip();
    } else {
      hideTooltip();
    }
  });

  slider.addEventListener("mouseenter", () => {
    if (slider._isDragging) showTooltip();
  });

  slider.addEventListener("mouseleave", () => {
    if (!slider._isDragging) hideTooltip();
  });

  slider.addEventListener("mousedown", () => {
    slider._isDragging = true;
    showTooltip();
  });

  slider.addEventListener("blur", () => {
    slider._isDragging = false;
    hideTooltip();
  });

  slider.addEventListener("input", showTooltip);

  slider.addEventListener("wheel", (e) => {
    e.preventDefault();
    const min = parseFloat(slider.min);
    const max = parseFloat(slider.max);
    let val = parseFloat(slider.value);
    val = e.deltaY < 0 ? Math.min(val + 1, max) : Math.max(val - 1, min);
    slider.value = val;
    slider.dispatchEvent(new Event("input"));
  }, { passive: false });

  return tooltip;
}

function showAudioContextMenu(x, y, device) {
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
      deviceName: device.name,
      displayName: audioDeviceNames[device.name] || device.name,
      nameSource: audioDeviceNames[device.name],
      onUpdate: (names) => { audioDeviceNames = names; },
      onRender: renderAudioDevices,
    });
  });
  menu.appendChild(renameItem);

  const hideItem = document.createElement("div");
  hideItem.className = "context-menu-item";
  hideItem.textContent = "隐藏";
  hideItem.addEventListener("click", async () => {
    await invoke("toggle_audio_device_hidden", { name: device.name });
    const config = await invoke("get_config");
    hiddenAudioDevices = config.hidden_audio_devices || [];
    renderAudioDevices();
    hideAllContextMenus();
  });
  menu.appendChild(hideItem);

  const shortcutItem = document.createElement("div");
  shortcutItem.className = "context-menu-item";
  shortcutItem.textContent = "快捷键";
  shortcutItem.addEventListener("click", () => {
    hideAllContextMenus();
    showDeviceShortcutDialog(device);
  });
  menu.appendChild(shortcutItem);

  document.body.appendChild(menu);
  clampMenuPosition(menu, x, y);
  activeAudioMenu = menu;
}

function showDeviceShortcutDialog(device) {
  const invoke = getInvoke();
  if (!invoke) return;

  const wrap = document.createElement("div");
  wrap.className = "shortcut-dialog";

  const deviceLabel = document.createElement("div");
  deviceLabel.className = "shortcut-dialog-device";
  deviceLabel.textContent = audioDeviceNames[device.name] || device.name;
  wrap.appendChild(deviceLabel);

  const row = document.createElement("div");
  row.className = "shortcut-dialog-row";

  const input = document.createElement("input");
  input.type = "text";
  input.className = "shortcut-key-input dialog-input";
  input.placeholder = "点击录制快捷键";
  input.readOnly = true;

  const clearBtn = document.createElement("button");
  clearBtn.className = "shortcut-clear-btn";
  clearBtn.textContent = "×";
  clearBtn.title = "清除快捷键";
  clearBtn.style.display = "none";

  const hint = document.createElement("div");
  hint.className = "shortcut-dialog-hint";
  hint.textContent = "点击输入框后按下键盘组合键，用于快速切换到此设备。在设置中开启共享开关后，多个设备可共用同一快捷键，按下时按设备列表顺序循环切换。";

  row.appendChild(input);
  row.appendChild(clearBtn);
  wrap.appendChild(row);
  wrap.appendChild(hint);

  const buttons = [];
  buttons.push({
    text: "取消",
    className: "cancel",
    onClick: () => closeDialog(overlay),
  });
  buttons.push({
    text: "完成",
    className: "confirm",
    onClick: () => closeDialog(overlay),
  });

  const overlay = createDialog({
    title: "设置设备快捷键",
    content: [wrap],
    buttons,
  });

  bindShortcutRecorder(
    input,
    clearBtn,
    () => (deviceShortcuts[device.id] || {}).shortcut || null,
    (display, shortcut) => {
      if (shortcut === "") {
        invoke("set_device_shortcut", { deviceId: device.id, name: device.name, key: null }).catch(() => {});
        deviceShortcuts[device.id] = { name: device.name, shortcut: null };
        input.value = "";
        clearBtn.style.display = "none";
        input.placeholder = "点击录制快捷键";
        return;
      }
      invoke("set_device_shortcut", { deviceId: device.id, name: device.name, key: shortcut })
        .then(() => {
          deviceShortcuts[device.id] = { name: device.name, shortcut };
          clearBtn.style.display = "";
          hint.textContent = `快捷键 "${display}" 已保存`;
          hint.style.color = "#4caf50";
          setTimeout(() => {
            hint.textContent = "点击输入框后按下键盘组合键，用于快速切换到此设备。在设置中开启共享开关后，多个设备可共用同一快捷键，按下时按设备列表顺序循环切换。";
            hint.style.color = "#999";
          }, 2500);
        })
        .catch((err) => {
          const msg = String(err);
          if (msg.includes("已被占用")) {
            hint.textContent = `"${display}" 已被其他功能占用，请选择其他快捷键。`;
          } else {
            hint.textContent = "暂不支持该快捷键。";
          }
          hint.style.color = "#e81123";
        });
    }
  );
}

async function loadAudioDevices() {
  const list = document.getElementById("audio-device-list");
  const invoke = getInvoke();
  if (!invoke) {
    return;
  }
  try {
    const [devices, cfg] = await Promise.all([invoke("get_audio_devices"), invoke("get_config")]);
    audioDevices = devices;
    hiddenAudioDevices = cfg.hidden_audio_devices || [];
    audioDeviceNames = cfg.device_names || {};
    deviceShortcuts = cfg.device_shortcuts || {};
    renderAudioDevices();
    if (audioDevices.length > 0 && !selectedDeviceId) {
      const firstVisible = audioDevices.find(d => !hiddenAudioDevices.includes(d.name));
      if (firstVisible) selectDevice(firstVisible.id);
    }
  } catch (e) {
    if (list.querySelectorAll('.audio-device-card').length === 0) {
      list.innerHTML = `<div class="loading">加载失败: ${e}</div>`;
    }
  }
}

function renderAudioDevices() {
  const list = document.getElementById("audio-device-list");
  const visibleDevices = audioDevices.filter(d => !hiddenAudioDevices.includes(d.name));
  if (visibleDevices.length === 0) {
    list.innerHTML = audioDevices.length === 0
      ? '<div class="loading">没有检测到音频设备</div>'
      : '<div class="loading">所有音频设备已隐藏</div>';
    return;
  }

  list.querySelectorAll('.loading').forEach(el => el.remove());

  const existingCards = new Map();
  list.querySelectorAll('.audio-device-card').forEach(card => {
    existingCards.set(card.dataset.deviceId, card);
  });

  const newIds = new Set(visibleDevices.map(d => d.id));

  existingCards.forEach((card, id) => {
    if (!newIds.has(id)) {
      card.remove();
    }
  });

  for (const device of visibleDevices) {
    let card = existingCards.get(device.id);

    if (card) {
      updateAudioDeviceCard(card, device);
    } else {
      card = createAudioDeviceCard(device);
      list.appendChild(card);
    }
  }
}

function createAudioDeviceCard(device) {
  const card = document.createElement("div");
  card.className = "audio-device-card";
  card.dataset.deviceId = device.id;
  card.dataset.deviceName = device.name;

  const header = document.createElement("div");
  header.className = "audio-device-header";

  const nameEl = document.createElement("div");
  nameEl.className = "audio-device-name" + (device.is_default ? " default" : "");
  nameEl.textContent = audioDeviceNames[device.name] || device.name;
  if (device.is_default) {
    const badge = document.createElement("span");
    badge.className = "default-badge";
    badge.textContent = "(默认)";
    nameEl.appendChild(badge);
  }
  nameEl.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (nameEl.classList.contains("default")) return;
    const invoke = getInvoke();
    if (!invoke) return;
    try {
      nameEl.classList.add("default");
      if (!nameEl.querySelector('.default-badge')) {
        const badge = document.createElement("span");
        badge.className = "default-badge";
        badge.textContent = "(默认)";
        nameEl.appendChild(badge);
      }
      await invoke("set_default_device", { deviceId: device.id });
      await new Promise(r => setTimeout(r, 500));
      await loadAudioDevices();
      selectDevice(device.id);
    } catch (err) {
      nameEl.classList.remove("default");
      const badge = nameEl.querySelector('.default-badge');
      if (badge) badge.remove();
      console.error("Failed to set default device:", err);
    }
  });
  header.appendChild(nameEl);
  card.appendChild(header);

  const controls = document.createElement("div");
  controls.className = "audio-device-controls";

  const slider = document.createElement("input");
  slider.type = "range";
  slider.className = "volume-slider";
  slider.min = "0";
  slider.max = "100";
  slider.value = Math.round(device.volume * 100);

  const throttledSetDeviceVolume = throttle(setDeviceVolume, 150);

  slider.addEventListener("input", (e) => {
    const value = parseInt(e.target.value) / 100;
    device.volume = value;
    updateSliderGradient(e.target);
    throttledSetDeviceVolume(device.id, value);
  });
  slider.addEventListener("change", () => {
    setTimeout(() => slider.blur(), 100);
  });
  updateSliderGradient(slider);
  controls.appendChild(slider);

  createSliderTooltip(slider);

  const muteBtn = document.createElement("button");
  muteBtn.className = "mute-btn" + (device.is_muted ? " muted" : "");
  muteBtn.innerHTML = device.is_muted ? getMuteIcon() : getVolumeIcon();
  muteBtn.addEventListener("click", () => toggleDeviceMute(device.id));
  controls.appendChild(muteBtn);

  card.appendChild(controls);

  card.addEventListener("click", (e) => {
    if (e.target.tagName !== 'INPUT' && e.target.tagName !== 'BUTTON') {
      selectDevice(device.id);
    }
  });

  card.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showAudioContextMenu(e.clientX, e.clientY, device);
  });

  return card;
}

function updateAudioDeviceCard(card, device) {

  const nameEl = card.querySelector('.audio-device-name');
  if (nameEl) {
    const displayName = audioDeviceNames[device.name] || device.name;
    const firstChild = nameEl.firstChild;
    if (firstChild && firstChild.nodeType === Node.TEXT_NODE) {
      if (firstChild.textContent !== displayName) {
        firstChild.textContent = displayName;
      }
    }
    if (device.is_default) {
      nameEl.classList.add("default");
      if (!nameEl.querySelector('.default-badge')) {
        const badge = document.createElement("span");
        badge.className = "default-badge";
        badge.textContent = "(默认)";
        nameEl.appendChild(badge);
      }
    } else {
      nameEl.classList.remove("default");
      const badge = nameEl.querySelector('.default-badge');
      if (badge) badge.remove();
    }
  }

  updateSliderValue(card.querySelector('.volume-slider'), device.volume);
  updateMuteButton(card.querySelector('.mute-btn'), device.is_muted);
}

function selectDevice(deviceId) {
  selectedDeviceId = deviceId;
  renderAudioDevices();
  loadAudioSessions(deviceId);
}

async function loadAudioSessions(deviceId) {
  const list = document.getElementById("audio-session-list");
  const invoke = getInvoke();
  if (!invoke) {
    return;
  }
  try {
    audioSessions = await invoke("get_audio_sessions", { deviceId });
    renderAudioSessions();
  } catch (e) {
    if (list.querySelectorAll('.audio-session-card').length === 0) {
      list.innerHTML = `<div class="loading">加载失败: ${e}</div>`;
    }
  }
}

function renderAudioSessions() {
  const list = document.getElementById("audio-session-list");
  if (audioSessions.length === 0) {
    list.innerHTML = '<div class="loading">没有正在播放的应用</div>';
    return;
  }

  list.querySelectorAll('.loading').forEach(el => el.remove());

  const existingCards = new Map();
  list.querySelectorAll('.audio-session-card').forEach(card => {
    existingCards.set(card.dataset.sessionId, card);
  });

  const newIds = new Set(audioSessions.map(s => s.id));

  existingCards.forEach((card, id) => {
    if (!newIds.has(id)) {
      card.remove();
    }
  });

  for (const session of audioSessions) {
    let card = existingCards.get(session.id);

    if (card) {
      updateAudioSessionCard(card, session);
    } else {
      card = createAudioSessionCard(session);
      list.appendChild(card);
    }
  }
}

function createAudioSessionCard(session) {
  const card = document.createElement("div");
  card.className = "audio-session-card";
  card.dataset.sessionId = session.id;

  const iconEl = document.createElement("div");
  iconEl.className = "session-icon";
  if (session.icon && session.icon.length > 100) {
    const img = document.createElement("img");
    img.src = `data:image/png;base64,${session.icon}`;
    img.style.width = "100%";
    img.style.height = "100%";
    img.style.borderRadius = "4px";
    img.onerror = () => { iconEl.textContent = session.name.charAt(0).toUpperCase(); };
    iconEl.style.background = "transparent";
    iconEl.appendChild(img);
  } else {
    iconEl.textContent = session.name.charAt(0).toUpperCase();
    iconEl.style.background = stringToColor(session.name);
    iconEl.style.color = "#fff";
    iconEl.style.fontWeight = "bold";
  }
  card.appendChild(iconEl);

  const controls = document.createElement("div");
  controls.className = "session-controls";

  const slider = document.createElement("input");
  slider.type = "range";
  slider.className = "volume-slider session-slider";
  slider.min = "0";
  slider.max = "100";
  slider.value = Math.round(session.volume * 100);

  const throttledSetSessionVolume = throttle(setSessionVolume, 100);

  slider.addEventListener("input", async (e) => {
    const value = parseInt(e.target.value) / 100;
    const sess = audioSessions.find(s => s.id === card.dataset.sessionId);
    if (sess) sess.volume = value;
    updateSliderGradient(e.target);
    throttledSetSessionVolume(card.dataset.sessionId, value);
  });
  slider.addEventListener("change", () => {
    setTimeout(() => slider.blur(), 100);
  });
  updateSliderGradient(slider);
  controls.appendChild(slider);

  createSliderTooltip(slider);

  const muteBtn = document.createElement("button");
  muteBtn.className = "mute-btn" + (session.is_muted ? " muted" : "");
  muteBtn.innerHTML = session.is_muted ? getMuteIcon() : getVolumeIcon();
  muteBtn.addEventListener("click", async () => {
    const sessionId = card.dataset.sessionId;
    await toggleSessionMute(sessionId);
    const sess = audioSessions.find(s => s.id === sessionId);
    if (sess) {
      sess.is_muted = !sess.is_muted;
      muteBtn.className = "mute-btn" + (sess.is_muted ? " muted" : "");
      muteBtn.innerHTML = sess.is_muted ? getMuteIcon() : getVolumeIcon();
    }
  });
  controls.appendChild(muteBtn);

  card.appendChild(controls);
  return card;
}

function updateAudioSessionCard(card, session) {
  updateSliderValue(card.querySelector('.volume-slider'), session.volume);
  updateMuteButton(card.querySelector('.mute-btn'), session.is_muted);
}

async function setDeviceVolume(deviceId, volume) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    await invoke("set_device_volume", { deviceId, volume });
  } catch (e) {
    console.error("Failed to set volume:", e);
  }
}

async function toggleDeviceMute(deviceId) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    await invoke("toggle_device_mute", { deviceId });
    const device = audioDevices.find(d => d.id === deviceId);
    if (device) {
      device.is_muted = !device.is_muted;
      renderAudioDevices();
    }
  } catch (e) {
    console.error("Failed to toggle mute:", e);
  }
}

async function setSessionVolume(sessionId, volume) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    await invoke("set_session_volume", { sessionId, volume });
  } catch (e) {
    console.error("Failed to set session volume:", e);
  }
}

async function toggleSessionMute(sessionId) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    await invoke("toggle_session_mute", { sessionId });
  } catch (e) {
    console.error("Failed to toggle session mute:", e);
  }
}

function getVolumeIcon() {
  return `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/>
    <path d="M19.07 4.93a10 10 0 0 1 0 14.14"/>
    <path d="M15.54 8.46a5 5 0 0 1 0 7.07"/>
  </svg>`;
}

function getMuteIcon() {
  return `<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
    <polygon points="11 5 6 9 2 9 2 15 6 15 11 19 11 5"/>
    <line x1="23" y1="9" x2="17" y2="15"/>
    <line x1="17" y1="9" x2="23" y2="15"/>
  </svg>`;
}

function stringToColor(str) {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    hash = str.charCodeAt(i) + ((hash << 5) - hash);
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue}, 60%, 50%)`;
}
