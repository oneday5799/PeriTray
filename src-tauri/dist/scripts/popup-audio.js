/* popup-audio.js — 主窗口·音量控制 tab：设备/会话音量滑块渲染与调节/mute 切换/
 *            滚轮微调与 tooltip/强制静音记账/volume-changed 监听
 * 加载序 2/4 · 提供：loadAudioDevices()/loadAudioSessions()/renderAudioDevices()/renderAudioSessions()
 * 依赖：common.js(getInvoke/describeShortcutError/attachSessionTooltip/showToast/createSubmenuShell/
 *       formatDeviceName/registerContextMenu/clampMenuPosition/hideAllContextMenus/
 *       showRenameDialog/createDialog/closeDialog/bindShortcutRecorder/attachTooltip) */
let audioDevices = [];
let audioSessions = [];
let selectedDeviceId = null;
let hiddenAudioDevices = [];
let audioDeviceNames = {};
let deviceShortcuts = {};
let muteLockEnabled = false;
let fineAdjustEnabled = false;
let simplifyDeviceNames = true;
let forceMuteDevices = [];
let spatialSoundEnabled = false;
const forceMuteHold = {};
const forceMutePrevVolume = {};
const buttonMutedDevices = new Set();
let activeAudioMenu = null;

// 设备显示名：有重命名使用重命名，否则简化括号内名称（数据源为本页运行态）
function deviceDisplayName(name) {
  return window.formatDeviceName(name, audioDeviceNames, simplifyDeviceNames);
}

// config -> 本页运行态字段（config-changed 监听与初次加载共用）
function applyAudioRuntimeConfig(cfg) {
  muteLockEnabled = !!cfg.mute_lock;
  fineAdjustEnabled = !!cfg.volume_fine_adjust;
  spatialSoundEnabled = !!cfg.enable_spatial_sound;
  simplifyDeviceNames = cfg.simplify_device_names !== false;
  forceMuteDevices = cfg.force_mute_devices || [];
}
registerContextMenu({ get menu() { return activeAudioMenu; }, set menu(v) { activeAudioMenu = v; } });

// ── 音量滑块工具（本页专属，自 common.js 迁入） ──────────────

// 节流：首次立即执行，后续在 delay 窗口内合并且窗口末补发最后一次
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

// 按当前值渲染滑轨填充渐变
function updateSliderGradient(slider) {
  const value = slider.value;
  const percentage = ((value - slider.min) / (slider.max - slider.min)) * 100;
  slider.style.setProperty("--track-color", `linear-gradient(to right, #0078d7 0%, #0078d7 ${percentage}%, var(--slider-track, #e0e0e0) ${percentage}%, var(--slider-track, #e0e0e0) 100%)`);
}

document.addEventListener("mouseup", () => {
  document.querySelectorAll('input[type="range"]').forEach(s => { s._isDragging = false; });
});

function updateSessionCard(session) {
  const cards = document.querySelectorAll(".card.session");
  for (const card of cards) {
    if (card.dataset.sessionId === session.id) {
      updateSliderValue(card.querySelector(".volume-slider"), session.volume);
      updateMuteButton(card.querySelector(".mute-btn"), session.is_muted, session.volume, session.permanentMute);
      break;
    }
  }
}

if (window.__TAURI__ && window.__TAURI__.event) {
  window.__TAURI__.event.listen("volume-changed", (event) => {
    const changes = event.payload;
    if (Array.isArray(changes)) {
      for (const change of changes) {
        const device = audioDevices.find(d => d.id === change.device_id);
        if (device) {
          const isFM = forceMuteDevices.includes(device.name);
          const hold = isFM ? forceMuteHold[device.name] : null;
          if (hold) {
            device.is_muted = hold.muted;
            device.volume = hold.volume;
          } else if (isFM && change.is_muted) {
            device.is_muted = true;
          } else {
            device.volume = change.volume;
            device.is_muted = change.is_muted;
          }
          if (!device.is_muted) {
            device.permanentMute = false;
            buttonMutedDevices.delete(device.id);
          }
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

  window.__TAURI__.event.listen("audio-devices-changed", () => {
    loadAudioDevices();
  });

  window.__TAURI__.event.listen("config-changed", async () => {
    try {
      const cfg = await getInvoke()("get_config");
      applyAudioRuntimeConfig(cfg);
      for (const d of audioDevices) {
        d.permanentMute = muteLockEnabled && buttonMutedDevices.has(d.id);
      }
      for (const s of audioSessions) {
        s.permanentMute = muteLockEnabled && !!(s.is_muted && s.volume > 0);
      }
      document.querySelectorAll(".volume-slider").forEach(s => {
        s.step = fineAdjustEnabled ? "0.1" : "1";
      });
      renderAudioDevices();
      renderAudioSessions();
    } catch (e) {
      console.error("Failed to reload mute lock config:", e);
    }
  });
}

function updateDeviceCard(device) {
  const cards = document.querySelectorAll(".card.audio-device");
  let targetCard = null;
  for (const card of cards) {
    if (card.dataset.deviceId === device.id) {
      targetCard = card;
      break;
    }
  }
  if (!targetCard) return;

  updateSliderValue(targetCard.querySelector(".volume-slider"), device.volume);
  updateMuteButton(targetCard.querySelector(".mute-btn"), device.is_muted, device.volume, device.permanentMute);
}

function updateSliderValue(slider, volume) {
  if (slider && document.activeElement !== slider) {
    slider.value = fineAdjustEnabled ? Math.round(volume * 1000) / 10 : Math.round(volume * 100);
    updateSliderGradient(slider);
  }
}

function muteStateText(isMuted, volume) {
  return (isMuted || !(volume > 0)) ? "已静音" : "未静音";
}

function updateMuteButton(muteBtn, isMuted, volume, permanent) {
  if (muteBtn) {
    muteBtn.classList.toggle("muted", !!(isMuted && permanent));
    const html = isMuted ? getMuteIcon() : getVolumeIcon(volume);
    const iconEl = muteBtn.querySelector(".mute-icon");
    if (iconEl && iconEl.innerHTML !== html) iconEl.innerHTML = html;
    const tip = muteBtn.querySelector(".tooltip-content");
    const tipText = muteStateText(isMuted, volume);
    if (tip && tip.textContent !== tipText) tip.textContent = tipText;
  }
}

function createSliderTooltip(slider) {
  const tooltip = document.createElement("div");
  tooltip.className = "slider-tooltip";
  tooltip.textContent = slider.value;
  tooltip.style.display = "none";
  slider.parentElement.style.position = "relative";
  slider.parentElement.appendChild(tooltip);

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
  }

  function hideTooltip() {
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

  slider.addEventListener("mouseup", () => {
    slider._isDragging = false;
    if (slider.matches(":hover")) {
      showTooltip();
    } else {
      hideTooltip();
    }
  });

  slider.addEventListener("blur", () => {
    slider._isDragging = false;
    if (!slider.matches(":hover")) hideTooltip();
  });

  slider.addEventListener("input", showTooltip);

  slider.addEventListener("wheel", (e) => {
    e.preventDefault();
    const min = parseFloat(slider.min);
    const max = parseFloat(slider.max);
    let val = parseFloat(slider.value);
    if (fineAdjustEnabled) {
      val = Math.round((val + (e.deltaY < 0 ? 0.1 : -0.1)) * 10) / 10;
    } else {
      val = e.deltaY < 0 ? Math.floor(val) + 1 : Math.ceil(val) - 1;
    }
    val = Math.min(Math.max(val, min), max);
    slider.value = val;
    slider.dispatchEvent(new Event("input"));
  }, { passive: false });

  return tooltip;
}

let audioMenuToken = 0;

async function showAudioContextMenu(x, y, device) {
  hideAllContextMenus();
  const invoke = getInvoke();
  if (!invoke) return;
  const token = ++audioMenuToken;

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

  // 空间音效（实验性，设置页开关控制）：查询当前格式后追加子菜单；接口不可用时降级为系统设置入口
  if (spatialSoundEnabled) {
    const spatialState = await invoke("get_spatial_sound", { deviceId: device.id }).catch(() => null);
    if (token !== audioMenuToken) return;
    if (spatialState && Array.isArray(spatialState.supported) && spatialState.supported.length > 0) {
      buildSpatialSoundSubmenu(menu, device, spatialState);
    } else {
      const fallbackItem = document.createElement("div");
      fallbackItem.className = "context-menu-item";
      fallbackItem.textContent = "空间音效（系统设置）";
      fallbackItem.addEventListener("click", () => {
        hideAllContextMenus();
        const isDefault = audioDevices.find(d => d.id === device.id)?.is_default;
        const url = isDefault ? "ms-settings:sound-defaultoutputproperties" : "ms-settings:sound-devices";
        invoke("open_url", { url }).catch(() => {});
      });
      menu.appendChild(fallbackItem);
    }
  }

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
  deviceLabel.textContent = deviceDisplayName(device.name);
  wrap.appendChild(deviceLabel);

  const DEFAULT_HINT = "点击输入框后按下键盘组合键，用于快速切换到此设备。在设置中开启共享开关后，多个设备可共用同一快捷键，按下时按设备列表顺序循环切换。";

  const row = document.createElement("div");
  row.className = "shortcut-dialog-row";

  const input = document.createElement("input");
  input.type = "text";
  input.className = "shortcut-key-input dialog-input";
  input.placeholder = "点击录制快捷键";
  input.readOnly = true;

  const hint = document.createElement("div");
  hint.className = "shortcut-dialog-hint";
  hint.textContent = DEFAULT_HINT;

  row.appendChild(input);
  wrap.appendChild(row);
  wrap.appendChild(hint);

  let savedShortcut = (deviceShortcuts[device.id] || {}).shortcut || null;
  let clearBtn = null;

  const clearShortcut = () => {
    invoke("set_device_shortcut", { deviceId: device.id, name: device.name, key: null }).catch(() => {});
    deviceShortcuts[device.id] = { name: device.name, shortcut: null };
    savedShortcut = null;
    input.value = "";
    input.placeholder = "点击录制快捷键";
    if (clearBtn) clearBtn.disabled = true;
    hint.textContent = DEFAULT_HINT;
    hint.style.color = "#999";
  };

  const buttons = [];
  buttons.push({
    text: "清除",
    className: "danger",
    onClick: clearShortcut,
  });
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

  clearBtn = overlay.querySelector(".dialog-btn.danger");
  if (clearBtn) clearBtn.disabled = !savedShortcut;

  bindShortcutRecorder(
    input,
    null,
    () => savedShortcut,
    (display, shortcut) => {
      invoke("set_device_shortcut", { deviceId: device.id, name: device.name, key: shortcut })
        .then(() => {
          deviceShortcuts[device.id] = { name: device.name, shortcut };
          savedShortcut = shortcut;
          if (clearBtn) clearBtn.disabled = false;
          hint.textContent = `快捷键 "${display}" 已保存`;
          hint.style.color = "#4caf50";
          setTimeout(() => {
            hint.textContent = DEFAULT_HINT;
            hint.style.color = "#999";
          }, 2500);
        })
        .catch((err) => {
          hint.textContent = describeShortcutError(err, display);
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
    applyAudioRuntimeConfig(cfg);
    audioDevices = devices.map(d => ({ ...d, permanentMute: muteLockEnabled && buttonMutedDevices.has(d.id) }));
    hiddenAudioDevices = cfg.hidden_audio_devices || [];
    audioDeviceNames = cfg.device_names || {};
    deviceShortcuts = cfg.device_shortcuts || {};
    renderAudioDevices();
    if (audioDevices.length > 0 && !selectedDeviceId) {
      const firstVisible = audioDevices.find(d => !hiddenAudioDevices.includes(d.name));
      if (firstVisible) selectDevice(firstVisible.id);
    }
  } catch (e) {
    if (list.querySelectorAll(".card.audio-device").length === 0) {
      list.innerHTML = `<div class="loading">加载失败: ${e}</div>`;
    }
  }
}

// 对账式渲染骨架：按 id 差集移除失效卡，已有卡走 update，新增项建卡后追加
function reconcileCards(list, cardSelector, idProp, items, createCard, updateCard) {
  const existingCards = new Map();
  list.querySelectorAll(cardSelector).forEach(card => {
    existingCards.set(card.dataset[idProp], card);
  });

  const newIds = new Set(items.map(item => item.id));

  existingCards.forEach((card, id) => {
    if (!newIds.has(id)) {
      card.remove();
    }
  });

  for (const item of items) {
    let card = existingCards.get(item.id);

    if (card) {
      updateCard(card, item);
    } else {
      list.appendChild(createCard(item));
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

  list.querySelectorAll(".loading").forEach(el => el.remove());

  reconcileCards(list, ".card.audio-device", "deviceId", visibleDevices,
    createAudioDeviceCard, updateAudioDeviceCard);
}

// 确保"(默认)"角标存在（幂等，已存在则跳过）
function ensureDefaultBadge(nameEl) {
  if (!nameEl.querySelector(".default-badge")) {
    const badge = document.createElement("span");
    badge.className = "default-badge";
    badge.textContent = "(默认)";
    nameEl.appendChild(badge);
  }
}

function createAudioDeviceCard(device) {
  const card = document.createElement("div");
  card.className = "card column audio-device";
  card.dataset.deviceId = device.id;

  const header = document.createElement("div");
  header.className = "card-left";

  const nameEl = document.createElement("div");
  nameEl.className = "card-title audio-device-name" + (device.is_default ? " default" : "");
  nameEl.textContent = deviceDisplayName(device.name);
  if (device.is_default) ensureDefaultBadge(nameEl);
  nameEl.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (nameEl.classList.contains("default")) return;
    const invoke = getInvoke();
    if (!invoke) return;
    try {
      nameEl.classList.add("default");
      ensureDefaultBadge(nameEl);
      await invoke("set_default_device", { deviceId: device.id });
      await new Promise(r => setTimeout(r, 500));
      await loadAudioDevices();
      selectDevice(device.id);
    } catch (err) {
      nameEl.classList.remove("default");
      const badge = nameEl.querySelector(".default-badge");
      if (badge) badge.remove();
      console.error("Failed to set default device:", err);
    }
  });
  header.appendChild(nameEl);
  card.appendChild(header);

  const controls = document.createElement("div");
  controls.className = "card-controls";

  const slider = document.createElement("input");
  slider.type = "range";
  slider.className = "volume-slider";
  slider.min = "0";
  slider.max = "100";
  slider.step = fineAdjustEnabled ? "0.1" : "1";
  slider.value = Math.round(device.volume * 100);

  const throttledSetDeviceVolume = throttle(setDeviceVolume, 150);

  slider.addEventListener("input", (e) => {
    const value = parseFloat(e.target.value) / 100;
    const dev = audioDevices.find(d => d.id === card.dataset.deviceId);
    if (!dev) return;
    const wasMuted = dev.is_muted;
    const targetMuted = value <= 0;
    dev.volume = value;
    updateSliderGradient(e.target);
    if (!dev.permanentMute && wasMuted && !targetMuted) {
      dev.is_muted = false;
      // 保序锚点：解除静音落地后再补发最终值，规避静音锁开启时后端"只许降"钳制竞态
      setDeviceMute(dev.id, false).then(() => {
        setDeviceVolume(dev.id, value);
      });
    }
    updateMuteButton(muteBtn, dev.is_muted, dev.volume, dev.permanentMute);
    if (dev.permanentMute && forceMuteDevices.includes(dev.name)) {
      forceMutePrevVolume[dev.name] = value;
    }
    if (!dev.permanentMute) {
      throttledSetDeviceVolume(dev.id, value);
    }
  });
  slider.addEventListener("change", () => {
    setTimeout(() => {
      slider.blur();
      const dev = audioDevices.find(d => d.id === card.dataset.deviceId);
      if (dev) updateSliderValue(slider, dev.volume);
    }, 100);
  });
  updateSliderGradient(slider);
  controls.appendChild(slider);

  createSliderTooltip(slider);

  const muteBtn = document.createElement("button");
  muteBtn.className = "mute-btn" + (device.is_muted && device.permanentMute ? " muted" : "");
  const muteIcon = document.createElement("span");
  muteIcon.className = "mute-icon";
  muteIcon.innerHTML = device.is_muted ? getMuteIcon() : getVolumeIcon(device.volume);
  muteBtn.appendChild(muteIcon);
  muteBtn.addEventListener("click", () => toggleDeviceMute(device.id));
  controls.appendChild(muteBtn);
  attachTooltip(muteBtn, muteStateText(device.is_muted, device.volume));

  card.appendChild(controls);

  card.addEventListener("click", (e) => {
    if (e.target.tagName !== "INPUT" && e.target.tagName !== "BUTTON") {
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

  const nameEl = card.querySelector(".audio-device-name");
  if (nameEl) {
    const displayName = deviceDisplayName(device.name);
    const firstChild = nameEl.firstChild;
    if (firstChild && firstChild.nodeType === Node.TEXT_NODE) {
      if (firstChild.textContent !== displayName) {
        firstChild.textContent = displayName;
      }
    }
    if (device.is_default) {
      nameEl.classList.add("default");
      ensureDefaultBadge(nameEl);
    } else {
      nameEl.classList.remove("default");
      const badge = nameEl.querySelector(".default-badge");
      if (badge) badge.remove();
    }
  }

  updateSliderValue(card.querySelector(".volume-slider"), device.volume);
  updateMuteButton(card.querySelector(".mute-btn"), device.is_muted, device.volume, device.permanentMute);
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
    audioSessions = (await invoke("get_audio_sessions", { deviceId })).map(s => ({ ...s, permanentMute: muteLockEnabled && !!(s.is_muted && s.volume > 0) }));
    renderAudioSessions();
  } catch (e) {
    if (list.querySelectorAll(".card.session").length === 0) {
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

  list.querySelectorAll(".loading").forEach(el => el.remove());

  reconcileCards(list, ".card.session", "sessionId", audioSessions,
    createAudioSessionCard, updateAudioSessionCard);
}

function createAudioSessionCard(session) {
  const card = document.createElement("div");
  card.className = "card session";
  card.dataset.sessionId = session.id;

  const iconEl = document.createElement("div");
  iconEl.className = "card-icon session-icon";
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
  const iconWrap = document.createElement("div");
  iconWrap.className = "session-icon-wrap";
  iconWrap.appendChild(iconEl);
  card.appendChild(iconWrap);
  window.attachSessionTooltip(iconWrap, session.name);

  const controls = document.createElement("div");
  controls.className = "card-controls session-controls";

  const slider = document.createElement("input");
  slider.type = "range";
  slider.className = "volume-slider session-slider";
  slider.min = "0";
  slider.max = "100";
  slider.step = fineAdjustEnabled ? "0.1" : "1";
  slider.value = Math.round(session.volume * 100);

  const throttledSetSessionVolume = throttle(setSessionVolume, 100);

  slider.addEventListener("input", async (e) => {
    const value = parseFloat(e.target.value) / 100;
    const sess = audioSessions.find(s => s.id === card.dataset.sessionId);
    if (!sess) return;
    const wasMuted = sess.is_muted;
    sess.volume = value;
    updateSliderGradient(e.target);
    if (!sess.permanentMute) {
      const targetMuted = value <= 0;
      if (targetMuted !== wasMuted) {
        sess.is_muted = targetMuted;
        setSessionMute(sess.id, targetMuted);
      }
    }
    updateMuteButton(muteBtn, sess.is_muted, sess.volume, sess.permanentMute);
    throttledSetSessionVolume(sess.id, value);
  });
  slider.addEventListener("change", () => {
    setTimeout(() => slider.blur(), 100);
  });
  updateSliderGradient(slider);
  controls.appendChild(slider);

  createSliderTooltip(slider);

  const muteBtn = document.createElement("button");
  muteBtn.className = "mute-btn" + (session.is_muted && session.permanentMute ? " muted" : "");
  const muteIcon = document.createElement("span");
  muteIcon.className = "mute-icon";
  muteIcon.innerHTML = session.is_muted ? getMuteIcon() : getVolumeIcon(session.volume);
  muteBtn.appendChild(muteIcon);
  muteBtn.addEventListener("click", async () => {
    const sessionId = card.dataset.sessionId;
    const sess = audioSessions.find(s => s.id === sessionId);
    if (!sess) return;
    const targetMuted = !sess.is_muted;
    try {
      await setSessionMute(sessionId, targetMuted);
      sess.is_muted = targetMuted;
      sess.permanentMute = muteLockEnabled && targetMuted;
      updateMuteButton(muteBtn, sess.is_muted, sess.volume, sess.permanentMute);
    } catch (e) {
      console.error("Failed to set session mute:", e);
    }
  });
  controls.appendChild(muteBtn);
  attachTooltip(muteBtn, muteStateText(session.is_muted, session.volume));

  card.appendChild(controls);

  card.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    showSessionContextMenu(e.clientX, e.clientY, session);
  });
  return card;
}

function updateAudioSessionCard(card, session) {
  updateSliderValue(card.querySelector(".volume-slider"), session.volume);
  updateMuteButton(card.querySelector(".mute-btn"), session.is_muted, session.volume, session.permanentMute);
}

let sessionMenuToken = 0;

async function showSessionContextMenu(x, y, session) {
  const token = ++sessionMenuToken;
  hideAllContextMenus();
  const invoke = getInvoke();
  if (!invoke) return;

  const menu = document.createElement("div");
  menu.className = "context-menu";

  const [curOut, inDevices, curIn] = await Promise.all([
    invoke("get_session_device", { pid: session.pid, direction: "output" }).catch(() => null),
    invoke("get_input_devices").catch(() => []),
    invoke("get_session_device", { pid: session.pid, direction: "input" }).catch(() => null),
  ]);
  if (token !== sessionMenuToken) return;

  const visibleOutDevices = audioDevices.filter(d => !hiddenAudioDevices.includes(d.name));
  buildSessionSubmenu(menu, "输出设备", visibleOutDevices, curOut, (deviceId) => {
    setSessionDevice(session, "output", deviceId);
  });
  const visibleInDevices = inDevices.filter(d => !hiddenAudioDevices.includes(d.name));
  buildSessionSubmenu(menu, "输入设备", visibleInDevices, curIn, (deviceId) => {
    setSessionDevice(session, "input", deviceId);
  });

  document.body.appendChild(menu);
  clampMenuPosition(menu, x, y);
  activeAudioMenu = menu;
}

function buildSessionSubmenu(menu, label, devices, currentId, onSelect) {
  const shell = createSubmenuShell(menu, label);

  const defaultId = (devices.find(d => d.is_default) || {}).id;
  const isDefault = !currentId || currentId === defaultId || !devices.some(d => d.id === currentId);

  shell.addItem("系统默认", isDefault, () => onSelect(""));
  for (const dev of devices) {
    shell.addItem(deviceDisplayName(dev.name), !isDefault && dev.id === currentId, () => onSelect(dev.id));
  }

  shell.finish();
}

function buildSpatialSoundSubmenu(menu, device, state) {
  const shell = createSubmenuShell(menu, "空间音效");

  const entries = [{ guid: "", name: "关" }].concat(state.supported);
  for (const format of entries) {
    const checked = (state.current || "") === format.guid;
    shell.addItem(format.name, checked, async () => {
      const invoke = getInvoke();
      if (!invoke) return;
      try {
        await invoke("set_spatial_sound", { deviceId: device.id, formatGuid: format.guid || null });
      } catch (e) {
        showToast("设置空间音效失败：" + e, null, true);
      }
    });
  }

  shell.finish();
}

async function setSessionDevice(session, direction, deviceId) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    await invoke("set_session_device", { pid: session.pid, direction, deviceId });
    loadAudioSessions(selectedDeviceId);
  } catch (e) {
    console.error("Failed to set session device:", e);
    showToast("设置设备失败：" + e, null, true);
  }
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

async function setDeviceMute(deviceId, muted) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    await invoke("set_device_mute", { deviceId, muted });
    const devices = await invoke("get_audio_devices");
    const fresh = devices.find(d => d.id === deviceId);
    const cur = audioDevices.find(d => d.id === deviceId);
    if (fresh && cur) {
      cur.is_muted = fresh.is_muted;
      cur.volume = fresh.volume;
    }
    renderAudioDevices();
  } catch (e) {
    console.error("Failed to set mute:", e);
  }
}

async function toggleDeviceMute(deviceId) {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    const cur = audioDevices.find(d => d.id === deviceId);
    const prevVolume = cur ? cur.volume : null;
    const devName = cur ? cur.name : "";
    const isForceMute = forceMuteDevices.includes(devName);
    const wasLocked = !!(cur && cur.permanentMute);
    if (isForceMute) {
      forceMuteHold[devName] = { muted: !(cur && cur.is_muted), volume: prevVolume };
    }
    await invoke("toggle_device_mute", { deviceId });
    if (wasLocked) {
      if (isForceMute) {
        const intended = forceMutePrevVolume[devName];
        if (intended != null) {
          await setDeviceVolume(deviceId, intended);
        }
      } else {
        await setDeviceVolume(deviceId, prevVolume != null ? prevVolume : 0);
      }
    }
    const devices = await invoke("get_audio_devices");
    const fresh = devices.find(d => d.id === deviceId);
    if (fresh && cur) {
      cur.is_muted = fresh.is_muted;
      if (isForceMute && fresh.is_muted) {
        cur.volume = prevVolume != null ? prevVolume : fresh.volume;
        if (prevVolume != null) forceMutePrevVolume[devName] = prevVolume;
      } else {
        cur.volume = fresh.volume;
      }
      if (fresh.is_muted) buttonMutedDevices.add(deviceId); else buttonMutedDevices.delete(deviceId);
      cur.permanentMute = muteLockEnabled && fresh.is_muted;
      if (isForceMute && !fresh.is_muted) delete forceMutePrevVolume[devName];
    }
    if (isForceMute) delete forceMuteHold[devName];
    renderAudioDevices();
  } catch (e) {
    console.error("Failed to toggle mute:", e);
  }
}

async function setSessionMute(sessionId, muted) {
  const invoke = getInvoke();
  if (!invoke) return;
  await invoke("set_session_mute", { sessionId, muted });
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

function getVolumeIcon(volume) {
  if (!(volume > 0)) return getMuteIcon();
  const pct = Math.floor(volume * 100);
  if (pct <= 32) {
    return `<svg width="16" height="16" viewBox="0 0 1024 1024" fill="currentColor" aria-hidden="true"><path d="M256 298.965333L341.162667 298.666667l199.466666-159.36q31.914667-20.906667 65.493334-2.730667 33.578667 18.133333 34.048 56.32v638.72q0 38.101333-33.536 56.234667-33.578667 18.176-65.536-2.730667L341.674667 726.186667l-85.162667 0.256q-35.370667 0-60.330667-25.002667-25.002667-25.002667-25.514666-60.330667v-256.853333q0-35.328 25.002666-60.288 25.002667-25.002667 60.330667-25.002667zM366.592 384L256 384.298667l0.512 256.810666 110.549333-0.256 187.733334 151.338667-0.426667-559.786667L366.549333 384z m361.386667-0.128a42.666667 42.666667 0 0 0 7.594666 24.32q32.042667 46.208 32.426667 102.442667 0.341333 56.234667-31.061333 102.869333l-1.706667 2.56a42.666667 42.666667 0 1 0 70.826667 47.658667l1.664-2.517334q22.826667-33.877333 34.474666-72.96 11.392-38.229333 11.136-78.165333-0.256-39.978667-12.16-78.037333-12.202667-38.912-35.456-72.490667a42.666667 42.666667 0 0 0-77.781333 24.32z"/></svg>`;
  }
  if (pct <= 65) {
    return `<svg width="16" height="16" viewBox="0 0 1024 1024" fill="currentColor" aria-hidden="true"><path d="M298.538667 298.666667l-85.12 0.298666q-35.370667 0-60.373334 25.002667-25.002667 24.96-25.002666 60.330667v256.810666q0.512 35.328 25.514666 60.330667t60.330667 25.002667l85.162667-0.256 199.466666 158.976q31.957333 20.906667 65.493334 2.730666 33.578667-18.133333 33.578666-56.277333V192.896q-0.512-38.144-34.048-56.277333-33.578667-18.133333-65.536 2.730666L298.538667 298.709333zM213.418667 384.341333L323.968 384l187.733333-151.68 0.512 559.786667-187.733333-151.296-110.592 0.256-0.469333-256.853334z m527.957333-57.770666a42.666667 42.666667 0 1 1 64.512-55.893334q44.373333 51.157333 67.541333 114.346667 22.528 61.354667 22.528 126.933333 0 65.536-22.528 126.933334-23.210667 63.189333-67.584 114.346666a42.666667 42.666667 0 0 1-64.426666-55.893333q34.048-39.338667 51.882666-87.850667 17.28-47.146667 17.28-97.536t-17.28-97.536q-17.834667-48.554667-51.925333-87.850666z m-92.117333 81.706666a42.666667 42.666667 0 1 1 70.144-48.64q23.253333 33.578667 35.413333 72.490667 11.946667 38.058667 12.202667 78.037333 0.256 39.936-11.093334 78.165334-11.690667 39.082667-34.56 72.96l-1.664 2.517333a42.666667 42.666667 0 0 1-70.784-47.701333l1.706667-2.517334q31.402667-46.634667 31.061333-102.826666-0.426667-56.277333-32.426666-102.485334z"/></svg>`;
  }
  return `<svg width="16" height="16" viewBox="0 0 1024 1024" fill="currentColor" aria-hidden="true"><path d="M255.829333 298.666667L170.666667 299.008q-35.328 0-60.330667 25.002667Q85.333333 348.928 85.333333 384.298667v256.810666q0.512 35.328 25.514667 60.330667t60.330667 25.002667l85.162666-0.256 199.466667 158.976q31.957333 20.906667 65.493333 2.730666 33.578667-18.133333 33.578667-56.277333V192.896q-0.512-38.144-34.090667-56.277333-33.578667-18.133333-65.493333 2.730666L255.829333 298.709333z m655.658667 43.946666q-27.733333-83.968-81.066667-154.965333a42.666667 42.666667 0 1 0-68.266666 51.2q44.928 59.818667 68.266666 130.56 22.784 68.992 22.912 141.866667 0.085333 72.874667-22.528 141.994666-23.125333 70.784-67.882666 130.688l-0.853334 1.066667a42.666667 42.666667 0 1 0 68.394667 51.072l0.853333-1.066667q53.12-71.168 80.64-155.264 26.837333-82.090667 26.709334-168.618666-0.128-86.485333-27.178667-168.533334zM170.666667 384.298667L281.258667 384l187.733333-151.68 0.512 559.786667-187.733333-151.296-110.592 0.256L170.666667 384.256z m490.666666-85.76a42.666667 42.666667 0 0 0 10.453334 27.989333q34.090667 39.296 51.882666 87.850667 17.322667 47.146667 17.322667 97.536t-17.322667 97.536q-17.792 48.512-51.882666 87.850666a42.666667 42.666667 0 1 0 64.426666 55.893334q44.373333-51.157333 67.584-114.346667 22.528-61.397333 22.528-126.933333 0-65.578667-22.528-126.933334-23.168-63.189333-67.498666-114.346666a42.666667 42.666667 0 0 0-74.965334 27.946666z m-66.218666 109.696a42.666667 42.666667 0 1 1 70.144-48.64q23.296 33.578667 35.413333 72.490666 11.946667 38.058667 12.202667 78.037334 0.298667 39.936-11.093334 78.165333-11.690667 39.082667-34.56 72.96l-1.664 2.517333a42.666667 42.666667 0 0 1-70.784-47.701333l1.706667-2.517333q31.445333-46.634667 31.061333-102.826667-0.384-56.277333-32.426666-102.485333z"/></svg>`;
}

function getMuteIcon() {
  return `<svg width="16" height="16" viewBox="0 0 1024 1024" fill="currentColor" aria-hidden="true"><path d="M170.666667 298.666667l85.162666-0.213334L455.253333 139.093333q31.914667-20.906667 65.493334-2.730666 33.578667 18.133333 34.048 56.32v638.677333q0 38.144-33.536 56.32-33.578667 18.133333-65.493334-2.773333l-199.466666-158.976-85.162667 0.256q-35.370667 0-60.330667-25.002667-25.002667-25.002667-25.514666-60.330667V384q0-35.328 25.002666-60.330667Q135.338667 298.709333 170.666667 298.666667z m110.592 85.12L170.666667 384l0.512 256.853333 110.592-0.256 187.733333 151.338667-0.512-559.829333-187.733333 151.68z m403.541333-42.709334a42.666667 42.666667 0 1 0-60.330667 60.330667l90.496 90.496-90.453333 90.538667a42.666667 42.666667 0 0 0 60.288 60.330666l90.538667-90.538666 90.496 90.538666a42.666667 42.666667 0 0 0 60.330666-60.330666l-90.496-90.538667 90.496-90.496a42.666667 42.666667 0 0 0-60.330666-60.330667l-90.496 90.496-90.538667-90.496z"/></svg>`;
}

function stringToColor(str) {
  let hash = 0;
  for (let i = 0; i < str.length; i++) {
    hash = str.charCodeAt(i) + ((hash << 5) - hash);
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue}, 60%, 50%)`;
}
