/* popup.js — 主窗口·入口：导航指示条/tab 切换动画/按钮与 focus 刷新/托盘联动
 * 加载序 4/4（common → popup-audio → popup-devices → 本文件）
 * 提供：switchToTab()（托盘跳转 tab 用）；focus 处理器写入 popup-devices.js 的域缓存变量（同页全局作用域）
 * 依赖：common.js(initTheme/getInvoke) / popup-audio.js(loadAudioDevices/loadAudioSessions) /
 *       popup-devices.js(loadDevices/renderDevices)
 */
document.getElementById("btn-refresh").addEventListener("click", async () => {
  const activeTab = document.querySelector(".win-nav-item.active");
  if (!activeTab) return;
  const tabName = activeTab.dataset.tab;
  // 立即反馈：设备页强制现查耗时可达数秒，先提示避免误以为点击未生效
  if (tabName === "devices") {
    showToast("正在刷新...");
    try {
      // true = 强制现查 2.4G 设备连接状态与电量（绕过缓存）
      await loadDevices(true);
      showToast("已刷新");
    } catch (e) {
      console.error("Refresh failed:", e);
      showToast("刷新失败", null, true);
    }
  } else if (tabName === "volume") {
    showToast("正在刷新...");
    try {
      await loadAudioDevices();
      if (selectedDeviceId) {
        await loadAudioSessions(selectedDeviceId);
      }
      showToast("已刷新");
    } catch (e) {
      console.error("Refresh failed:", e);
      showToast("刷新失败", null, true);
    }
  }
});

document.getElementById("btn-settings").addEventListener("click", async () => {
  const invoke = getInvoke();
  if (invoke) {
    try { await invoke("open_settings"); } catch (e) { console.error(e); }
  }
});

let lastFocusRefresh = 0;

window.addEventListener("focus", async () => {
  const now = Date.now();
  if (now - lastFocusRefresh < 2000) return;
  const invoke = getInvoke();
  if (!invoke) return;
  try {
    const volumeTab = document.getElementById("tab-volume");
    const deviceTab = document.getElementById("tab-devices");
    const scrollTop = (volumeTab.style.display !== "none" ? volumeTab : deviceTab).scrollTop;

    const cfg = await invoke("get_config");
    hiddenDevices = cfg.hidden_devices || [];
    hiddenGroups = cfg.hidden_groups || [];
    deviceNames = cfg.device_names || {};
    deviceGroups = cfg.device_groups || {};
    useSystemBt = cfg.use_system_bt || false;
    trayDevices = cfg.tray_devices || [];
    allDevices = await invoke("get_devices");
    renderDevices();
    if (volumeTab.style.display !== "none") {
      await loadAudioDevices();
      if (selectedDeviceId) {
        await loadAudioSessions(selectedDeviceId);
      }
    }
    (volumeTab.style.display !== "none" ? volumeTab : deviceTab).scrollTop = scrollTop;
    lastFocusRefresh = Date.now();
  } catch (e) {
    console.error("Failed to refresh on focus:", e);
  }
});

if (window.__TAURI__) {
  initTheme();
  loadDevices();
} else {
  window.addEventListener("DOMContentLoaded", () => {
    setTimeout(loadDevices, 100);
  });
}

// WinUI NavigationView top-pane indicator
const INDICATOR_SIZE = 16;
const EASE_OUT = "cubic-bezier(0.1, 0.9, 0.2, 1)";
let prevNavItem = null;

function getIndicatorLayoutPosition(element) {
  let left = 0;
  let top = 0;
  let offsetNode = element;
  while (offsetNode) {
    left += offsetNode.offsetLeft || 0;
    top += offsetNode.offsetTop || 0;
    offsetNode = offsetNode.offsetParent;
  }
  let parent = element ? element.parentElement : null;
  while (parent) {
    left -= parent.scrollLeft || 0;
    top -= parent.scrollTop || 0;
    parent = parent.parentElement;
  }
  return { left, top };
}

function getIndicatorItemRect(element, track) {
  const trackPos = getIndicatorLayoutPosition(track);
  const itemPos = getIndicatorLayoutPosition(element);
  const left = itemPos.left - trackPos.left;
  return { left, right: left + element.offsetWidth };
}

function setIndicatorVisibility(track, targetRect, sourceRect) {
  const width = track.offsetWidth || 1;
  const clamp = (rect) => {
    if (!rect) return null;
    const start = Math.max(0, Math.min(width, rect.left));
    const end = Math.max(start, Math.min(width, rect.right));
    return end > start ? { start, end } : null;
  };
  const target = clamp(targetRect);
  const source = clamp(sourceRect);
  if (!target) {
    track.style.clipPath = "inset(0 100% 0 0)";
    return;
  }
  const start = source ? Math.min(target.start, source.start) : target.start;
  const end = source ? Math.max(target.end, source.end) : target.end;
  track.style.clipPath = `inset(0px ${Math.max(0, width - end)}px 0px ${start}px)`;
}

function readIndicatorX(indicator, fallback) {
  const transform = getComputedStyle(indicator).transform;
  if (transform && transform !== "none") {
    const match = transform.match(/matrix\((.+)\)/);
    if (match) {
      const parts = match[1].split(",").map(s => parseFloat(s));
      if (parts.length >= 5 && !isNaN(parts[4])) return parts[4];
    }
  }
  return fallback;
}

function updateTopNavIndicator(animate) {
  const track = document.getElementById("nav-indicator-track");
  const indicator = document.getElementById("nav-indicator");
  const active = document.querySelector(".win-nav-item.active");
  if (!track || !indicator || !active) return;
  const rect = getIndicatorItemRect(active, track);
  const newX = rect.left + (rect.right - rect.left) / 2 - (INDICATOR_SIZE / 2);

  if (!animate) {
    indicator.getAnimations().forEach(a => a.cancel());
    indicator.style.transition = "none";
    indicator.style.transform = `translateX(${newX}px)`;
    indicator.style.width = INDICATOR_SIZE + "px";
    setIndicatorVisibility(track, rect, null);
    prevNavItem = active;
    return;
  }

  const oldX = readIndicatorX(indicator, newX);
  const dist = Math.abs(newX - oldX);
  if (dist < 1) {
    indicator.style.transform = `translateX(${newX}px)`;
    indicator.style.width = INDICATOR_SIZE + "px";
    prevNavItem = active;
    return;
  }

  const sourceRect = prevNavItem ? getIndicatorItemRect(prevNavItem, track) : null;
  setIndicatorVisibility(track, rect, sourceRect);
  const edge = Math.min(oldX, newX);
  const keyframes = [
    { transform: `translateX(${oldX}px)`, width: INDICATOR_SIZE + "px", offset: 0, easing: "cubic-bezier(0.9, 0.1, 1, 0.2)" },
    { transform: `translateX(${edge}px)`, width: (dist + INDICATOR_SIZE) + "px", offset: 0.333, easing: EASE_OUT },
    { transform: `translateX(${newX}px)`, width: INDICATOR_SIZE + "px", offset: 1 }
  ];
  indicator.getAnimations().forEach(a => a.cancel());
  const anim = indicator.animate(keyframes, { duration: 600, fill: "forwards" });
  anim.onfinish = () => {
    indicator.style.transform = `translateX(${newX}px)`;
    indicator.style.width = INDICATOR_SIZE + "px";
    setIndicatorVisibility(track, rect, null);
  };
  prevNavItem = active;
}

// ── 标签页切换（WinUI 滑动过渡风格） ─────────────────────────
const tabOrder = ["devices", "volume"];
let currentTabIndex = 0;
let suppressNextSwitchAnimation = false;
// 切换代际：连续快速切换时使上一次的收尾定时器失效，防止旧定时器隐藏正在进入的标签
let tabAnimToken = 0;

function applyTabContentDisplay(tabName) {
  document.getElementById("tab-devices").style.display = tabName === "devices" ? "block" : "none";
  document.getElementById("tab-volume").style.display = tabName === "volume" ? "block" : "none";
}

function animateTabSwitch(oldIndex, newIndex) {
  const token = ++tabAnimToken;
  const oldTab = tabOrder[oldIndex];
  const newTab = tabOrder[newIndex];
  const oldContent = document.getElementById("tab-" + oldTab);
  const newContent = document.getElementById("tab-" + newTab);
  const forward = newIndex > oldIndex;
  const leaveClass = forward ? "slide-leave-left" : "slide-leave-right";
  const enterClass = forward ? "slide-enter-right" : "slide-enter-left";
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  document.querySelectorAll(".tab-content").forEach(c => {
    c.classList.remove("slide-leave-left", "slide-enter-right", "slide-leave-right", "slide-enter-left");
    c.style.zIndex = "";
  });

  if (reduced) {
    applyTabContentDisplay(newTab);
    return;
  }

  oldContent.style.display = "block";
  newContent.style.display = "block";
  newContent.style.zIndex = "2";
  oldContent.classList.add(leaveClass);
  newContent.classList.add(enterClass);

  setTimeout(() => {
    if (token !== tabAnimToken) return;
    oldContent.style.display = "none";
    oldContent.classList.remove(leaveClass);
    oldContent.style.zIndex = "";
    newContent.style.zIndex = "";
  }, 150);

  setTimeout(() => {
    if (token !== tabAnimToken) return;
    newContent.classList.remove(enterClass);
  }, 450);
}

document.querySelectorAll(".win-nav-item").forEach(tab => {
  tab.addEventListener("click", async () => {
    const newIndex = tabOrder.indexOf(tab.dataset.tab);
    // 抑制标志一次性消费：托盘重开时若目标==当前 tab 会提前早退，
    // 标志必须在此之前复位，否则残留并吞掉下一次手动切换的动画
    const suppressed = suppressNextSwitchAnimation;
    suppressNextSwitchAnimation = false;
    if (newIndex === currentTabIndex) return;
    const oldIndex = currentTabIndex;
    // 同步更新，避免 await 加载期间再次点击时以旧值重复触发切换
    currentTabIndex = newIndex;
    document.querySelectorAll(".win-nav-item").forEach(t => {
      t.classList.remove("active");
      t.setAttribute("aria-selected", "false");
    });
    tab.classList.add("active");
    tab.setAttribute("aria-selected", "true");
    const tabName = tab.dataset.tab;
    if (suppressed) {
      applyTabContentDisplay(tabName);
    } else {
      animateTabSwitch(oldIndex, newIndex);
    }
    updateTopNavIndicator(true);
    if (tabName === "volume") {
      await loadAudioDevices();
      if (selectedDeviceId) {
        await loadAudioSessions(selectedDeviceId);
      }
    }
  });
});

function switchToTab(tabName) {
  const target = document.querySelector(`.win-nav-item[data-tab="${tabName}"]`);
  if (!target) return;
  suppressNextSwitchAnimation = true;
  target.click();
}

window.addEventListener("DOMContentLoaded", () => {
  setTimeout(() => updateTopNavIndicator(false), 100);
});
setTimeout(() => updateTopNavIndicator(false), 200);

if (location.hash === "#volume") {
  switchToTab("volume");
}

if (window.__TAURI__ && window.__TAURI__.event) {
  window.__TAURI__.event.listen("switch-tab", (e) => {
    switchToTab(e.payload);
  });
}
