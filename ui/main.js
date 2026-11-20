import { call } from "./ipc.js";
import { overview } from "./shared.js";
import { ago, h, prefs, fill } from "./format.js";
import * as configure from "./configure.js";
import * as live from "./live.js";
import * as history from "./history.js";
import * as settings from "./settings.js";
import { icon } from "./icons.js";

const pages = { configure, live, history, "settings-page": settings };
const tabs = [...document.querySelectorAll('[role="tab"]')];
const ICONS = { configure: "configure", live: "live", history: "history", "settings-page": "settings" };
for (const t of tabs) {
  const label = t.querySelector("span");
  label.classList.add("label");
  label.prepend(icon(ICONS[t.getAttribute("aria-controls")]));
}
let current = null;

// ── Navigation ──

function select(name, focus = false) {
  if (!(name in pages) || name === current) return;
  if (current) pages[current].hide?.();
  for (const t of tabs) {
    const on = t.getAttribute("aria-controls") === name;
    t.setAttribute("aria-selected", String(on));
    t.tabIndex = on ? 0 : -1;
    document.getElementById(t.getAttribute("aria-controls")).hidden = !on;
    if (on && focus) t.focus();
  }
  current = name;
  prefs.set("page", name);
  pages[name].show();
}

for (const t of tabs) t.addEventListener("click", () => select(t.getAttribute("aria-controls")));

document.querySelector('[role="tablist"]').addEventListener("keydown", (e) => {
  const i = tabs.indexOf(document.activeElement);
  if (i < 0) return;
  const next = { ArrowDown: i + 1, ArrowUp: i - 1, Home: 0, End: tabs.length - 1 }[e.key];
  if (next == null) return;
  e.preventDefault();
  select(tabs[(next + tabs.length) % tabs.length].getAttribute("aria-controls"), true);
});

document.addEventListener("keydown", (e) => {
  if (e.ctrlKey && !e.altKey && ["1", "2", "3", "4"].includes(e.key)) {
    e.preventDefault();
    select(Object.keys(pages)[Number(e.key) - 1], true);
  }
});

document.addEventListener("visibilitychange", () => {
  if (!current) return;
  if (document.hidden) pages[current].hide?.();
  else pages[current].show();
});

// ── Window behaviour: an app, not a web page ──

document.addEventListener("contextmenu", (e) => {
  if (!e.target.closest("input, textarea")) e.preventDefault();
});
document.addEventListener("dragstart", (e) => e.preventDefault());
document.addEventListener("auxclick", (e) => e.preventDefault());

// ── Status bar ──

const sb = {
  power: document.getElementById("sb-power"),
  recorder: document.getElementById("sb-recorder"),
  plan: document.getElementById("sb-plan"),
  machine: document.getElementById("sb-machine"),
};
let recorderInstalled = true;

async function refreshStatus() {
  try {
    const st = await call("status");
    const battery = st.battery_pct == null ? "" : ` · ${st.battery_pct}%`;
    sb.power.textContent = `${st.on_ac ? "Plugged in" : "On battery"}${battery}`;
    sb.plan.textContent = st.power_profile ? `${st.power_profile} plan` : "";
    fill(sb.recorder, 
      !recorderInstalled
        ? "Background recording not set up"
        : !st.recorder_running
          ? "Background recording off"
          : st.last_sample_ms == null
            ? "Recording · waiting for first sample"
            : h("span", { class: "ok" }, h("span", { class: "dot" }), `Recording · last sample ${ago(st.last_sample_ms)}`),
    );
  } catch {
    /* transient; the next tick retries */
  }
}

// ── Start ──

overview.then((o) => {
  settings.paintTheme(o.theme);
  recorderInstalled = o.recorder_installed;
  sb.machine.textContent = o.machine ?? "No vendor driver for this machine";
  refreshStatus();
});

document.addEventListener("recorder-changed", () => {
  recorderInstalled = true;
  refreshStatus();
});
setInterval(() => !document.hidden && refreshStatus(), 5000);

select(prefs.get("page", "configure"));
