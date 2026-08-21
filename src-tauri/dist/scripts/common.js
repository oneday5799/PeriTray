// Shared constants and utilities for popup and settings pages
const { invoke } = window.__TAURI__.core;

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

window.CATEGORIES = [
  { key: "Audio", label: "音频设备", subtitle: "扬声器、耳机等音频设备", icon: "🔊" },
  { key: "Usb", label: "输入设备", subtitle: "键盘、鼠标等USB设备", icon: "⌨️" },
  { key: "Battery", label: "电池", subtitle: "电池设备", icon: "🔋" },
  { key: "Monitor", label: "显示器", subtitle: "显示器设备", icon: "🖥️" },
  { key: "Other", label: "其他设备", subtitle: "未归类的设备", icon: "📦" },
];

// 给元素挂载与「设备快捷键共享切换」一致的样式 tooltip（替代原生 title 提示）
window.attachTooltip = function (el, text, position) {
  if (!el || !text || el.dataset.tooltipSetup) return;
  el.dataset.tooltipSetup = "1";
  el.classList.add("tooltip-host");
  const tip = document.createElement("span");
  tip.className = "tooltip-content";
  if (position === "below") tip.classList.add("tooltip-content--below");
  else if (position === "end") tip.classList.add("tooltip-content--end");
  else if (position === "start") tip.classList.add("tooltip-content--start");
  tip.textContent = text;
  el.appendChild(tip);
};

window.getInvoke = function () {
  return window.__TAURI__ && window.__TAURI__.core
    ? window.__TAURI__.core.invoke
    : null;
};

// ── 主题（共享：设置页 + 主窗口） ─────────────────────
let themeMode = "follow_system";

window.applyThemeMode = function (mode) {
  themeMode = mode || "follow_system";
  const html = document.documentElement;
  const isDark = themeMode === "dark" ||
    (themeMode === "follow_system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
  html.setAttribute("data-theme", isDark ? "dark" : "light");

  const invoke = getInvoke();
  if (invoke) {
    const theme = themeMode === "follow_system" ? "system" : isDark ? "dark" : "light";
    invoke("set_window_theme", { theme }).catch(() => {});
  }
};

window.initTheme = async function () {
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    const config = await invoke("get_config");
    applyThemeMode(config.theme_mode || "follow_system");
  } catch (e) {
    console.error("Failed to init theme:", e);
  }
};

// 跟随系统时实时响应系统主题切换
window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
  if (themeMode === "follow_system") applyThemeMode("follow_system");
});

// config-changed: 设置页切主题时，主窗口/设置页实时同步
window.__TAURI__.event.listen("config-changed", () => {
  initTheme();
});

// ── 窗口材质（共享：设置页 + 主窗口） ─────────────────
window.applyMaterialMode = function (material) {
  const html = document.documentElement;
  if (material && material !== "default") html.setAttribute("data-material", material);
  else html.removeAttribute("data-material");
};

(async () => {
  try {
    const cfg = await invoke("get_config");
    applyMaterialMode(cfg.window_material);
  } catch (e) {}
})();

// 材质变更时由 Rust 发出 material-changed；设置页切换过程中跳过（防闪烁时序由 settings.js 控制）
window.__TAURI__.event.listen("material-changed", (e) => {
  if (!window.__materialChangeInProgress) applyMaterialMode(e.payload);
});

window.getDisplayName = function (dev, deviceNames) {
  return deviceNames[dev.name] || dev.name;
};

// 简化设备名称：仅保留括号内的内容，如 "耳机 (小爱音箱-9205)" -> "小爱音箱-9205"
window.simplifyDeviceName = function (name) {
  if (!name) return name;
  const open = name.indexOf("(");
  const close = name.lastIndexOf(")");
  if (open >= 0 && close > open) {
    const inner = name.slice(open + 1, close).trim();
    if (inner) return inner;
  }
  return name;
};

// ── 应用名 tooltip（页面内，超出边界自动避让） ────────────
let sessionTipTimer = null;
let sessionTip = null;

function getSessionTip() {
  if (!sessionTip) {
    sessionTip = document.createElement("div");
    sessionTip.className = "session-tip";
    document.body.appendChild(sessionTip);
  }
  return sessionTip;
}

function showSessionTip(el, text) {
  const tip = getSessionTip();
  tip.textContent = text;
  tip.style.visibility = "hidden";
  tip.style.display = "block";
  const tw = tip.offsetWidth;
  const th = tip.offsetHeight;
  const rect = el.getBoundingClientRect();
  let left = rect.left + rect.width / 2 - tw / 2;
  left = Math.max(4, Math.min(left, window.innerWidth - tw - 4));
  let top = rect.top - th - 8;
  if (top < 4) top = rect.bottom + 8;
  if (top + th > window.innerHeight - 4) top = window.innerHeight - th - 4;
  tip.style.left = left + "px";
  tip.style.top = top + "px";
  tip.style.visibility = "visible";
}

window.attachSessionTooltip = function (el, text) {
  el.addEventListener("pointerenter", () => {
    if (sessionTipTimer) clearTimeout(sessionTipTimer);
    sessionTipTimer = setTimeout(() => showSessionTip(el, text), 800);
  });
  el.addEventListener("pointerleave", () => hideSessionTip());
};

// 供页面级事件委托复用（如设置页 [data-tip] 提示）
window.showSessionTip = showSessionTip;
window.hideSessionTip = function () {
  if (sessionTipTimer) {
    clearTimeout(sessionTipTimer);
    sessionTipTimer = null;
  }
  if (sessionTip) sessionTip.style.display = "none";
};

// ── 右键菜单共享工具 ─────────────────────────────────────

const contextMenuHolders = [];

window.registerContextMenu = function (holderRef) {
  contextMenuHolders.push(holderRef);
};

window.clampMenuPosition = function (menu, x, y) {
  const menuW = menu.offsetWidth;
  const menuH = menu.offsetHeight;
  let posX = x;
  let posY = y;
  if (x + menuW > window.innerWidth) posX = x - menuW;
  if (y + menuH > window.innerHeight) posY = y - menuH;
  if (posX < 0) posX = 0;
  if (posY < 0) posY = 0;
  menu.style.left = posX + "px";
  menu.style.top = posY + "px";
};

window.hideAllContextMenus = function () {
  for (const holder of contextMenuHolders) {
    if (holder.menu) {
      holder.menu.remove();
      holder.menu = null;
    }
  }
};

document.addEventListener("click", hideAllContextMenus);

// ── 重命名对话框 ─────────────────────────────────────────

window.showRenameDialog = function ({ deviceName, displayName, nameSource, onUpdate, onRender }) {
  const input = document.createElement("input");
  input.type = "text";
  input.className = "dialog-input";
  input.value = displayName;
  input.placeholder = "输入新名称";

  const isRenamed = nameSource !== undefined;

  const buttons = [];

  buttons.push({
    text: "恢复默认",
    className: "danger",
    onClick: async () => {
      const invoke = getInvoke();
      if (invoke) {
        await invoke("rename_device", { original: deviceName, newName: "" });
        const config = await invoke("get_config");
        onUpdate(config.device_names || {});
        onRender();
      }
      closeDialog(overlay);
    },
  });

  buttons.push({
    text: "取消",
    className: "cancel",
    onClick: () => closeDialog(overlay),
  });

  buttons.push({
    text: "确定",
    className: "confirm",
    onClick: async () => {
      const newName = input.value.trim();
      const invoke = getInvoke();
      if (invoke) {
        await invoke("rename_device", { original: deviceName, newName });
        const config = await invoke("get_config");
        onUpdate(config.device_names || {});
        onRender();
      }
      closeDialog(overlay);
    },
  });

  const overlay = createDialog({
    title: "重命名设备",
    content: [input],
    buttons,
  });

  const restoreBtn = overlay.querySelector(".dialog-btn.danger");
  if (restoreBtn) restoreBtn.disabled = !isRenamed;

  input.focus();
  input.select();

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") overlay.querySelector(".dialog-btn.confirm")?.click();
  });
};

function updateSliderGradient(slider) {
  const value = slider.value;
  const percentage = ((value - slider.min) / (slider.max - slider.min)) * 100;
  slider.style.setProperty('--track-color', `linear-gradient(to right, #0078d7 0%, #0078d7 ${percentage}%, var(--slider-track, #e0e0e0) ${percentage}%, var(--slider-track, #e0e0e0) 100%)`);
}

// ── 快捷键录制（共享工具） ────────────────────────────────

window.shortcutCodeMap = {
  "Space": { display: "Space", key: "Space" },
  "Backspace": { display: "Backspace", key: "Backspace" },
  "Delete": { display: "Delete", key: "Delete" },
  "Tab": { display: "Tab", key: "Tab" },
  "CapsLock": { display: "CapsLock", key: "CapsLock" },
  "Escape": { display: "Escape", key: "Escape" },
  "Insert": { display: "Insert", key: "Insert" },
  "Home": { display: "Home", key: "Home" },
  "End": { display: "End", key: "End" },
  "PageUp": { display: "PageUp", key: "PageUp" },
  "PageDown": { display: "PageDown", key: "PageDown" },
  "ArrowUp": { display: "↑", key: "ArrowUp" },
  "ArrowDown": { display: "↓", key: "ArrowDown" },
  "ArrowLeft": { display: "←", key: "ArrowLeft" },
  "ArrowRight": { display: "→", key: "ArrowRight" },
  "PrintScreen": { display: "PrtSc", key: "PrintScreen" },
  "ScrollLock": { display: "ScrLk", key: "ScrollLock" },
  "Pause": { display: "Pause", key: "Pause" },
  "NumLock": { display: "NumLock", key: "NumLock" },
  "Numpad0": { display: "Num0", key: "Numpad0" },
  "Numpad1": { display: "Num1", key: "Numpad1" },
  "Numpad2": { display: "Num2", key: "Numpad2" },
  "Numpad3": { display: "Num3", key: "Numpad3" },
  "Numpad4": { display: "Num4", key: "Numpad4" },
  "Numpad5": { display: "Num5", key: "Numpad5" },
  "Numpad6": { display: "Num6", key: "Numpad6" },
  "Numpad7": { display: "Num7", key: "Numpad7" },
  "Numpad8": { display: "Num8", key: "Numpad8" },
  "Numpad9": { display: "Num9", key: "Numpad9" },
  "NumpadAdd": { display: "Num+", key: "NumpadAdd" },
  "NumpadSubtract": { display: "Num-", key: "NumpadSubtract" },
  "NumpadMultiply": { display: "Num*", key: "NumpadMultiply" },
  "NumpadDivide": { display: "Num/", key: "NumpadDivide" },
  "NumpadDecimal": { display: "Num.", key: "NumpadDecimal" },
  "NumpadEnter": { display: "NumEnter", key: "NumpadEnter" },
  "MediaPlayPause": { display: "MediaPlayPause", key: "MediaPlayPause" },
  "MediaStop": { display: "MediaStop", key: "MediaStop" },
  "MediaNextTrack": { display: "MediaNextTrack", key: "MediaNextTrack" },
  "MediaPrevTrack": { display: "MediaPrevTrack", key: "MediaPrevTrack" },
  "VolumeUp": { display: "VolumeUp", key: "VolumeUp" },
  "VolumeDown": { display: "VolumeDown", key: "VolumeDown" },
  "VolumeMute": { display: "VolumeMute", key: "VolumeMute" },
  "Semicolon": { display: ";", key: "Semicolon" },
  "Equal": { display: "=", key: "Equal" },
  "Comma": { display: ",", key: "Comma" },
  "Period": { display: ".", key: "Period" },
  "Slash": { display: "/", key: "Slash" },
  "Backquote": { display: "`", key: "Backquote" },
  "Backslash": { display: "\\", key: "Backslash" },
  "BracketLeft": { display: "[", key: "BracketLeft" },
  "BracketRight": { display: "]", key: "BracketRight" },
  "Minus": { display: "-", key: "Minus" },
  "Quote": { display: "'", key: "Quote" },
  "Enter": { display: "Enter", key: "Enter" },
};

window.shortcutReverseCodeMap = {};
for (const v of Object.values(window.shortcutCodeMap)) {
  window.shortcutReverseCodeMap[v.key] = v.display;
}

window.shortcutJoinSaved = function (saved) {
  if (!saved) return "";
  return saved.split("+").map(p => {
    if (window.shortcutReverseCodeMap[p]) return window.shortcutReverseCodeMap[p];
    if (p.length === 4 && p.startsWith("Key")) return p[3];
    if (p.length === 6 && p.startsWith("Digit")) return p[5];
    return p;
  }).join("+");
};

// 绑定快捷键输入框录制行为。input/clearBtn 为 DOM 元素，getSavedKey() 返回当前保存的原始快捷键（含 Super）
// onSaved(display, shortcut) 在录制成功或点击清除时回调（清除时 shortcut 为空串）；onError(msg) 在失败时回调
const shortcutRecorders = new Set();
let shortcutRecordListenerReady = false;
function ensureShortcutRecordListener() {
  if (shortcutRecordListenerReady) return;
  shortcutRecordListenerReady = true;
  // 已注册为全局快捷键的组合键，其按键事件可能被系统吞掉而收不到 keydown，
  // 由后端在录制期间直接上报按下的组合键。
  window.__TAURI__.event.listen("shortcut-recorded", (event) => {
    const key = event.payload;
    if (!key) return;
    for (const rec of shortcutRecorders) rec.recordFromBackend(key);
  });
}

window.bindShortcutRecorder = function (input, clearBtn, getSavedKey, onSaved, onError) {
  let recording = false;
  let keys = new Set();

  function setRecordingFlag(on) {
    try {
      window.__TAURI__.core.invoke("set_shortcut_recording", { recording: on }).catch(() => {});
    } catch (_) {}
  }

  function resetRecording() {
    recording = false;
    keys.clear();
    input.classList.remove("recording");
    input.placeholder = "点击录制快捷键";
    setRecordingFlag(false);
  }

  function restoreSaved() {
    resetRecording();
    const savedKey = getSavedKey();
    input.value = savedKey ? window.shortcutJoinSaved(savedKey).replace("Super", "Win") : "";
    if (clearBtn) clearBtn.style.display = savedKey ? "" : "none";
  }

  input.addEventListener("click", () => {
    if (recording) return;
    recording = true;
    keys.clear();
    input.value = "";
    input.classList.add("recording");
    input.placeholder = "请按下组合键...";
    setRecordingFlag(true);
  });

  input.addEventListener("blur", () => {
    resetRecording();
  });

  input.addEventListener("keydown", (e) => {
    if (!recording) return;
    e.preventDefault();
    e.stopPropagation();

    if (e.key === "Escape") {
      restoreSaved();
      return;
    }

    keys.clear();
    if (e.ctrlKey) keys.add({ display: "Ctrl", key: "Ctrl" });
    if (e.shiftKey) keys.add({ display: "Shift", key: "Shift" });
    if (e.altKey) keys.add({ display: "Alt", key: "Alt" });
    if (e.metaKey) keys.add({ display: "Win", key: "Super" });

    const code = e.code;
    if (code === "ControlLeft" || code === "ControlRight" ||
        code === "ShiftLeft" || code === "ShiftRight" ||
        code === "AltLeft" || code === "AltRight" ||
        code === "MetaLeft" || code === "MetaRight") {
      const preview = [...keys].map(k => k.display).join("+");
      input.value = preview;
      return;
    }

    if (code.startsWith("Numpad") && /\d/.test(code[6]) && code.length === 7) {
      keys.add({ display: "Num" + code[6], key: code });
    } else if (window.shortcutCodeMap[code]) {
      const entry = window.shortcutCodeMap[code];
      keys.add({ display: entry.display, key: entry.key });
    } else if (code.startsWith("F") && code.length >= 2 && code.length <= 3) {
      keys.add({ display: code, key: code });
    } else if (code.startsWith("Digit") && code.length === 6) {
      keys.add({ display: code[5], key: code });
    } else if (code.startsWith("Key") && code.length === 4) {
      keys.add({ display: code[3], key: code });
    } else {
      restoreSaved();
      if (onError) onError("暂不支持该快捷键。");
      return;
    }

    const display = [...keys].map(k => k.display).join("+");
    const shortcut = [...keys].map(k => k.key).join("+");
    if (display) {
      recording = false;
      input.value = display;
      input.classList.remove("recording");
      input.placeholder = "点击录制快捷键";
      // 延迟释放录制标志，确保本次按键的全局快捷键分发已被抑制
      setTimeout(() => setRecordingFlag(false), 300);
      if (onSaved) onSaved(display, shortcut.replace("Win", "Super"));
    }
  });

  if (clearBtn) {
    clearBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (onSaved) onSaved("", "");
    });
  }

  function recordFromBackend(canonicalKey) {
    if (!recording) return;
    if (!canonicalKey) return;
    const display = window.shortcutJoinSaved(canonicalKey);
    if (!display) return;
    recording = false;
    keys.clear();
    input.value = display;
    input.classList.remove("recording");
    input.placeholder = "点击录制快捷键";
    // 延迟释放录制标志，确保本次按键的全局快捷键分发已被抑制
    setTimeout(() => setRecordingFlag(false), 300);
    if (onSaved) onSaved(display, canonicalKey);
  }

  const self = { recordFromBackend };
  shortcutRecorders.add(self);
  ensureShortcutRecordListener();

  restoreSaved();
  return { restore: restoreSaved };
};

// ── Dialog ──────────────────────────────────────────────

window.createDialog = function ({ title, content = [], buttons = [] }) {
  const overlay = document.createElement("div");
  overlay.className = "dialog-overlay";

  const dialog = document.createElement("div");
  dialog.className = "rename-dialog";

  const contentEl = document.createElement("div");
  contentEl.className = "dialog-content";

  const titleEl = document.createElement("div");
  titleEl.className = "dialog-title";
  titleEl.textContent = title;
  contentEl.appendChild(titleEl);

  for (const el of content) {
    contentEl.appendChild(el);
  }
  dialog.appendChild(contentEl);

  if (buttons.length > 0) {
    const buttonsEl = document.createElement("div");
    buttonsEl.className = "dialog-buttons";
    for (const btn of buttons) {
      const btnEl = document.createElement("button");
      btnEl.className = `dialog-btn ${btn.className || ""}`;
      btnEl.textContent = btn.text;
      btnEl.addEventListener("click", btn.onClick);
      buttonsEl.appendChild(btnEl);
    }
    dialog.appendChild(buttonsEl);
  }

  overlay.appendChild(dialog);
  document.body.appendChild(overlay);

  overlay.addEventListener("keydown", (e) => {
    if (e.key === "Escape") overlay.remove();
  });

  return overlay;
};

window.closeDialog = function (overlay) {
  if (overlay && overlay.parentNode) {
    overlay.remove();
  }
};

// ── Toast 通知 ──────────────────────────────────────────

window.showToast = function (msg, onClick, isError) {
  let el = document.querySelector(".toast");
  if (!el) {
    el = document.createElement("div");
    el.className = "toast";
    document.body.appendChild(el);
  }
  el.innerHTML = msg;
  el.classList.toggle("error", !!isError);
  el.classList.add("show");
  el.style.cursor = onClick ? "pointer" : "default";
  el.onclick = onClick || null;
  clearTimeout(el._timer);
  el._timer = setTimeout(() => {
    el.classList.remove("show");
    el.classList.remove("error");
    el.onclick = null;
    el.style.cursor = "default";
  }, 5000);
};

// ── 启动时更新检测（全局监听） ─────────────────────────
window.__TAURI__.event.listen("update-available", (event) => {
  const info = event.payload;
  window.showToast(
    `发现新版本 ${info.latest_version}（当前 ${info.current_version}）<br>点击前往下载`,
    () => window.__TAURI__.core.invoke("open_url", { url: info.release_url })
  );
});
