/* settings-about.js — 设置页·关于 tab：版本按钮触发更新检测/infobar 与链接交互
 * 加载序 6/7 · 提供：initAboutTab()
 * 依赖：common.js(invoke) /
 *       settings.js(runUpdateCheck/hideUpdateErrorFlyout/renderUpdateInfobar/createExpandableCard) */
function initAboutTab() {
  const card = document.getElementById("about-info-card");
  const items = document.getElementById("about-info-items");
  const arrow = document.getElementById("arrow-about");
  if (card && items) {
    // HTML 初始即带 .show + inline 999px；此后展开态由骨架经 .show 类跟踪
    createExpandableCard(items, arrow).bindHeaderClick(card, {
      extraGuards: ["button", "a"],
    });
  }

  const versionBtn = document.getElementById("about-version-btn");
  if (versionBtn) {
    versionBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      runUpdateCheck("about-version-btn");
    });
    // 静态文案仅作首帧占位，加载后以真实包版本覆盖（版本号唯一事实来源是 tauri.conf.json）
    invoke("get_app_version").then((v) => {
      versionBtn.textContent = `版本 v${v}`;
    }).catch(() => {});
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
    "about-24g-devices": "https://github.com/oneday5799/PeriphMonitor/wiki/11-%E6%94%AF%E6%8C%81%E8%AE%BE%E5%A4%87%E5%88%97%E8%A1%A8",
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
