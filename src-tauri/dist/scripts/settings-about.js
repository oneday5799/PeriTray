/* settings-about.js — 设置页·关于 tab：版本按钮触发更新检测/infobar 与链接交互
 * 加载序 6/7 · 提供：initAboutTab()
 * 依赖：common.js(invoke) /
 *       settings.js(runUpdateCheck/hideUpdateErrorFlyout/renderUpdateInfobar) */
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
