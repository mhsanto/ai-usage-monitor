const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const PROVIDERS = [
  { key: "claude", name: "Claude" },
  { key: "codex", name: "Codex" },
];

const refreshButton = document.getElementById("refresh");
let snapshot = null;
let spinTimer = null;

function el(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
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
  if (key === "codex") return `From your last Codex session, ${day(data.asOf)} ${clock(data.asOf)}`;
  if (data.error) return `Last good numbers from ${clock(data.asOf)}`;
  return null;
}

function card(provider, data) {
  const section = el("section", "card");
  const head = el("div", "card-head");
  head.append(el("span", "card-name", provider.name), el("span", "plan", data.plan ?? ""));
  section.append(head);

  for (const m of data.meters) section.append(meter(m));
  if (data.meters.length === 0 && !data.error) section.append(el("p", "note muted", data.note ?? "Loading…"));
  if (data.error) section.append(el("p", "error", data.error));
  const foot = footnote(provider.key, data);
  if (foot) section.append(el("p", "foot muted", foot));
  return section;
}

function render() {
  if (!snapshot) return;
  const cards = PROVIDERS.filter((p) => snapshot[p.key].available).map((p) => card(p, snapshot[p.key]));
  document.getElementById("providers").replaceChildren(...cards);
  const asOf = snapshot.claude.asOf;
  document.getElementById("updated").textContent = asOf ? `Updated ${clock(asOf)}` : "";
  requestAnimationFrame(() => {
    const height = Math.ceil(document.getElementById("app").getBoundingClientRect().height);
    invoke("fit_popover", { height });
  });
}

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

listen("usage-updated", (event) => {
  snapshot = event.payload;
  stopSpin();
  render();
});

invoke("get_snapshot").then((initial) => {
  snapshot = initial;
  render();
});

setInterval(render, 30000);
