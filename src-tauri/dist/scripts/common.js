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

window.getInvoke = function () {
  return window.__TAURI__ && window.__TAURI__.core
    ? window.__TAURI__.core.invoke
    : null;
};

window.getDisplayName = function (dev, deviceNames) {
  return deviceNames[dev.name] || dev.name;
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

  if (isRenamed) {
    buttons.push({
      text: "恢复默认",
      className: "restore",
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
  }

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

  input.focus();
  input.select();

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") overlay.querySelector(".dialog-btn.confirm")?.click();
  });
};

function updateSliderGradient(slider) {
  const value = slider.value;
  const percentage = ((value - slider.min) / (slider.max - slider.min)) * 100;
  slider.style.setProperty('--track-color', `linear-gradient(to right, #0078d7 0%, #0078d7 ${percentage}%, #e0e0e0 ${percentage}%, #e0e0e0 100%)`);
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
  return saved.split("+").map(p => window.shortcutReverseCodeMap[p] || p).join("+");
};

// 绑定快捷键输入框录制行为。input/clearBtn 为 DOM 元素，getSavedKey() 返回当前保存的原始快捷键（含 Super）
// onSaved(display, shortcut) 在录制成功或点击清除时回调（清除时 shortcut 为空串）；onError(msg) 在失败时回调
window.bindShortcutRecorder = function (input, clearBtn, getSavedKey, onSaved, onError) {
  let recording = false;
  let keys = new Set();

  function resetRecording() {
    recording = false;
    keys.clear();
    input.classList.remove("recording");
    input.placeholder = "点击录制快捷键";
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
      restoreSaved();
      if (onError) onError("暂不支持该快捷键。");
      return;
    }
    if (window.shortcutCodeMap[code]) {
      const entry = window.shortcutCodeMap[code];
      keys.add({ display: entry.display, key: entry.key });
    } else if (code.startsWith("F") && code.length >= 2 && code.length <= 3) {
      keys.add({ display: code, key: code });
    } else if (code.startsWith("Digit") && code.length === 6) {
      keys.add({ display: code, key: code });
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
      if (onSaved) onSaved(display, shortcut.replace("Win", "Super"));
    }
  });

  if (clearBtn) {
    clearBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      if (onSaved) onSaved("", "");
    });
  }

  restoreSaved();
  return { restore: restoreSaved };
};

// ── Dialog ──────────────────────────────────────────────

window.createDialog = function ({ title, content = [], buttons = [] }) {
  const overlay = document.createElement("div");
  overlay.className = "dialog-overlay";

  const dialog = document.createElement("div");
  dialog.className = "rename-dialog";

  const titleEl = document.createElement("div");
  titleEl.className = "dialog-title";
  titleEl.textContent = title;
  dialog.appendChild(titleEl);

  for (const el of content) {
    dialog.appendChild(el);
  }

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

window.showToast = function (msg, onClick) {
  let el = document.querySelector(".toast");
  if (!el) {
    el = document.createElement("div");
    el.className = "toast";
    document.body.appendChild(el);
  }
  el.innerHTML = msg;
  el.classList.add("show");
  el.style.cursor = onClick ? "pointer" : "default";
  el.onclick = onClick || null;
  clearTimeout(el._timer);
  el._timer = setTimeout(() => {
    el.classList.remove("show");
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
