const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const PROVIDERS = [
  { key: "claude", name: "Claude" },
  { key: "codex", name: "Codex" },
];

const $ = (id) => document.getElementById(id);
const refreshButton = $("refresh");
const auto = {
  card: $("auto-reset"),
  summary: $("auto-summary"),
  weeklyEnabled: $("weekly-enabled"),
  weeklyPercent: $("weekly-percent"),
  skipHours: $("skip-hours"),
  fiveEnabled: $("five-enabled"),
  fiveMinutes: $("five-minutes"),
  status: $("auto-status"),
};
const settingsInputs = [$("keep-open"), $("always-on-top"), auto.weeklyEnabled, auto.weeklyPercent, auto.skipHours, auto.fiveEnabled, auto.fiveMinutes];

function setSettingsDisabled(disabled) {
  $("compact-mode").disabled = disabled;
  $("window-options").disabled = disabled;
  for (const input of settingsInputs) input.disabled = disabled;
}
setSettingsDisabled(true);

let snapshot = null;
let spinTimer = null;
let confirmingReset = false;
let spendingReset = false;

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function button(text, className, onClick) {
  const node = el("button", className, text);
  node.type = "button";
  node.addEventListener("click", onClick);
  return node;
}

function clock(ms) {
  return new Date(ms).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

function day(ms) {
  return new Date(ms).toLocaleDateString([], { day: "numeric", month: "short" });
}

function resetText(ms) {
  const left = ms - Date.now();
  if (left <= 0) return "Resetting now";
  const minutes = Math.round(left / 60000);
  if (minutes < 60) return `Resets in ${minutes}m`;
  if (minutes < 24 * 60) return `Resets in ${Math.floor(minutes / 60)}h ${minutes % 60}m`;
  const when = new Date(ms).toLocaleString([], { weekday: "short", hour: "numeric", minute: "2-digit" });
  return `Resets ${when}`;
}

function meter(m) {
  const row = el("div", "meter");
  row.title = `${m.label}: ${Math.round(m.percent)}%${m.resetsAt ? ` · ${resetText(m.resetsAt)}` : ""}`;
  const top = el("div", "meter-top");
  top.append(el("span", "label", m.label), el("span", `pct ${m.severity}`, `${Math.round(m.percent)}%`));
  const bar = el("div", "bar");
  const fill = el("div", `fill ${m.severity}`);
  fill.style.width = `${Math.min(100, Math.max(0, m.percent))}%`;
  bar.append(fill);
  row.append(top, bar);
  if (m.resetsAt) row.append(el("div", "reset muted", resetText(m.resetsAt)));
  return row;
}

function footnote(key, data) {
  if (!data.asOf) return null;
  if (key === "codex" && !snapshot.codexResets.live) return `From your last Codex session, ${day(data.asOf)} ${clock(data.asOf)}`;
  if (data.error) return `Last good numbers from ${clock(data.asOf)}`;
  return null;
}

function resetsLabel(resets) {
  if (resets.available <= 0) return "No resets available";
  const count = resets.available === 1 ? "1 reset available" : `${resets.available} resets available`;
  return resets.nextExpiry ? `${count} · expires ${day(resets.nextExpiry)}` : count;
}

function resetsBlock(resets) {
  const block = el("div", "resets");
  if (confirmingReset) {
    block.append(el("p", "confirm-text", "Use a reset now? It refills your Codex limits and restarts your weekly clock."));
    const choices = el("div", "btns");
    choices.append(
      button("Cancel", "btn", () => {
        confirmingReset = false;
        render();
      }),
      button("Use it", "btn primary", useReset),
    );
    block.append(choices);
    return block;
  }
  const row = el("div", "reset-row");
  row.append(el("span", "muted", resetsLabel(resets)));
  if (resets.available > 0) {
    const use = button(spendingReset ? "Using…" : "Use reset", "btn", () => {
      confirmingReset = true;
      render();
    });
    use.disabled = spendingReset;
    row.append(use);
  }
  block.append(row);
  return block;
}

function eventLine(event) {
  return el("p", event.ok ? "event ok" : "event error", `${event.message} (${clock(event.at)})`);
}

function card(provider, data) {
  const section = el("section", "card");
  const head = el("div", "card-head");
  head.append(el("span", "card-name", provider.name), el("span", "plan", data.plan ?? ""));
  section.append(head);

  for (const m of data.meters) section.append(meter(m));
  if (data.meters.length === 0 && !data.error && !data.note) section.append(el("p", "note muted", "Loading…"));
  if (data.note) section.append(el("p", "note muted", data.note));
  if (data.error) section.append(el("p", "error", data.error));
  if (provider.key === "codex") {
    const resets = snapshot.codexResets;
    if (resets.live) section.append(resetsBlock(resets));
    if (resets.lastEvent) section.append(eventLine(resets.lastEvent));
  }
  const foot = footnote(provider.key, data);
  if (foot) section.append(el("p", "foot muted", foot));
  return section;
}

function renderAutoReset() {
  const resets = snapshot.codexResets;
  auto.card.hidden = !snapshot.codex.available;
  auto.status.textContent = resets.live ? resets.status ?? "" : "Paused until Codex's login works.";
  auto.status.hidden = !auto.status.textContent;
}

function fit() {
  requestAnimationFrame(() => {
    const height = Math.ceil($("app").getBoundingClientRect().height);
    invoke("fit_popover", { height });
  });
}

function render() {
  if (!snapshot) return;
  const cards = PROVIDERS.filter((p) => snapshot[p.key].available).map((p) => card(p, snapshot[p.key]));
  $("providers").replaceChildren(...cards);
  renderAutoReset();
  const asOf = snapshot.claude.asOf;
  $("updated").textContent = asOf ? `Updated ${clock(asOf)}` : "";
  fit();
}

async function useReset() {
  confirmingReset = false;
  spendingReset = true;
  render();
  try {
    snapshot.codexResets.lastEvent = await invoke("use_codex_reset");
  } catch (error) {
    snapshot.codexResets.lastEvent = { at: Date.now(), ok: false, message: String(error) };
  }
  spendingReset = false;
  render();
}

function summaryText(settings) {
  const parts = [];
  if (settings.weeklyEnabled) parts.push(`at ${settings.weeklyPercent}% weekly`);
  if (settings.fiveHourEnabled) parts.push(`5-hour block > ${settings.fiveHourMinutes} min`);
  return parts.length ? `On · ${parts.join(", ")}` : "Off";
}

function showSettings(settings) {
  document.body.classList.toggle("compact", settings.compactMode);
  document.body.classList.toggle("movable", settings.keepOpen);
  $("window-header").title = settings.keepOpen ? "Drag to move" : "";
  $("compact-mode").setAttribute("aria-pressed", String(settings.compactMode));
  $("compact-mode").title = settings.compactMode ? "Show full view" : "Compact mode";
  $("keep-open").checked = settings.keepOpen;
  $("always-on-top").checked = settings.alwaysOnTop;
  const a = settings.codexAutoReset;
  auto.weeklyEnabled.checked = a.weeklyEnabled;
  auto.weeklyPercent.value = a.weeklyPercent;
  auto.skipHours.value = a.skipWithinHours;
  auto.fiveEnabled.checked = a.fiveHourEnabled;
  auto.fiveMinutes.value = a.fiveHourMinutes;
  auto.summary.textContent = summaryText(a);
  fit();
}

async function saveSettings() {
  const codexAutoReset = {
    weeklyEnabled: auto.weeklyEnabled.checked,
    weeklyPercent: Number(auto.weeklyPercent.value),
    skipWithinHours: Number(auto.skipHours.value),
    fiveHourEnabled: auto.fiveEnabled.checked,
    fiveHourMinutes: Number(auto.fiveMinutes.value),
  };
  const settings = {
    codexAutoReset,
    keepOpen: $("keep-open").checked,
    alwaysOnTop: $("always-on-top").checked,
    compactMode: $("compact-mode").getAttribute("aria-pressed") === "true",
  };
  setSettingsDisabled(true);
  $("settings-error").hidden = true;
  try {
    showSettings(await invoke("save_settings", { settings }));
  } catch (error) {
    $("settings-error").textContent = String(error);
    $("settings-error").hidden = false;
    await invoke("get_settings").then(showSettings).catch(() => {});
  } finally {
    setSettingsDisabled(false);
    fit();
  }
}

for (const input of settingsInputs) {
  input.addEventListener("change", saveSettings);
}
auto.card.addEventListener("toggle", fit);
$("compact-mode").addEventListener("click", () => {
  const compact = $("compact-mode").getAttribute("aria-pressed") !== "true";
  $("compact-mode").setAttribute("aria-pressed", String(compact));
  confirmingReset = false;
  saveSettings();
});
$("window-header").addEventListener("mousedown", (event) => {
  if (event.button !== 0 || event.target.closest("button") || !$("keep-open").checked) return;
  event.preventDefault();
  invoke("drag_popover").catch((error) => {
    $("settings-error").textContent = `Couldn't move window: ${error}`;
    $("settings-error").hidden = false;
    fit();
  });
});

function stopSpin() {
  clearTimeout(spinTimer);
  refreshButton.classList.remove("spinning");
}

refreshButton.addEventListener("click", () => {
  refreshButton.classList.add("spinning");
  clearTimeout(spinTimer);
  spinTimer = setTimeout(stopSpin, 20000);
  invoke("refresh_now");
});

document.addEventListener("keydown", (event) => {
  if (event.key === "Escape") invoke("hide_popover");
});
document.addEventListener("contextmenu", (event) => event.preventDefault());
$("close").addEventListener("click", () => invoke("hide_popover"));

listen("usage-updated", (event) => {
  snapshot = event.payload;
  stopSpin();
  render();
});

invoke("get_settings").then((settings) => {
  showSettings(settings);
  setSettingsDisabled(false);
}).catch((error) => {
  $("settings-error").textContent = `Couldn't load settings: ${error}`;
  $("settings-error").hidden = false;
  fit();
});
invoke("get_snapshot").then((initial) => {
  snapshot = initial;
  render();
});

setInterval(render, 30000);
