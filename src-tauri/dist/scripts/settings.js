async function saveConfig() {
  try {
    await invoke("update_config", { newConfig: config });
  } catch (e) {
    console.error("Failed to save config:", e);
  }
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

async function init() {
  // NavigationView Top indicator (WinUIonWeb stretch animation)
  const INDICATOR_SIZE = 16;
  const EASE_OUT = 'cubic-bezier(0.1, 0.9, 0.2, 1)';
  const EASE_COLLAPSE = 'cubic-bezier(0.4, 0.0, 0.7, 0.3)';
  let indicatorAnimationId = 0;
  let currentTabIndex = 0;
  let isTransitioning = false;

  function getIndicatorX(item) {
    const itemRect = item.getBoundingClientRect();
    const track = document.querySelector('.win-nav-indicator-track');
    const trackRect = track.getBoundingClientRect();
    return itemRect.left - trackRect.left + (itemRect.width / 2) - (INDICATOR_SIZE / 2);
  }

  function setIndicatorRestingStyle(indicatorEl, x) {
    indicatorEl.style.transform = `translateX(${x}px)`;
    indicatorEl.style.width = INDICATOR_SIZE + 'px';
    indicatorEl.style.transition = 'none';
  }

  function animateIndicator(oldX, newX) {
    const indicatorEl = document.getElementById('nav-indicator');
    if (!indicatorEl) return;
    indicatorEl.getAnimations().forEach(a => a.cancel());
    const animationId = ++indicatorAnimationId;
    const distance = Math.abs(newX - oldX);
    const edge = Math.min(oldX, newX);
    const dur = 600;
    const keyframes = [
      { transform: `translateX(${oldX}px)`, width: INDICATOR_SIZE + 'px', offset: 0, easing: 'cubic-bezier(0.9, 0.1, 1, 0.2)' },
      { transform: `translateX(${edge}px)`, width: (distance + INDICATOR_SIZE) + 'px', offset: 0.333, easing: EASE_OUT },
      { transform: `translateX(${newX}px)`, width: INDICATOR_SIZE + 'px', offset: 1 }
    ];
    const anim = indicatorEl.animate(keyframes, { duration: dur, fill: 'forwards' });
    anim.onfinish = () => {
      if (animationId === indicatorAnimationId) {
        setIndicatorRestingStyle(indicatorEl, newX);
      }
    };
  }

  function moveIndicator(item, animate) {
    const indicatorEl = document.getElementById('nav-indicator');
    if (!indicatorEl || !item) return;
    const newX = getIndicatorX(item);
    if (animate) {
      const oldTransform = indicatorEl.style.transform;
      const oldX = oldTransform ? parseFloat(oldTransform.match(/translateX\(([^)]+)px\)/)?.[1] || 0) : newX;
      animateIndicator(oldX, newX);
    } else {
      setIndicatorRestingStyle(indicatorEl, newX);
    }
  }

  // Slide page transition (WinUIonWeb SlideNavigationTransitionInfo)
  function slideTransition(oldTab, newTab, oldIndex, newIndex) {
    if (isTransitioning) return;
    isTransitioning = true;
    const goingRight = newIndex > oldIndex;
    const oldContent = document.getElementById('tab-' + oldTab.dataset.tab);
    const newContent = document.getElementById('tab-' + newTab.dataset.tab);

    // Remove any lingering animation classes
    document.querySelectorAll('.tab-content').forEach(c => {
      c.classList.remove('slide-enter-right', 'slide-leave-left', 'slide-enter-left', 'slide-leave-right', 'slide-active');
    });

    // Start leave animation on old content
    if (oldContent) {
      oldContent.classList.add(goingRight ? 'slide-leave-left' : 'slide-leave-right');
    }

    // Start enter animation on new content
    newContent.classList.add('slide-active', goingRight ? 'slide-enter-right' : 'slide-enter-left');

    // After leave animation finishes, hide old content
    setTimeout(() => {
      if (oldContent) {
        oldContent.classList.remove('active', 'slide-leave-left', 'slide-leave-right', 'slide-active');
      }
      isTransitioning = false;
    }, 150);
  }

  // Tab switching (NavigationView Top mode)
  const navItems = document.querySelectorAll(".win-nav-item");
  navItems.forEach((tab, index) => {
    tab.addEventListener("click", () => {
      if (isTransitioning) return;
      const oldIndex = currentTabIndex;
      const newIndex = index;
      if (oldIndex === newIndex) return;

      // Update nav item states
      navItems.forEach(t => {
        t.classList.remove("is-selected");
        t.setAttribute("aria-selected", "false");
        t.setAttribute("tabindex", "-1");
      });
      tab.classList.add("is-selected");
      tab.setAttribute("aria-selected", "true");
      tab.setAttribute("tabindex", "0");

      // Get old and new tab elements
      const oldTab = navItems[oldIndex];
      const newTab = navItems[newIndex];

      // Animate indicator
      moveIndicator(tab, true);

      // Animate page transition
      slideTransition(oldTab, newTab, oldIndex, newIndex);

      currentTabIndex = newIndex;
    });
  });

  // Initialize indicator position (no animation)
  requestAnimationFrame(() => {
    const selected = document.querySelector(".win-nav-item.is-selected");
    if (selected) moveIndicator(selected, false);
  });

  // Reposition indicator on resize (no animation)
  window.addEventListener("resize", () => {
    const selected = document.querySelector(".win-nav-item.is-selected");
    if (selected) moveIndicator(selected, false);
  });

  try {
    config = await invoke("get_config");

    // Auto-start toggle
    const toggle = document.getElementById("toggle-autostart");
    toggle.checked = config.auto_start;
    toggle.addEventListener("change", async () => {
      config.auto_start = toggle.checked;
      await saveConfig();
    });

    // Default popup tab
    initComboBox("combo-default-popup-tab", config.default_popup_tab || "devices", async (val) => {
      config.default_popup_tab = val;
      await saveConfig();
    });

    // Hardware acceleration toggle
    const hwAccel = document.getElementById("toggle-hardware-acceleration");
    hwAccel.checked = config.hardware_acceleration || false;
    hwAccel.addEventListener("change", async () => {
      config.hardware_acceleration = hwAccel.checked;
      await saveConfig();
    });

    // Filter toggle
    const filterToggle = document.getElementById("toggle-filter");
    const filterWrap = document.getElementById("filter-regex-wrap");
    const filterArrow = document.getElementById("arrow-filter");
    filterToggle.checked = config.filter_enabled;
    filterWrap.style.display = config.filter_enabled ? "block" : "none";
    if (filterArrow) filterArrow.classList.toggle("expanded", config.filter_enabled);
    filterToggle.addEventListener("change", async () => {
      config.filter_enabled = filterToggle.checked;
      filterWrap.style.display = filterToggle.checked ? "block" : "none";
      if (filterArrow) filterArrow.classList.toggle("expanded", filterToggle.checked);
      await saveConfig();
      await loadDevicesAsync();
    });

    // Filter regex input
    const regexInput = document.getElementById("filter-regex");
    regexInput.value = config.filter_regex || "";
    let debounceTimer = null;
    regexInput.addEventListener("input", () => {
      clearTimeout(debounceTimer);
      debounceTimer = setTimeout(async () => {
        config.filter_regex = regexInput.value;
        await saveConfig();
        await loadDevicesAsync();
      }, 500);
    });

    // Dedup toggle
    const dedupToggle = document.getElementById("toggle-dedup");
    dedupToggle.checked = config.dedup_devices;
    dedupToggle.addEventListener("change", async () => {
      config.dedup_devices = dedupToggle.checked;
      await saveConfig();
      await loadDevicesAsync();
    });

    // Show unnamed BT devices toggle
    const unnamedBtToggle = document.getElementById("toggle-unnamed-bt");
    unnamedBtToggle.checked = config.show_unnamed_bt;
    unnamedBtToggle.addEventListener("change", async () => {
      config.show_unnamed_bt = unnamedBtToggle.checked;
      await saveConfig();
      await loadDevicesAsync();
    });

    // Use system Bluetooth connection toggle
    const useSystemBtToggle = document.getElementById("toggle-use-system-bt");
    useSystemBtToggle.checked = config.use_system_bt;
    useSystemBtToggle.addEventListener("change", async () => {
      config.use_system_bt = useSystemBtToggle.checked;
      await saveConfig();
    });

    // Logging settings
    const logToggle = document.getElementById("toggle-log");
    logToggle.checked = config.log_enabled;
    logToggle.addEventListener("change", async () => {
      config.log_enabled = logToggle.checked;
      await saveConfig();
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

    const checkUpdatesToggle = document.getElementById("toggle-check-updates");
    checkUpdatesToggle.checked = config.check_updates !== false;
    checkUpdatesToggle.addEventListener("change", async () => {
      config.check_updates = checkUpdatesToggle.checked;
      await saveConfig();
    });

    const includePrereleaseToggle = document.getElementById("toggle-include-prerelease");
    includePrereleaseToggle.checked = config.include_prerelease || false;
    includePrereleaseToggle.addEventListener("change", async () => {
      config.include_prerelease = includePrereleaseToggle.checked;
      await saveConfig();
    });

    document.getElementById("btn-check-update").addEventListener("click", async () => {
      const btn = document.getElementById("btn-check-update");
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
          showToast(
            `发现新版本 ${info.latest_version}（当前 ${info.current_version}）<br>点击前往下载`,
            () => invoke("open_url", { url: info.release_url })
          );
        } else {
          showToast("已是最新版本");
        }
      } catch (e) {
        clearTimeout(timeoutId);
        const err = String(e);
        if (err.includes("超时") || err.includes("timeout")) {
          showToast(
            "检测超时，请检查网络后重试<br>点击前往 Release 页面",
            () => invoke("open_url", { url: "https://github.com/oneday5799/PeriphMonitor/releases" })
          );
        } else if (err.includes("频繁") || err.includes("rate_limited")) {
          showToast("GitHub API 请求过于频繁，请稍后再试");
        } else {
          showToast("检测失败：" + err);
        }
      } finally {
        btn.textContent = originalText;
        btn.disabled = false;
      }
    });

    loadDevicesAsync();
    loadAudioDevicesAsync();
    initShutdownVolumeSettings();
    initShortcutSettings();
    initDeviceShortcutSettings();

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
        await invoke("open_url", { url: "https://github.com/oneday5799/PeriphMonitor#%E6%B7%BB%E5%8A%A0%E8%87%AA%E5%AE%9A%E4%B9%89-24g-%E8%AE%BE%E5%A4%87" });
      } catch (e) {
        console.error("Failed to open URL:", e);
      }
    });
  } catch (e) {
    console.error("Failed to load settings:", e);
  }
}

init();
