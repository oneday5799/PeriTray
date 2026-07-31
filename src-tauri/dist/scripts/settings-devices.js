const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

let config = null;
let devices = [];
let expandedGroups = new Set();
let deviceGroups = {};

async function loadDevicesAsync() {
  try {
    config = await invoke("get_config");
    devices = await invoke("get_devices");
    deviceGroups = config.device_groups || {};
    renderGroups();
  } catch (e) {
    console.error("Failed to load devices:", e);
  }
}

function renderGroups() {
  const container = document.getElementById("device-groups");
  container.innerHTML = "";

  const groups = {};
  for (const d of devices) {
    const group = deviceGroups[d.name] || d.dt;
    if (!groups[group]) groups[group] = [];
    groups[group].push(d);
  }

  for (const group of CATEGORIES) {
    const devs = groups[group.key] || [];
    const groupEl = document.createElement("div");
    groupEl.className = "group";

    const card = document.createElement("div");
    card.className = "group-card";

    const header = document.createElement("div");
    header.className = "group-header";

    const icon = document.createElement("div");
    icon.className = "group-icon";
    icon.textContent = group.icon;
    header.appendChild(icon);

    const textWrap = document.createElement("div");
    textWrap.className = "group-text";

    const title = document.createElement("div");
    title.className = "group-title";
    title.textContent = group.label;
    textWrap.appendChild(title);

    const subtitle = document.createElement("div");
    subtitle.className = "group-subtitle";
    subtitle.textContent = group.subtitle;
    textWrap.appendChild(subtitle);

    header.appendChild(textWrap);

    const groupToggle = document.createElement("label");
    groupToggle.className = "toggle group-toggle";
    const isGroupHidden = config.hidden_groups.includes(group.key);
    const groupInput = document.createElement("input");
    groupInput.type = "checkbox";
    groupInput.checked = !isGroupHidden;

    groupToggle.addEventListener("click", (e) => {
      e.stopPropagation();
    });
    groupInput.addEventListener("change", async (e) => {
      e.stopPropagation();
      await invoke("toggle_group_hidden", { group: group.key });
      const cfg = await invoke("get_config");
      config.hidden_groups = cfg.hidden_groups || [];
      renderGroups();
    });

    const groupSlider = document.createElement("span");
    groupSlider.className = "slider";
    groupToggle.appendChild(groupInput);
    groupToggle.appendChild(groupSlider);
    header.appendChild(groupToggle);

    const arrow = document.createElement("div");
    arrow.className = "group-arrow";
    arrow.innerHTML = `<svg width="12" height="12" viewBox="0 0 12 12" fill="none"><path d="M4 2L8 6L4 10" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
    header.appendChild(arrow);

    const items = document.createElement("div");
    items.className = "group-items";

    header.addEventListener("click", () => {
      const isExpanded = items.classList.toggle("show");
      arrow.classList.toggle("expanded", isExpanded);
      if (isExpanded) {
        expandedGroups.add(group.key);
        items.style.maxHeight = items.scrollHeight + "px";
      } else {
        expandedGroups.delete(group.key);
        items.style.maxHeight = "0px";
      }
    });

    if (expandedGroups.has(group.key)) {
      items.classList.add("show");
      arrow.classList.add("expanded");
    }

    for (const dev of devs) {
      const item = document.createElement("div");
      item.className = "device-item";

      const nameEl = document.createElement("div");
      nameEl.className = "device-item-name";
      nameEl.textContent = dev.name;

      const isHidden = config.hidden_devices.includes(dev.name);
      if (isHidden) nameEl.classList.add("hidden");

      const toggle = document.createElement("label");
      toggle.className = "toggle";

      const input = document.createElement("input");
      input.type = "checkbox";
      input.checked = !isHidden;
      input.addEventListener("change", async () => {
        await invoke("toggle_device_hidden", { name: dev.name });
        config = await invoke("get_config");
        nameEl.classList.toggle("hidden", !input.checked);
      });

      const slider = document.createElement("span");
      slider.className = "slider";

      toggle.appendChild(input);
      toggle.appendChild(slider);

      item.appendChild(nameEl);
      item.appendChild(toggle);
      items.appendChild(item);
    }

    card.appendChild(header);
    card.appendChild(items);
    groupEl.appendChild(card);
    container.appendChild(groupEl);

    if (expandedGroups.has(group.key)) {
      requestAnimationFrame(() => {
        items.style.maxHeight = items.scrollHeight + "px";
      });
    }
  }
}
