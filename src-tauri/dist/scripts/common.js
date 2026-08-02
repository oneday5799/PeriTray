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
