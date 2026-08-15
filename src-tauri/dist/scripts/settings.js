async function saveConfig() {
  try {
    await invoke("update_config", { newConfig: config });
  } catch (e) {
    console.error("Failed to save config:", e);
  }
}

async function init() {
  // Tab switching
  document.querySelectorAll(".tab-item").forEach(tab => {
    tab.addEventListener("click", () => {
      document.querySelectorAll(".tab-item").forEach(t => t.classList.remove("active"));
      document.querySelectorAll(".tab-content").forEach(c => c.classList.remove("active"));
      tab.classList.add("active");
      document.getElementById("tab-" + tab.dataset.tab).classList.add("active");
    });
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
    const defaultPopupTab = document.getElementById("default-popup-tab");
    defaultPopupTab.value = config.default_popup_tab || "devices";
    defaultPopupTab.addEventListener("change", async () => {
      config.default_popup_tab = defaultPopupTab.value;
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
    filterToggle.checked = config.filter_enabled;
    filterWrap.style.display = config.filter_enabled ? "block" : "none";
    filterToggle.addEventListener("change", async () => {
      config.filter_enabled = filterToggle.checked;
      filterWrap.style.display = filterToggle.checked ? "block" : "none";
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

    const logRetentionSelect = document.getElementById("log-retention");
    logRetentionSelect.value = config.log_retention || "one_day";
    logRetentionSelect.addEventListener("change", async () => {
      config.log_retention = logRetentionSelect.value;
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
