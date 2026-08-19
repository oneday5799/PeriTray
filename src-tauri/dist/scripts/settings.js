let config = null;

async function saveConfig() {
  try {
    await invoke("update_config", { newConfig: config });
  } catch (e) {
    console.error("Failed to save config:", e);
  }
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

function initComboBox(comboId, selectedValue, onChange) {
  const combo = document.getElementById(comboId);
  if (!combo) return;
  const btn = combo.querySelector('.win-combo-btn');
  const flyout = combo.querySelector('.win-combo-flyout');
  const content = btn.querySelector('.win-combo-content');
  const items = combo.querySelectorAll('.win-combo-item');
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
      item.classList.toggle('selected', isSelected);
      if (isSelected) {
        content.textContent = item.querySelector('.win-combo-item-content').textContent;
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
    flyout.style.clipPath = '';
  }

  function playFlyoutAnimation() {
    cancelFlyoutAnimation();

    const prefersReducedMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
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
      { duration: 800, easing: 'cubic-bezier(0.092, 1.003, 0.028, 0.997)', fill: 'none' }
    );

    flyoutAnimation.onfinish = () => {
      flyoutAnimation = null;
      flyout.style.clipPath = '';
    };
    flyoutAnimation.oncancel = () => {
      flyoutAnimation = null;
    };
  }

  function closeFlyout() {
    flyout.style.display = 'none';
    flyout.style.visibility = 'hidden';
    btn.setAttribute('aria-expanded', 'false');
    combo.classList.remove('is-open');
    cancelFlyoutAnimation();
    removeOverlay();
    document.removeEventListener('pointerdown', onDocPointerDown);
    window.removeEventListener('scroll', onScroll, true);
    window.removeEventListener('resize', onScroll);
  }

  function positionFlyout() {
    const btnRect = btn.getBoundingClientRect();
    const viewportW = window.innerWidth;
    const viewportH = window.innerHeight;
    const itemCount = items.length;
    const selectedIndex = getSelectedIndex();

    flyout.style.visibility = 'hidden';
    flyout.style.display = 'block';
    flyout.style.top = '0px';
    flyout.style.left = '0px';
    flyout.style.width = 'auto';
    flyout.style.maxWidth = viewportW + 'px';

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

    flyout.style.top = Math.round(popupTop) + 'px';
    flyout.style.left = Math.round(popupLeft) + 'px';
    flyout.style.width = Math.round(flyoutW) + 'px';
    flyout.style.maxHeight = flyoutH + 'px';
    flyout.style.visibility = 'visible';

    const scrollTop = Math.max(0, selectedItemCenter - btnCenter + popupTop);
    flyout.scrollTop = scrollTop;
  }

  function onDocPointerDown(e) {
    if (!combo.contains(e.target) && !flyout.contains(e.target)) closeFlyout();
  }

  function onScroll() {
    if (flyout.style.display !== 'none') positionFlyout();
  }

  btn.addEventListener('click', (e) => {
    e.stopPropagation();
    const isOpen = flyout.style.display !== 'none' && flyout.style.visibility !== 'hidden';
    if (isOpen) {
      closeFlyout();
    } else {
      removeOverlay();
      overlay = document.createElement('div');
      overlay.className = 'win-combo-overlay';
      document.body.appendChild(overlay);

      document.body.appendChild(flyout);

      btn.setAttribute('aria-expanded', 'true');
      combo.classList.add('is-open');
      selectItem(currentValue);
      positionFlyout();
      playFlyoutAnimation();

      window.addEventListener('scroll', onScroll, true);
      window.addEventListener('resize', onScroll);
      setTimeout(() => document.addEventListener('pointerdown', onDocPointerDown), 0);
    }
  });

  items.forEach(item => {
    item.addEventListener('click', (e) => {
      e.stopPropagation();
      selectItem(item.dataset.value);
      closeFlyout();
      if (onChange) onChange(currentValue);
    });
  });

  selectItem(currentValue);

  return {
    setValue(v) { selectItem(v); currentValue = v; },
    getValue() { return currentValue; }
  };
}

function initNavigation() {
  let currentTabIndex = 0;
  const INDICATOR_SIZE = 16;
  const EASE_OUT = 'cubic-bezier(0.1, 0.9, 0.2, 1)';

  function getIndicatorY(item) {
    const itemRect = item.getBoundingClientRect();
    const panelRect = document.querySelector('.win-nav-left-panel').getBoundingClientRect();
    return itemRect.top - panelRect.top + (itemRect.height / 2) - (INDICATOR_SIZE / 2);
  }

  function setIndicatorStyle(indicatorEl, y, h) {
    indicatorEl.style.transform = `translateY(${y}px)`;
    indicatorEl.style.height = (h || INDICATOR_SIZE) + 'px';
    indicatorEl.style.transition = 'none';
  }

  function animateIndicator(oldY, newY) {
    const indicatorEl = document.getElementById('nav-indicator');
    if (!indicatorEl) return;
    indicatorEl.getAnimations().forEach(a => a.cancel());
    const distance = Math.abs(newY - oldY);
    const edge = Math.min(oldY, newY);
    const keyframes = [
      { transform: `translateY(${oldY}px)`, height: INDICATOR_SIZE + 'px', offset: 0, easing: 'cubic-bezier(0.9, 0.1, 1, 0.2)' },
      { transform: `translateY(${edge}px)`, height: (distance + INDICATOR_SIZE) + 'px', offset: 0.333, easing: EASE_OUT },
      { transform: `translateY(${newY}px)`, height: INDICATOR_SIZE + 'px', offset: 1 }
    ];
    const anim = indicatorEl.animate(keyframes, { duration: 200, fill: 'forwards' });
    anim.onfinish = () => {
      // Sync computed transform back to style (Web Animations API doesn't update style.transform)
      const computed = getComputedStyle(indicatorEl).transform;
      const match = computed.match(/matrix.*\((.+)\)/);
      if (match) {
        // matrix(a,b,c,d,tx,ty) — ty is at index 5
        const parts = match[1].split(',').map(s => s.trim());
        const ty = parts[5] ? parseFloat(parts[5]) : newY;
        indicatorEl.style.transform = `translateY(${ty}px)`;
      }
      indicatorEl.style.height = INDICATOR_SIZE + 'px';
    };
  }

  // Tab switching (NavigationView Left mode)
  const navItems = document.querySelectorAll(".win-nav-item");
  const pageHeader = document.getElementById("page-header");
  navItems.forEach((tab, index) => {
    tab.addEventListener("click", () => {
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
      const indicatorEl = document.getElementById('nav-indicator');
      if (indicatorEl) {
        const oldTransform = indicatorEl.style.transform;
        const oldY = oldTransform ? parseFloat(oldTransform.match(/translateY\(([-\d.]+)px\)/)?.[1] || 0) : 0;
        const newY = getIndicatorY(tab);
        animateIndicator(oldY, newY);
      }

      // Slide transition — always: old content slides up, new content slides in from bottom
      const oldContent = document.getElementById('tab-' + navItems[oldIndex].dataset.tab);
      const newContent = document.getElementById('tab-' + tab.dataset.tab);

      document.querySelectorAll('.tab-content').forEach(c => {
        c.classList.remove('slide-enter-down', 'slide-leave-up', 'slide-enter-up', 'slide-leave-down', 'slide-active');
      });

      if (oldContent) oldContent.classList.add('slide-leave-up');
      newContent.classList.add('slide-active', 'slide-enter-down');

      setTimeout(() => {
        if (oldContent) oldContent.classList.remove('active', 'slide-leave-up', 'slide-leave-down', 'slide-active');
      }, 170);

      if (pageHeader) pageHeader.textContent = tab.querySelector('.label').textContent;
      currentTabIndex = index;
    });
  });

  // Initialize indicator position after layout is stable
  function initIndicator() {
    const selected = document.querySelector(".win-nav-item.is-selected");
    if (!selected) return;
    const indicatorEl = document.getElementById('nav-indicator');
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
}

function initGeneralTab() {
  bindToggle("toggle-autostart", {
    get: () => config.auto_start,
    set: (v) => { config.auto_start = v; }
  });

  initComboBox("combo-default-popup-tab", config.default_popup_tab || "devices", async (val) => {
    config.default_popup_tab = val;
    await saveConfig();
  });

  bindToggle("toggle-hardware-acceleration", {
    get: () => config.hardware_acceleration || false,
    set: (v) => { config.hardware_acceleration = v; }
  });

  initComboBox("combo-theme-mode", config.theme_mode || "follow_system", async (val) => {
    config.theme_mode = val;
    await saveConfig();
    applyThemeMode(val);
  });
}

function initLogSettings() {
  bindToggle("toggle-log", {
    get: () => config.log_enabled,
    set: (v) => { config.log_enabled = v; }
  });

  initComboBox("combo-log-retention", config.log_retention || "one_day", async (val) => {
    config.log_retention = val;
    await saveConfig();
  });

  document.getElementById("btn-log-dir").addEventListener("click", async () => {
    try {
      await invoke("open_log_dir");
    } catch (e) {
      console.error("Failed to open log dir:", e);
    }
  });
}

function initUpdateSettings() {
  bindToggle("toggle-check-updates", {
    get: () => config.check_updates !== false,
    set: (v) => { config.check_updates = v; },
    onChange: (enabled) => {
      if (!enabled) {
        hideUpdateErrorFlyout();
        const bar = document.getElementById("about-update-infobar");
        if (bar) bar.hidden = true;
      } else {
        invoke("get_update_status").then((status) => {
          renderUpdateInfobar(status);
        }).catch(() => {});
      }
    }
  });

  bindToggle("toggle-include-prerelease", {
    get: () => config.include_prerelease || false,
    set: (v) => { config.include_prerelease = v; }
  });

  document.getElementById("btn-check-update").addEventListener("click", () => {
    runUpdateCheck("btn-check-update");
  });
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

function initDeviceFilterTab() {
  const filterWrap = document.getElementById("filter-regex-wrap");
  const filterArrow = document.getElementById("arrow-filter");
  const filterCard = document.getElementById("filter-card");

  // Filter regex input
  const regexInput = document.getElementById("filter-regex");
  regexInput.value = config.filter_regex || "";

  function resizeRegexInput() {
    regexInput.style.height = "auto";
    regexInput.style.minHeight = "80px";
    regexInput.style.height = Math.max(regexInput.scrollHeight, 80) + "px";
  }
  resizeRegexInput();

  function setFilterExpanded(expanded) {
    if (expanded) {
      filterWrap.classList.add("show");
      filterWrap.style.maxHeight = "999px";
    } else {
      filterWrap.classList.remove("show");
      filterWrap.style.maxHeight = "0px";
    }
    regexInput.style.height = "auto";
    regexInput.style.minHeight = "80px";
    if (filterArrow) filterArrow.classList.toggle("expanded", expanded);
  }

  bindToggle("toggle-filter", {
    get: () => config.filter_enabled,
    set: (v) => { config.filter_enabled = v; },
    onChange: async (checked) => {
      setFilterExpanded(checked);
      await loadDevicesAsync();
    }
  });

  if (config.filter_enabled) {
    filterWrap.classList.add("show");
    filterWrap.style.transition = "none";
    filterWrap.style.maxHeight = "999px";
    regexInput.style.height = "auto";
    regexInput.style.minHeight = "80px";
    requestAnimationFrame(() => {
      filterWrap.style.transition = "";
    });
  } else {
    filterWrap.style.maxHeight = "0px";
  }
  if (filterArrow) filterArrow.classList.toggle("expanded", config.filter_enabled);

  if (filterCard) {
    filterCard.addEventListener("click", (e) => {
      if (e.target.closest('.card-items')) return;
      if (e.target.closest('.toggle')) return;
      const isOpen = filterWrap.classList.contains("show");
      setFilterExpanded(!isOpen);
    });
  }

  let debounceTimer = null;
  regexInput.addEventListener("input", () => {
    resizeRegexInput();
    clearTimeout(debounceTimer);
    debounceTimer = setTimeout(async () => {
      config.filter_regex = regexInput.value;
      await saveConfig();
      await loadDevicesAsync();
    }, 500);
  });

  bindToggle("toggle-dedup", {
    get: () => config.dedup_devices,
    set: (v) => { config.dedup_devices = v; },
    onChange: async () => { await loadDevicesAsync(); }
  });

  bindToggle("toggle-unnamed-bt", {
    get: () => config.show_unnamed_bt,
    set: (v) => { config.show_unnamed_bt = v; },
    onChange: async () => { await loadDevicesAsync(); }
  });

  bindToggle("toggle-use-system-bt", {
    get: () => config.use_system_bt,
    set: (v) => { config.use_system_bt = v; }
  });

  // Open 2.4G device list button
  document.getElementById("btn-add-24g").addEventListener("click", async () => {
    try {
      await invoke("open_24g_device_file");
    } catch (e) {
      console.error("Failed to open file:", e);
    }
  });

  // Help link for 2.4G device
  document.getElementById("help-24g").addEventListener("click", async () => {
    try {
      await invoke("open_url", { url: "https://github.com/oneday5799/PeriphMonitor#24g-%E8%AE%BE%E5%A4%87%E6%94%AF%E6%8C%81" });
    } catch (e) {
      console.error("Failed to open URL:", e);
    }
  });
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

const RELEASES_URL = "https://github.com/oneday5799/PeriphMonitor/releases";

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

function initAboutTab() {
  const card = document.getElementById("about-info-card");
  const items = document.getElementById("about-info-items");
  const arrow = document.getElementById("arrow-about");
  if (card && items) {
    card.addEventListener("click", (e) => {
      if (e.target.closest('.card-items')) return;
      if (e.target.closest("button") || e.target.closest("a")) return;
      const expanded = items.style.maxHeight !== "0px";
      items.style.maxHeight = expanded ? "0px" : "999px";
      if (arrow) arrow.classList.toggle("expanded", !expanded);
    });
  }

  const versionBtn = document.getElementById("about-version-btn");
  if (versionBtn) {
    versionBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      runUpdateCheck("about-version-btn");
    });
  }

  const infobarClose = document.getElementById("infobar-close");
  if (infobarClose) {
    infobarClose.addEventListener("click", () => {
      const bar = document.getElementById("about-infobar");
      if (bar) bar.style.display = "none";
    });
  }

  const updateInfobarClose = document.getElementById("about-update-close");
  if (updateInfobarClose) {
    updateInfobarClose.addEventListener("click", () => {
      hideUpdateErrorFlyout();
      const bar = document.getElementById("about-update-infobar");
      if (bar) bar.hidden = true;
    });
  }

  // 恢复上次更新检测结果（仅当确实检测过时 get_update_status 才有值）
  invoke("get_update_status").then((status) => {
    renderUpdateInfobar(status);
  }).catch(() => {});

  // 启动时自动检测完成后实时更新 infobar
  window.__TAURI__.event.listen("update-status", (event) => {
    renderUpdateInfobar(event.payload);
  });

  const links = {
    "about-dev": "https://github.com/oneday5799",
    "about-license": "https://github.com/oneday5799/PeriphMonitor/blob/main/LICENSE",
    "about-homepage": "https://github.com/oneday5799/PeriphMonitor",
    "about-help": "https://github.com/oneday5799/PeriphMonitor/issues",
    "about-feedback": "https://github.com/oneday5799/PeriphMonitor/issues",
    "about-pr": "https://github.com/oneday5799/PeriphMonitor/pulls"
  };
  Object.entries(links).forEach(([id, url]) => {
    const el = document.getElementById(id);
    if (el) {
      el.addEventListener("click", async (e) => {
        e.preventDefault();
        try {
          await invoke("open_url", { url });
        } catch (err) {
          console.error("Failed to open URL:", err);
        }
      });
    }
  });
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

    loadDevicesAsync();
    loadAudioDevicesAsync();
    initAudioCardToggle();
    initMuteLockSettings();
    initFineAdjustSettings();
    initShutdownVolumeSettings();
    initShortcutSettings();
    initDeviceShortcutSettings();
    setupInputFocus();
  } catch (e) {
    console.error("Failed to load settings:", e);
  }
}

// config-changed: reload config and refresh all dynamic lists
window.__TAURI__.event.listen("config-changed", async () => {
  config = await invoke("get_config");
  await loadDevicesAsync();
  await loadAudioDevicesAsync();
  const listEl = document.getElementById("device-shortcut-list");
  if (listEl) initDeviceShortcutSettings();
});

// 托盘「关于」指向设置页关于标签（窗口已存在时）
window.__TAURI__.event.listen("settings-tab", (e) => {
  selectTab(e.payload);
});

// 初始化输入框 focus 状态（替代 :focus-within，WebView2 兼容）
function setupInputFocus() {
  function setupNumberbox(nb) {
    if (nb.dataset.focusSetup) return;
    nb.dataset.focusSetup = "1";
    const input = nb.querySelector(".win-numberbox-input");
    const border = nb.querySelector(".win-numberbox-border");
    if (!input) return;
    const setFocus = () => nb.classList.add("is-focused");
    const clearFocus = () => nb.classList.remove("is-focused");
    input.addEventListener("focus", setFocus);
    input.addEventListener("blur", clearFocus);
    if (border) {
      const setHover = () => border.classList.add("is-hovered");
      const clearHover = () => border.classList.remove("is-hovered");
      nb.addEventListener("mouseenter", setHover);
      nb.addEventListener("mouseleave", clearHover);
    }
    nb.querySelectorAll(".win-numberbox-spin-btn").forEach(btn => {
      const setHover = () => btn.classList.add("is-hovered");
      const clearHover = () => btn.classList.remove("is-hovered");
      btn.addEventListener("mouseenter", setHover);
      btn.addEventListener("mouseleave", clearHover);
    });
  }

  function setupShortcutInput(input) {
    if (input.dataset.focusSetup) return;
    input.dataset.focusSetup = "1";
    const setFocus = () => input.classList.add("is-focused");
    const clearFocus = () => input.classList.remove("is-focused");
    input.addEventListener("focus", setFocus);
    input.addEventListener("blur", clearFocus);
    const setHover = () => input.classList.add("is-hovered");
    const clearHover = () => input.classList.remove("is-hovered");
    input.addEventListener("mouseenter", setHover);
    input.addEventListener("mouseleave", clearHover);
  }

  // 绑定已存在的元素
  document.querySelectorAll(".win-numberbox").forEach(setupNumberbox);
  document.querySelectorAll(".shortcut-key-input").forEach(setupShortcutInput);

  // 监听后续动态添加的元素
  const observer = new MutationObserver(mutations => {
    for (const m of mutations) {
      for (const node of m.addedNodes) {
        if (node.nodeType !== 1) continue;
        if (node.classList.contains("win-numberbox")) setupNumberbox(node);
        if (node.classList.contains("shortcut-key-input")) setupShortcutInput(node);
        node.querySelectorAll(".win-numberbox").forEach(setupNumberbox);
        node.querySelectorAll(".shortcut-key-input").forEach(setupShortcutInput);
      }
    }
  });
  observer.observe(document.body, { childList: true, subtree: true });

  // 折叠卡 hover 抑制（事件委托，兼容动态添加的分组卡片）
  document.addEventListener("mouseover", (e) => {
    const card = e.target.closest?.(".card.expandable");
    if (!card) return;
    const items = card.querySelector(".card-items");
    if (!items) return;
    card.classList.toggle("no-hover", items.contains(e.target));
  });
}

init();
