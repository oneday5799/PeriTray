/* settings.js — 设置页·入口框架：init 装配/导航与 tab 切换/ComboBox 工厂/更新检测横切流程/
 *            NumberBox 焦点装配/材质浮层背景/config-changed 与 settings-tab 监听
 * 加载序 7/7（最后执行，调用各分区脚本提供的 init 系列与刷新函数）
 * 提供：saveConfig/createCheckableMenu/bindToggle/createToggle/createExpandableCard/
 *       initComboBox/runUpdateCheck/copyToClipboard/showUpdateErrorFlyout/renderUpdateInfobar/
 *       updateFlyoutBackdrop 等框架级共享函数
 * 依赖：common.js 全局 API + 各分区脚本(settings-general/shortcut/devices/audio/about) */
let config = null;
let activeSettingsMenu = null;
// 导航就绪门：get_config 往返期间各标签页内容尚未初始化，
// 此期间的点击/托盘事件先记录、init 完成后统一补执行（防首次切换播在空白面板上）
let settingsNavReady = false;
let pendingNavTab = null;
registerContextMenu({ get menu() { return activeSettingsMenu; }, set menu(v) { activeSettingsMenu = v; } });

// [data-tip] 提示：定位与 DOM 复用 common.js 的边界避让实现
document.addEventListener("pointerenter", (e) => {
  const el = e.target.closest?.("[data-tip]");
  if (el) showSessionTip(el, el.dataset.tip);
}, true);
document.addEventListener("pointerleave", (e) => {
  const el = e.target.closest?.("[data-tip]");
  if (el) hideSessionTip();
}, true);

async function saveConfig() {
  try {
    await invoke("update_config", { newConfig: config });
  } catch (e) {
    console.error("Failed to save config:", e);
  }
}

// ── 复选型多选菜单（低电量设备 / 强制静音设备共用的右键菜单壳） ─────────
// 仅负责 DOM 骨架（context-menu + leading 勾选图标 + 定位 + activeSettingsMenu 登记），
// 数据源/过滤/config 字段/空态文案由调用点注入；onToggle 负责更新 config 并同步 checked。
function createCheckableMenu({ anchor, items, checked, onToggle, emptyText }) {
  if (activeSettingsMenu) {
    hideAllContextMenus();
    return;
  }

  const menu = document.createElement("div");
  menu.className = "context-menu";
  menu.style.maxHeight = "360px";
  menu.style.overflowY = "auto";

  for (const { key, label } of items) {
    const item = document.createElement("div");
    item.className = "context-menu-item";
    item.style.display = "flex";
    item.style.alignItems = "center";

    const leading = document.createElement("span");
    leading.className = "context-menu-leading";
    if (checked.has(key)) {
      leading.appendChild(createCheckIcon());
    }
    item.appendChild(leading);

    const labelEl = document.createElement("span");
    labelEl.textContent = label;
    item.appendChild(labelEl);

    item.addEventListener("click", async (ev) => {
      ev.stopPropagation();
      await onToggle(key);
      leading.innerHTML = "";
      if (checked.has(key)) {
        leading.appendChild(createCheckIcon());
      }
    });

    menu.appendChild(item);
  }

  if (menu.childElementCount === 0) {
    const empty = document.createElement("div");
    empty.className = "context-menu-item";
    empty.textContent = emptyText;
    menu.appendChild(empty);
  }

  document.body.appendChild(menu);
  const rect = anchor.getBoundingClientRect();
  clampMenuPosition(menu, rect.right - menu.offsetWidth, rect.bottom + 4);
  activeSettingsMenu = menu;
}

function bindToggle(id, { get, set, onChange }) {
  const el = document.getElementById(id);
  if (!el) return;
  el.checked = !!get();
  el.addEventListener("change", async () => {
    set(el.checked);
    await saveConfig();
    if (onChange) await onChange(el.checked);
  });
}

function createToggle(checked, onChange, extraClass) {
  const toggle = document.createElement("label");
  toggle.className = "toggle" + (extraClass ? " " + extraClass : "");
  const input = document.createElement("input");
  input.type = "checkbox";
  input.checked = !!checked;
  const slider = document.createElement("span");
  slider.className = "slider";
  toggle.appendChild(input);
  toggle.appendChild(slider);
  if (onChange) input.addEventListener("change", () => onChange(input));
  return { toggle, input };
}

// ── 折叠卡骨架（设置页各折叠卡共享的展开/收起机制） ─────────
// 展开 = .show 类 + arrow 旋转 + inline max-height；CSS 基础态为收起（.card-items{max-height:0}）。
// expandHeight 取值："content"=按内容实测（scrollHeight，适合内容动态的卡）；
//                    "999px" 等固定值=高度上限（适合内容受控的卡），与既有行为一一对应。
function createExpandableCard(items, arrow) {
  function apply(expanded, expandHeight) {
    items.classList.toggle("show", expanded);
    if (arrow) arrow.classList.toggle("expanded", expanded);
    items.style.maxHeight = expanded
      ? (expandHeight === "content" ? items.scrollHeight + "px" : expandHeight)
      : "0px";
  }

  return {
    // 常规切换：带 max-height 过渡动画
    set(expanded, expandHeight = "999px") { apply(expanded, expandHeight); },

    // 初始化恢复：展开分支抑制过渡（避免页面加载时播放收展动画）；收起分支与常规等价
    setInstant(expanded, expandHeight = "999px") {
      if (!expanded) { apply(false); return; }
      items.style.transition = "none";
      apply(true, expandHeight);
      requestAnimationFrame(() => { items.style.transition = ""; });
    },

    // 绑定"点击卡片头部切换"。extraGuards：额外点击排除选择器（如 .toggle/button/a/input）。
    // onChanged(newExpanded)：切换后的附加动作（状态记忆、联动刷新等）。
    bindHeaderClick(card, { extraGuards = [], expandHeight = "999px", onChanged } = {}) {
      card.addEventListener("click", (e) => {
        if (e.target.closest(".card-items")) return;
        for (const sel of extraGuards) {
          if (e.target.closest(sel)) return;
        }
        const expanded = !items.classList.contains("show");
        apply(expanded, expandHeight);
        if (onChanged) onChanged(expanded);
      });
    },
  };
}

function initComboBox(comboId, selectedValue, onChange) {
  const combo = document.getElementById(comboId);
  if (!combo) return;
  const btn = combo.querySelector(".win-combo-btn");
  const flyout = combo.querySelector(".win-combo-flyout");
  const content = btn.querySelector(".win-combo-content");
  const items = combo.querySelectorAll(".win-combo-item");
  const ITEM_HEIGHT = 36;

  let currentValue = selectedValue;
  let overlay = null;
  let flyoutAnimation = null;

  function getSelectedIndex() {
    let idx = 0;
    items.forEach((item, i) => {
      if (item.dataset.value === currentValue) idx = i;
    });
    return idx;
  }

  function selectItem(value) {
    currentValue = value;
    items.forEach(item => {
      const isSelected = item.dataset.value === value;
      item.classList.toggle("selected", isSelected);
      if (isSelected) {
        content.textContent = item.querySelector(".win-combo-item-content").textContent;
      }
    });
  }

  function removeOverlay() {
    if (overlay && overlay.parentElement) {
      overlay.parentElement.removeChild(overlay);
    }
    overlay = null;
  }

  function cancelFlyoutAnimation() {
    if (flyoutAnimation) {
      flyoutAnimation.cancel();
      flyoutAnimation = null;
    }
    flyout.style.clipPath = "";
  }

  function playFlyoutAnimation() {
    cancelFlyoutAnimation();

    const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (prefersReducedMotion) return;

    const rect = flyout.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    // Edge strip at top (where button is), expand downward
    const stripSize = 36;
    const margin = 15;
    const startRect = { left: 0, right: rect.width, top: 0, bottom: stripSize };
    const endRect = { left: -margin, right: rect.width + margin, top: -margin, bottom: rect.height + margin };

    const toPolygon = (r) => `polygon(${r.left}px ${r.top}px, ${r.right}px ${r.top}px, ${r.right}px ${r.bottom}px, ${r.left}px ${r.bottom}px)`;

    flyoutAnimation = flyout.animate(
      [
        { clipPath: toPolygon(startRect) },
        { clipPath: toPolygon(endRect) }
      ],
      { duration: 800, easing: "cubic-bezier(0.092, 1.003, 0.028, 0.997)", fill: "none" }
    );

    flyoutAnimation.onfinish = () => {
      flyoutAnimation = null;
      flyout.style.clipPath = "";
    };
    flyoutAnimation.oncancel = () => {
      flyoutAnimation = null;
    };
  }

  function closeFlyout() {
    flyout.style.display = "none";
    flyout.style.visibility = "hidden";
    btn.setAttribute("aria-expanded", "false");
    combo.classList.remove("is-open");
    cancelFlyoutAnimation();
    removeOverlay();
    document.removeEventListener("pointerdown", onDocPointerDown);
    window.removeEventListener("scroll", onScroll, true);
    window.removeEventListener("resize", onScroll);
  }

  function positionFlyout() {
    const btnRect = btn.getBoundingClientRect();
    const viewportW = window.innerWidth;
    const viewportH = window.innerHeight;
    const itemCount = items.length;
    const selectedIndex = getSelectedIndex();

    flyout.style.visibility = "hidden";
    flyout.style.display = "block";
    flyout.style.top = "0px";
    flyout.style.left = "0px";
    flyout.style.width = "auto";
    flyout.style.maxWidth = viewportW + "px";

    const flyoutRect = flyout.getBoundingClientRect();
    const flyoutW = Math.max(btnRect.width, flyoutRect.width);

    const maxItems = Math.min(9, itemCount);
    const flyoutH = maxItems * ITEM_HEIGHT + 8;
    const selectedItemCenter = selectedIndex * ITEM_HEIGHT + ITEM_HEIGHT / 2 + 4;
    const btnCenter = btnRect.top + btnRect.height / 2;

    let popupTop = btnCenter - selectedItemCenter;
    if (popupTop + flyoutH > viewportH - 4) popupTop = viewportH - flyoutH - 4;
    if (popupTop < 4) popupTop = 4;

    let popupLeft = btnRect.left;
    if (popupLeft + flyoutW > viewportW - 4) popupLeft = Math.max(4, viewportW - flyoutW - 4);

    flyout.style.top = Math.round(popupTop) + "px";
    flyout.style.left = Math.round(popupLeft) + "px";
    flyout.style.width = Math.round(flyoutW) + "px";
    flyout.style.maxHeight = flyoutH + "px";
    flyout.style.visibility = "visible";

    const scrollTop = Math.max(0, selectedItemCenter - btnCenter + popupTop);
    flyout.scrollTop = scrollTop;
  }

  function onDocPointerDown(e) {
    if (!combo.contains(e.target) && !flyout.contains(e.target)) closeFlyout();
  }

  function onScroll() {
    if (flyout.style.display !== "none") positionFlyout();
  }

  btn.addEventListener("click", (e) => {
    e.stopPropagation();
    const isOpen = flyout.style.display !== "none" && flyout.style.visibility !== "hidden";
    if (isOpen) {
      closeFlyout();
    } else {
      removeOverlay();
      overlay = document.createElement("div");
      overlay.className = "win-combo-overlay";
      document.body.appendChild(overlay);

      document.body.appendChild(flyout);

      btn.setAttribute("aria-expanded", "true");
      combo.classList.add("is-open");
      selectItem(currentValue);
      positionFlyout();
      playFlyoutAnimation();

      window.addEventListener("scroll", onScroll, true);
      window.addEventListener("resize", onScroll);
      setTimeout(() => document.addEventListener("pointerdown", onDocPointerDown), 0);
    }
  });

  items.forEach(item => {
    item.addEventListener("click", (e) => {
      e.stopPropagation();
      selectItem(item.dataset.value);
      closeFlyout();
      if (onChange) onChange(currentValue);
    });
  });

  selectItem(currentValue);
}

function initNavigation() {
  let currentTabIndex = 0;
  const INDICATOR_SIZE = 16;
  const EASE_OUT = "cubic-bezier(0.1, 0.9, 0.2, 1)";

  function getIndicatorY(item) {
    const itemRect = item.getBoundingClientRect();
    const panelRect = document.querySelector(".win-nav-left-panel").getBoundingClientRect();
    return itemRect.top - panelRect.top + (itemRect.height / 2) - (INDICATOR_SIZE / 2);
  }

  function setIndicatorStyle(indicatorEl, y, h) {
    indicatorEl.style.transform = `translateY(${y}px)`;
    indicatorEl.style.height = (h || INDICATOR_SIZE) + "px";
    indicatorEl.style.transition = "none";
  }

  function animateIndicator(oldY, newY) {
    const indicatorEl = document.getElementById("nav-indicator");
    if (!indicatorEl) return;
    indicatorEl.getAnimations().forEach(a => a.cancel());
    const distance = Math.abs(newY - oldY);
    const edge = Math.min(oldY, newY);
    const keyframes = [
      { transform: `translateY(${oldY}px)`, height: INDICATOR_SIZE + "px", offset: 0, easing: "cubic-bezier(0.9, 0.1, 1, 0.2)" },
      { transform: `translateY(${edge}px)`, height: (distance + INDICATOR_SIZE) + "px", offset: 0.333, easing: EASE_OUT },
      { transform: `translateY(${newY}px)`, height: INDICATOR_SIZE + "px", offset: 1 }
    ];
    const anim = indicatorEl.animate(keyframes, { duration: 200, fill: "forwards" });
    anim.onfinish = () => {
      // 回写最终位并立即释放动画：fill:"forwards" 的已完成动画仍会覆盖后续
      // inline 样式（折叠侧栏时 repositionIndicator 重算的 transform 会被旧动画值压住）
      indicatorEl.style.transform = `translateY(${newY}px)`;
      indicatorEl.style.height = INDICATOR_SIZE + "px";
      anim.cancel();
    };
  }

  // Tab switching (NavigationView Left mode)
  const navItems = document.querySelectorAll(".win-nav-item");
  const pageHeader = document.getElementById("page-header");
  navItems.forEach((tab, index) => {
    tab.addEventListener("click", () => {
      if (!settingsNavReady) { pendingNavTab = tab; return; }
      if (currentTabIndex === index) return;
      const oldIndex = currentTabIndex;

      // Update nav item states
      navItems.forEach(t => {
        t.classList.remove("is-selected");
        t.setAttribute("aria-selected", "false");
        t.setAttribute("tabindex", "-1");
      });
      tab.classList.add("is-selected");
      tab.setAttribute("aria-selected", "true");
      tab.setAttribute("tabindex", "0");

      // Animate indicator with stretch
      const indicatorEl = document.getElementById("nav-indicator");
      if (indicatorEl) {
        const oldTransform = indicatorEl.style.transform;
        const oldY = oldTransform ? parseFloat(oldTransform.match(/translateY\(([-\d.]+)px\)/)?.[1] || 0) : 0;
        const newY = getIndicatorY(tab);
        animateIndicator(oldY, newY);
      }

      // Slide transition — always: old content slides up, new content slides in from bottom
      const oldContent = document.getElementById("tab-" + navItems[oldIndex].dataset.tab);
      const newContent = document.getElementById("tab-" + tab.dataset.tab);

      document.querySelectorAll(".tab-content").forEach(c => {
        c.classList.remove("slide-enter-down", "slide-leave-up", "slide-active");
      });

      if (oldContent) oldContent.classList.add("slide-leave-up");
      newContent.classList.add("slide-active", "slide-enter-down");

      setTimeout(() => {
        if (oldContent) oldContent.classList.remove("active", "slide-leave-up", "slide-active");
      }, 170);

      if (pageHeader) pageHeader.textContent = tab.querySelector(".label").textContent;
      currentTabIndex = index;
    });
  });

  // Initialize indicator position after layout is stable
  function initIndicator() {
    const selected = document.querySelector(".win-nav-item.is-selected");
    if (!selected) return;
    const indicatorEl = document.getElementById("nav-indicator");
    if (!indicatorEl) return;
    const y = getIndicatorY(selected);
    // Verify position is reasonable (within panel bounds)
    if (y >= 0 && y < 500) {
      setIndicatorStyle(indicatorEl, y);
    } else {
      // Retry after layout settles
      requestAnimationFrame(() => initIndicator());
    }
  }
  requestAnimationFrame(() => requestAnimationFrame(initIndicator));

  // 侧栏折叠/窗口缩放时重算指示条 Y（header 高度变化会使导航项垂直位移）
  function repositionIndicator() {
    const selected = document.querySelector(".win-nav-item.is-selected");
    const indicatorEl = document.getElementById("nav-indicator");
    if (!selected || !indicatorEl) return;
    // 取消进行中的指示条动画（Web Animations 会覆盖直接写 style.transform）
    indicatorEl.getAnimations().forEach(a => a.cancel());
    const y = getIndicatorY(selected);
    if (y >= 0 && y < 500) {
      setIndicatorStyle(indicatorEl, y);
    }
  }
  let repositionRaf = 0;
  function scheduleReposition() {
    cancelAnimationFrame(repositionRaf);
    repositionRaf = requestAnimationFrame(repositionIndicator);
  }
  window.addEventListener("resize", scheduleReposition);
  window.matchMedia("(max-width: 820px)").addEventListener("change", scheduleReposition);
}

async function runUpdateCheck(btnId) {
  const btn = document.getElementById(btnId);
  if (!btn) return;
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
      renderUpdateInfobar({
        status: "update",
        currentVersion: info.current_version,
        latestVersion: info.latest_version,
        releaseUrl: info.release_url,
        error: null
      });
      showToast(
        `发现新版本 ${info.latest_version}（当前 ${info.current_version}）<br>点击前往下载`,
        () => invoke("open_url", { url: info.release_url })
      );
    } else {
      renderUpdateInfobar({
        status: "latest",
        currentVersion: info.current_version,
        latestVersion: info.latest_version,
        releaseUrl: info.release_url,
        error: null
      });
      showToast("已是最新版本");
    }
  } catch (e) {
    clearTimeout(timeoutId);
    const err = String(e);
    renderUpdateInfobar({
      status: "error",
      currentVersion: "",
      latestVersion: "",
      releaseUrl: "",
      error: err
    });
    if (err.includes("超时") || err.includes("timeout")) {
      showToast("检测超时，请检查网络后重试");
    } else if (err.includes("频繁") || err.includes("rate_limited")) {
      showToast("GitHub API 请求过于频繁，请稍后再试");
    } else {
      showToast("检测失败：" + err);
    }
  } finally {
    btn.textContent = originalText;
    btn.disabled = false;
  }
}

function selectTab(tab) {
  const nav = document.querySelector(`.win-nav-item[data-tab="${tab}"]`);
  if (nav) nav.click();
}

const UPDATE_ICONS = {
  success: '<svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 1.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13zm3.53 4.03l-4.28 4.28-1.78-1.77a.75.75 0 1 0-1.06 1.06l2.31 2.31c.29.3.77.3 1.06 0l4.81-4.81a.75.75 0 1 0-1.06-1.06z"/></svg>',
  warning: '<svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 1.5L.75 14.5h14.5L8 1.5zm0 3.26l5.04 8.74H2.96L8 4.76zM8.75 7.5v3a.75.75 0 1 1-1.5 0v-3a.75.75 0 1 1 1.5 0zM8 12.5a.75.75 0 1 0 0-1.5.75.75 0 0 0 0 1.5z"/></svg>',
  error: '<svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor" aria-hidden="true"><path d="M8 1.5a6.5 6.5 0 1 0 0 13 6.5 6.5 0 0 0 0-13zm0 12A5.5 5.5 0 1 1 8 2.5a5.5 5.5 0 0 1 0 11zm2.03-8.53L8 6.94 5.97 4.97 4.97 5.97 6.94 8l-1.97 2.03 1 1L8 9.06l2.03 1.97 1-1L9.06 8l1.97-2.03-1-1z"/></svg>'
};

const RELEASES_URL = "https://github.com/oneday5799/PeriTray/releases";

function hideUpdateErrorFlyout() {
  const flyout = document.getElementById("about-update-flyout");
  if (flyout) flyout.hidden = true;
  if (window.__updateFlyoutDismiss) {
    document.removeEventListener("pointerdown", window.__updateFlyoutDismiss);
    window.__updateFlyoutDismiss = null;
  }
}

function copyToClipboard(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(text);
  }
  return new Promise((resolve, reject) => {
    try {
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.focus();
      ta.select();
      const ok = document.execCommand("copy");
      document.body.removeChild(ta);
      ok ? resolve() : reject(new Error("copy failed"));
    } catch (e) {
      reject(e);
    }
  });
}

function classifyUpdateError(err) {
  if (err.includes("超时") || err.includes("timeout")) return "检测超时，请检查网络后重试";
  if (err.includes("频繁") || err.includes("rate_limited")) return "GitHub API 请求过于频繁，请稍后再试";
  if (err.includes("403")) return "GitHub API 请求被拒绝（403），请稍后再试";
  if (err.includes("404")) return "未找到发布资源（404），请确认仓库地址";
  if (err.includes("解析失败")) return "响应数据解析失败，请稍后再试";
  return "";
}

function extractErrorCode(err) {
  const m = String(err).match(/\((\d{2,5})\)\s*$/);
  return m ? m[1] : "";
}

function showUpdateErrorFlyout(errorText, anchorBtn) {
  const flyout = document.getElementById("about-update-flyout");
  const summaryEl = document.getElementById("about-update-error-summary");
  const textEl = document.getElementById("about-update-error-text");
  if (!flyout || !textEl) return;
  const detail = errorText || "未知错误";
  const code = extractErrorCode(detail);
  const message = code ? detail.replace(/\(\d{2,5}\)\s*$/, "").trim() : detail;
  if (summaryEl) {
    const summary = classifyUpdateError(detail);
    summaryEl.textContent = summary;
    summaryEl.hidden = !summary;
  }
  textEl.textContent = code ? `${message}，错误代码：${code}` : message;
  flyout.hidden = false;
  const rect = anchorBtn.getBoundingClientRect();
  let left = rect.right - flyout.offsetWidth;
  let top = rect.bottom + 8;
  if (left < 8) left = 8;
  if (top + flyout.offsetHeight > window.innerHeight - 8) {
    top = Math.max(8, rect.top - flyout.offsetHeight - 8);
  }
  flyout.style.left = left + "px";
  flyout.style.top = top + "px";

  const copyBtn = document.getElementById("about-update-error-copy");
  copyBtn.onclick = async () => {
    try {
      await copyToClipboard(detail);
      showToast("已复制到剪贴板");
    } catch (e) {
      showToast("复制失败");
    }
    hideUpdateErrorFlyout();
  };

  const dismiss = (e) => {
    if (!flyout.contains(e.target) && e.target !== anchorBtn && !anchorBtn.contains(e.target)) {
      hideUpdateErrorFlyout();
    }
  };
  window.__updateFlyoutDismiss = dismiss;
  document.addEventListener("pointerdown", dismiss);
}

function renderUpdateInfobar(status) {
  const bar = document.getElementById("about-update-infobar");
  if (!bar) return;
  if (!status || !status.status) {
    bar.hidden = true;
    return;
  }
  bar.hidden = false;
  bar.classList.remove("win-infobar-success", "win-infobar-warning", "win-infobar-error");

  const icon = document.getElementById("about-update-icon");
  const content = document.getElementById("about-update-content");
  const action = document.getElementById("about-update-action");
  if (!icon || !content || !action) return;
  action.hidden = false;

  if (status.status === "latest") {
    bar.classList.add("win-infobar-success");
    icon.innerHTML = UPDATE_ICONS.success;
    content.textContent = `当前版本 v${status.currentVersion} ，已是最新版本。`;
    action.textContent = "查看更新日志";
    action.onclick = () => invoke("open_url", { url: RELEASES_URL });
  } else if (status.status === "update") {
    bar.classList.add("win-infobar-warning");
    icon.innerHTML = UPDATE_ICONS.warning;
    content.textContent = `检测到新版本 v${status.latestVersion} ，当前版本 v${status.currentVersion} 。`;
    action.textContent = "下载最新版本";
    action.onclick = () => invoke("open_url", { url: status.releaseUrl || RELEASES_URL });
  } else {
    bar.classList.add("win-infobar-error");
    icon.innerHTML = UPDATE_ICONS.error;
    content.textContent = "无法检测更新，请检查网络状况或稍后再试。";
    action.textContent = "查看详细信息";
    action.onclick = () => showUpdateErrorFlyout(status.error, action);
  }
}

async function init() {
  initNavigation();

  // 从托盘「关于」打开时定位到对应标签页
  const hash = location.hash.slice(1);
  if (hash) selectTab(hash);

  try {
    config = await invoke("get_config");

    applyThemeMode(config.theme_mode || "follow_system");
    initGeneralTab();
    initUpdateSettings();
    initLogSettings();
    initDeviceFilterTab();
    initAboutTab();
    initLowBatteryNotifyTab();
    initMaterialEffects();

    // 跟随系统时，实时响应系统主题切换 → 刷新 flyout 背景颜色
    new MutationObserver(() => updateFlyoutBackdrop(config?.window_material || "default"))
      .observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    loadDevicesAsync();
    loadAudioDevicesAsync();
    initAudioCardToggle();
    initMuteLockSettings();
    initFineAdjustSettings();
    initForceMuteSettings();
    initSpatialSoundSettings();
    initSimplifyNamesSettings();
    initShutdownVolumeSettings();
    initShortcutSettings();
    initDeviceShortcutSettings();
    setupCardHoverSuppression();
  } catch (e) {
    console.error("Failed to load settings:", e);
  }

  // 预热：提前完成各面板首次布局/绘制，避免首次切换动画被渲染耗时吃掉
  document.querySelectorAll(".tab-content").forEach(p => {
    p.style.display = "block";
    void p.offsetHeight;
    p.style.display = "";
  });

  // 无条件放行导航（配置加载失败也必须可操作），并补执行就绪前的待处理切换
  settingsNavReady = true;
  if (pendingNavTab) {
    const t = pendingNavTab;
    pendingNavTab = null;
    t.click();
  }
}

// config-changed: reload config and refresh all dynamic lists
window.__TAURI__.event.listen("config-changed", async () => {
  config = await invoke("get_config");
  await loadDevicesAsync();
  await loadAudioDevicesAsync();
  await renderShutdownVolumeDevices();
  const listEl = document.getElementById("device-shortcut-list");
  if (listEl) initDeviceShortcutSettings();
  await renderLowBatteryContent();
  // 材质切换进行中时跳过 initMaterialEffects，避免 data-material 被提前设置
  if (!window.__materialChangeInProgress) {
    initMaterialEffects();
  }
});

// 托盘「关于」指向设置页关于标签（窗口已存在时）
window.__TAURI__.event.listen("settings-tab", (e) => {
  selectTab(e.payload);
});

// 折叠卡 hover 抑制（事件委托，兼容动态添加的分组卡片）
function setupCardHoverSuppression() {
  document.addEventListener("mouseover", (e) => {
    const card = e.target.closest?.(".card.expandable");
    if (!card) return;
    const items = card.querySelector(".card-items");
    if (!items) return;
    card.classList.toggle("no-hover", items.contains(e.target));
  });
}

// ═══════════════════════════════════════════════════════════════
// 窗口材质辅助函数
// ═══════════════════════════════════════════════════════════════

/// 更新 flyout 背景模糊效果（Acrylic backdrop-filter）
function updateFlyoutBackdrop(material) {
  const root = document.documentElement;
  if (material === "recommended" || material === "acrylic") {
    root.style.setProperty("--flyout-backdrop", "blur(30px) saturate(125%)");
    const isDark = root.getAttribute("data-theme") === "dark";
    root.style.setProperty("--flyout-bg", isDark ? "rgba(44, 44, 44, 0.85)" : "rgba(252, 252, 252, 0.85)");
  } else {
    root.style.removeProperty("--flyout-backdrop");
    root.style.removeProperty("--flyout-bg");
  }
}

/// 初始化时应用当前材质的 CSS 效果
function initMaterialEffects() {
  const material = config?.window_material || "default";
  updateFlyoutBackdrop(material);
  applyMaterialMode(material);
}

init();
