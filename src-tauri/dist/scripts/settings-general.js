/* settings-general.js — 设置页·通用设置 tab：主题模式/窗口材质/默认打开页/硬件加速/
 *                        开机自启动/日志设置/更新开关（更新检测流程在 settings.js 横切共享）
 * 加载序 2/7 · 提供：initGeneralTab() / initLogSettings() / initUpdateSettings()
 * 依赖：common.js(applyThemeMode/showToast) /
 *       settings.js(config/bindToggle/initComboBox/saveConfig) */
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
    updateFlyoutBackdrop(config.window_material || "default");
  });

  initComboBox("combo-window-material", config.window_material || "default", async (val) => {
    // 云母在不支持的系统上需提示
    if (val === "mica") {
      const supported = await invoke("check_material_support", { material: "mica" });
      if (!supported) {
        showToast("当前系统不支持云母材质", null, true);
        return;
      }
    }
    config.window_material = val;
    window.__materialChangeInProgress = true;
    await saveConfig();
    updateFlyoutBackdrop(val);
    if (val === "default") {
      // 先移除 CSS 透明规则，再移除 DWM 材质，避免闪烁
      updateMaterialAttribute(val);
      await invoke("set_window_material", { material: val });
    } else {
      await invoke("set_window_material", { material: val });
      // 等待 DWM 材质生效 + webview 背景透明化后再设置 CSS 属性
      await new Promise(r => setTimeout(r, 200));
      updateMaterialAttribute(val);
    }
    window.__materialChangeInProgress = false;
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
