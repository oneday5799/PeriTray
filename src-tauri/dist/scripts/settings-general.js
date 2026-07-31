function initShortcutSettings() {
  const hintEl = document.getElementById("shortcut-hint");
  let hintTimer = null;

  function showHint(msg, isError) {
    clearTimeout(hintTimer);
    hintEl.textContent = msg;
    hintEl.style.color = isError ? "#e81123" : "#999";
    hintTimer = setTimeout(() => {
      hintEl.textContent = "点击输入框后按下键盘组合键录制快捷键，如 Ctrl+Shift+D";
      hintEl.style.color = "#999";
    }, 3000);
  }

  const actions = [
    { id: "devices", inputId: "shortcut-devices", clearId: "clear-shortcut-devices", configKey: "shortcut_devices" },
    { id: "volume", inputId: "shortcut-volume", clearId: "clear-shortcut-volume", configKey: "shortcut_volume" },
  ];

  const codeMap = {
    "Space": { display: "Space", key: "Space" },
    "Backspace": { display: "Backspace", key: "Backspace" },
    "Delete": { display: "Delete", key: "Delete" },
    "Tab": { display: "Tab", key: "Tab" },
    "CapsLock": { display: "CapsLock", key: "CapsLock" },
    "Escape": { display: "Escape", key: "Escape" },
    "Insert": { display: "Insert", key: "Insert" },
    "Home": { display: "Home", key: "Home" },
    "End": { display: "End", key: "End" },
    "PageUp": { display: "PageUp", key: "PageUp" },
    "PageDown": { display: "PageDown", key: "PageDown" },
    "ArrowUp": { display: "↑", key: "ArrowUp" },
    "ArrowDown": { display: "↓", key: "ArrowDown" },
    "ArrowLeft": { display: "←", key: "ArrowLeft" },
    "ArrowRight": { display: "→", key: "ArrowRight" },
    "PrintScreen": { display: "PrtSc", key: "PrintScreen" },
    "ScrollLock": { display: "ScrLk", key: "ScrollLock" },
    "Pause": { display: "Pause", key: "Pause" },
    "NumLock": { display: "NumLock", key: "NumLock" },
    "NumpadAdd": { display: "Num+", key: "NumpadAdd" },
    "NumpadSubtract": { display: "Num-", key: "NumpadSubtract" },
    "NumpadMultiply": { display: "Num*", key: "NumpadMultiply" },
    "NumpadDivide": { display: "Num/", key: "NumpadDivide" },
    "NumpadDecimal": { display: "Num.", key: "NumpadDecimal" },
    "NumpadEnter": { display: "NumEnter", key: "NumpadEnter" },
    "MediaPlayPause": { display: "MediaPlayPause", key: "MediaPlayPause" },
    "MediaStop": { display: "MediaStop", key: "MediaStop" },
    "MediaNextTrack": { display: "MediaNextTrack", key: "MediaNextTrack" },
    "MediaPrevTrack": { display: "MediaPrevTrack", key: "MediaPrevTrack" },
    "VolumeUp": { display: "VolumeUp", key: "VolumeUp" },
    "VolumeDown": { display: "VolumeDown", key: "VolumeDown" },
    "VolumeMute": { display: "VolumeMute", key: "VolumeMute" },
    "Semicolon": { display: ";", key: "Semicolon" },
    "Equal": { display: "=", key: "Equal" },
    "Comma": { display: ",", key: "Comma" },
    "Period": { display: ".", key: "Period" },
    "Slash": { display: "/", key: "Slash" },
    "Backquote": { display: "`", key: "Backquote" },
    "Backslash": { display: "\\", key: "Backslash" },
    "BracketLeft": { display: "[", key: "BracketLeft" },
    "BracketRight": { display: "]", key: "BracketRight" },
    "Minus": { display: "-", key: "Minus" },
    "Quote": { display: "'", key: "Quote" },
    "Enter": { display: "Enter", key: "Enter" },
  };
  const reverseCodeMap = {};
  for (const v of Object.values(codeMap)) {
    reverseCodeMap[v.key] = v.display;
  }

  function joinSaved(saved) {
    if (!saved) return "";
    return saved.split("+").map(p => reverseCodeMap[p] || p).join("+");
  }

  for (const action of actions) {
    const input = document.getElementById(action.inputId);
    const clearBtn = document.getElementById(action.clearId);

    const keyField = action.configKey;
    const savedKey = config[keyField];
    if (savedKey) {
      const display = joinSaved(savedKey);
      input.value = display.replace("Super", "Win");
      clearBtn.style.display = "";
    } else {
      input.value = "";
      clearBtn.style.display = "none";
    }

    let recording = false;
    let keys = new Set();

    function resetRecording() {
      recording = false;
      keys.clear();
      input.classList.remove("recording");
      input.placeholder = "点击录制快捷键";
    }

    function restoreSaved() {
      resetRecording();
      input.value = savedKey ? joinSaved(savedKey).replace("Super", "Win") : "";
      clearBtn.style.display = savedKey ? "" : "none";
    }

    input.addEventListener("click", () => {
      if (recording) return;
      recording = true;
      keys.clear();
      input.value = "";
      input.classList.add("recording");
      input.placeholder = "请按下组合键...";
    });

    input.addEventListener("blur", () => {
      resetRecording();
    });

    input.addEventListener("keydown", (e) => {
      if (!recording) return;
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        resetRecording();
        input.value = savedKey || "";
        input.placeholder = "点击录制快捷键";
        clearBtn.style.display = savedKey ? "" : "none";
        return;
      }

      keys.clear();
      if (e.ctrlKey) keys.add({ display: "Ctrl", key: "Ctrl" });
      if (e.shiftKey) keys.add({ display: "Shift", key: "Shift" });
      if (e.altKey) keys.add({ display: "Alt", key: "Alt" });
      if (e.metaKey) keys.add({ display: "Win", key: "Super" });

      const code = e.code;
      if (code === "ControlLeft" || code === "ControlRight" ||
          code === "ShiftLeft" || code === "ShiftRight" ||
          code === "AltLeft" || code === "AltRight" ||
          code === "MetaLeft" || code === "MetaRight") {
        const preview = [...keys].map(k => k.display).join("+");
        input.value = preview;
        return;
      }

      if (code.startsWith("Numpad") && /\d/.test(code[6]) && code.length === 7) {
        restoreSaved();
        showHint("暂不支持该快捷键。", true);
        return;
      }
      if (codeMap[code]) {
        const entry = codeMap[code];
        keys.add({ display: entry.display, key: entry.key });
      } else if (code.startsWith("F") && code.length >= 2 && code.length <= 3) {
        keys.add({ display: code, key: code });
      } else if (code.startsWith("Digit") && code.length === 6) {
        keys.add({ display: code, key: code });
      } else if (code.startsWith("Key") && code.length === 4) {
        keys.add({ display: code[3], key: code });
      } else {
        restoreSaved();
        showHint("暂不支持该快捷键。", true);
        return;
      }

      const display = [...keys].map(k => k.display).join("+");
      const shortcut = [...keys].map(k => k.key).join("+");
      if (display) {
        recording = false;
        input.value = display;
        input.classList.remove("recording");
        input.placeholder = "点击录制快捷键";

        registerShortcut(action.id, display, shortcut, input, clearBtn);
      }
    });

    clearBtn.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        await invoke("set_hotkey_config", { action: action.id, key: null });
        config[keyField] = null;
        input.value = "";
        clearBtn.style.display = "none";
        input.placeholder = "点击录制快捷键";
      } catch (_) {}
    });

    async function registerShortcut(actionId, display, shortcut, inputEl, clearEl) {
      try {
        const configKey = shortcut.replace("Win", "Super");
        await invoke("set_hotkey_config", {
          action: actionId,
          key: configKey,
        });
        config[keyField] = configKey;
        clearEl.style.display = "";
        await invoke("update_config", { newConfig: config });
        showHint(`快捷键 "${display}" 已保存`, false);
      } catch (err) {
        const msg = String(err);
        if (msg.includes("已被占用")) {
          showHint(`"${display}" 已被其他功能占用，请选择其他快捷键。`, true);
        } else {
          showHint("暂不支持该快捷键。", true);
        }
        config[keyField] = null;
        inputEl.value = "";
        clearEl.style.display = "none";
      }
    }
  }
}
