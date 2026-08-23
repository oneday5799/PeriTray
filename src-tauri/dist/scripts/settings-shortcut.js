/* settings-shortcut.js — 设置页·快捷键 tab：基础快捷键录制绑定/设备快捷键共享切换/设备快捷键列表
 * 加载序 3/7 · 提供：initShortcutSettings() / initDeviceShortcutSettings()
 * 依赖：common.js(invoke/bindShortcutRecorder/describeShortcutError/attachTooltip) /
 *       settings.js(config/bindToggle/saveConfig/showToast) */
function initShortcutSettings() {
  bindToggle("toggle-device-shortcut-cycle", {
    get: () => config.enable_device_shortcut_cycle,
    set: (v) => { config.enable_device_shortcut_cycle = v; }
  });

  const actions = [
    { id: "devices", inputId: "shortcut-devices", clearId: "clear-shortcut-devices", configKey: "shortcut_devices" },
    { id: "volume", inputId: "shortcut-volume", clearId: "clear-shortcut-volume", configKey: "shortcut_volume" },
    { id: "volume_up", inputId: "shortcut-volume-up", clearId: "clear-shortcut-volume-up", configKey: "shortcut_volume_up" },
    { id: "volume_down", inputId: "shortcut-volume-down", clearId: "clear-shortcut-volume-down", configKey: "shortcut_volume_down" },
    { id: "volume_mute", inputId: "shortcut-volume-mute", clearId: "clear-shortcut-volume-mute", configKey: "shortcut_volume_mute" },
  ];

  for (const action of actions) {
    const input = document.getElementById(action.inputId);
    const clearBtn = document.getElementById(action.clearId);
    const keyField = action.configKey;

    bindShortcutRecorder(
      input,
      clearBtn,
      () => config[keyField],
      (display, shortcut) => {
        if (shortcut === "") {
          invoke("set_hotkey_config", { action: action.id, key: null }).catch(() => {});
          config[keyField] = null;
          input.value = "";
          clearBtn.style.display = "none";
          input.placeholder = "点击录制快捷键";
          return;
        }
        invoke("set_hotkey_config", { action: action.id, key: shortcut })
          .then(async () => {
            config[keyField] = shortcut;
            clearBtn.style.display = "";
            await saveConfig();
            showToast(`快捷键 "${display}" 已保存`);
          })
          .catch((err) => {
            config[keyField] = null;
            input.value = "";
            clearBtn.style.display = "none";
            showToast(describeShortcutError(err, display), null, true);
          });
      }
    );
  }
}

function initDeviceShortcutSettings() {
  const listEl = document.getElementById("device-shortcut-list");

  function render() {
    const shortcuts = config.device_shortcuts || {};
    const ids = Object.keys(shortcuts);
    listEl.innerHTML = "";

    if (ids.length === 0) {
      const empty = document.createElement("div");
      empty.className = "shortcut-hint";
      empty.textContent = "暂无设备切换快捷键。可在「音量控制」页右键设备选择「快捷键」进行设置。";
      listEl.appendChild(empty);
      return;
    }

    for (const id of ids) {
      const entry = shortcuts[id];
      const item = document.createElement("div");
      item.className = "card";

      const left = document.createElement("div");
      left.className = "card-left";

      const label = document.createElement("span");
      label.className = "card-title device-shortcut-label";
      label.textContent = entry.name;
      window.attachTooltip(label, entry.name, "start");
      left.appendChild(label);
      item.appendChild(left);

      const actions = document.createElement("div");
      actions.className = "card-actions";

      const wrap = document.createElement("div");
      wrap.className = "shortcut-input-wrap";

      const input = document.createElement("input");
      input.type = "text";
      input.className = "shortcut-key-input";
      input.placeholder = "点击录制快捷键";
      input.readOnly = true;

      const clearBtn = document.createElement("button");
      clearBtn.className = "shortcut-clear-btn";
      clearBtn.textContent = "×";
      window.attachTooltip(clearBtn, "清除快捷键", "end");

      const deleteBtn = document.createElement("button");
      deleteBtn.className = "shortcut-delete-btn";
      deleteBtn.textContent = "删除";
      window.attachTooltip(deleteBtn, "删除此设备快捷键", "end");
      deleteBtn.addEventListener("click", async () => {
        try {
          await invoke("remove_device_shortcut", { deviceId: id });
          config = await invoke("get_config");
          render();
        } catch (e) {
          console.error("Failed to remove device shortcut:", e);
        }
      });

      const box = document.createElement("div");
      box.className = "shortcut-key-box";
      box.appendChild(input);
      box.appendChild(clearBtn);

      wrap.appendChild(box);
      wrap.appendChild(deleteBtn);
      actions.appendChild(wrap);
      item.appendChild(actions);
      listEl.appendChild(item);

      bindShortcutRecorder(
        input,
        clearBtn,
        () => (config.device_shortcuts[id] || {}).shortcut || null,
        (display, shortcut) => {
          if (shortcut === "") {
            invoke("set_device_shortcut", { deviceId: id, name: entry.name, key: null }).catch(() => {});
            config = { ...config };
            config.device_shortcuts = { ...config.device_shortcuts };
            config.device_shortcuts[id] = { ...config.device_shortcuts[id], shortcut: null };
            input.value = "";
            clearBtn.style.display = "none";
            input.placeholder = "点击录制快捷键";
            return;
          }
          invoke("set_device_shortcut", { deviceId: id, name: entry.name, key: shortcut })
            .then(async () => {
              config = await invoke("get_config");
              render();
            })
            .catch((err) => {
              showToast(describeShortcutError(err, display), null, true);
              render();
            });
        }
      );
    }
  }

  render();
}