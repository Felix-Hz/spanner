// Settings: appearance, background recording, presets, about, uninstall. Every change saves immediately.

import { call } from "./ipc.js";
import { overview } from "./shared.js";
import { ago, h, toast, fill } from "./format.js";

const root = document.getElementById("app-settings");
const THEMES = [["system", "System"], ["light", "Light"], ["dark", "Dark"]];
let theme = "system";
let info = null;
let recorder = null; // { installed, running, start_at_sign_in, last_sample_ms, config }
let presets = [];

export function paintTheme(t) {
  theme = t;
  if (t === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = t;
}

function setTheme(t) {
  paintTheme(t);
  render();
  call("set_theme", { theme: t }).catch((e) => toast(`Couldn't save the theme. ${e.message}`));
}

// ── Recorder ──

async function saveRecorder(patch, message) {
  const config = { ...recorder.config, ...patch };
  try {
    await call("save_recorder", config);
    recorder.config = config;
    toast(message);
  } catch (e) {
    toast(e.message, { ms: 7000 });
  }
  render();
}

async function switchRecorder(patch, message) {
  try {
    await call("set_recorder", patch);
    document.dispatchEvent(new Event("recorder-changed"));
    toast(message);
  } catch (e) {
    toast(e.message, { ms: 7000 });
  }
  // The recorder takes a moment to start or stop.
  setTimeout(load, 800);
}

async function setUpRecorder() {
  try {
    await call("install_recorder");
    document.dispatchEvent(new Event("recorder-changed"));
    toast("Background recording is set up and on.");
  } catch (e) {
    toast(e.message, { ms: 7000 });
  }
  load();
}

// ── Presets ──

async function deletePreset(p) {
  try {
    presets = await call("delete_preset", { name: p.name });
    render();
    toast(`Deleted preset "${p.name}".`, {
      action: {
        label: "Undo",
        run: async () => {
          presets = await call("save_preset", p);
          render();
        },
      },
    });
  } catch (e) {
    toast(e.message, { ms: 7000 });
  }
}

// ── Uninstall ──

function uninstall() {
  const dlg = document.getElementById("uninstall-dialog");
  dlg.returnValue = "";
  dlg.showModal();
  dlg.addEventListener(
    "close",
    async () => {
      if (dlg.returnValue !== "confirm") return;
      const restore = document.getElementById("uninstall-restore").checked;
      toast("Uninstalling…", { ms: 15000 });
      try {
        const { problems } = await call("uninstall", { restore_defaults: restore });
        toast(problems.length ? `Uninstalled, but some items couldn't be removed: ${problems.join("; ")}` : "spanner is uninstalled. Closing…", { ms: 15000 });
      } catch (e) {
        toast(`Uninstall failed. ${e.message}`, { ms: 10000 });
      }
    },
    { once: true },
  );
}

// ── Building blocks ──

function row(name, help, control, hint = null) {
  return h(
    "div",
    { class: "row" },
    h("div", {}, h("h3", {}, name), help ? h("p", { class: "help" }, help) : null, hint ? h("p", { class: "hint" }, hint) : null),
    h("div", { class: "control" }, control),
  );
}

function seg(label, options, value, onPick) {
  return h(
    "div",
    { class: "seg", role: "radiogroup", "aria-label": label },
    options.map(([k, text]) => h("button", { type: "button", role: "radio", "aria-checked": String(k === value), onclick: () => onPick(k) }, text)),
  );
}

function switchControl(on, label, onToggle, disabled = false) {
  return h("button", { type: "button", class: "switch", role: "switch", "aria-checked": String(on), "aria-label": label, disabled, onclick: onToggle });
}

function numberField(label, value, min, max, unit, onCommit) {
  const input = h("input", {
    type: "number",
    min,
    max,
    value,
    "aria-label": label,
    onchange: (e) => {
      const n = Math.round(Number(e.target.value));
      if (Number.isFinite(n) && n >= min && n <= max) onCommit(n);
      else {
        toast(`${label} has to be between ${min} and ${max}.`);
        e.target.value = value;
      }
    },
  });
  return h("span", { class: "number-unit" }, input, unit);
}

function trackedApps(apps) {
  const name = h("input", { type: "text", placeholder: "Name, e.g. Planet Zoo", maxlength: 40, "aria-label": "App name (optional)" });
  const exe = h("input", { type: "text", placeholder: "Program file, e.g. PlanetZoo.exe", spellcheck: "false", "aria-label": "Program file name" });
  const add = () => {
    const file = exe.value.trim();
    if (!file) return exe.focus();
    if (apps.some((a) => a.exe.toLowerCase() === file.toLowerCase())) return toast(`${file} is already tracked.`);
    const app = { exe: file, ...(name.value.trim() ? { name: name.value.trim() } : {}) };
    saveRecorder({ tracked_apps: [...apps, app] }, `Tracking ${app.name ?? file}.`);
  };
  for (const input of [name, exe]) input.addEventListener("keydown", (e) => e.key === "Enter" && add());
  return h(
    "div",
    { class: "apps" },
    h(
      "div",
      { class: "app-list" },
      apps.length
        ? apps.map((a) =>
            h(
              "span",
              { class: "app-chip", title: a.exe },
              a.name ? [a.name, h("span", { class: "exe" }, a.exe)] : a.exe,
              h(
                "button",
                {
                  type: "button",
                  "aria-label": `Stop tracking ${a.name ?? a.exe}`,
                  title: "Stop tracking",
                  onclick: () => saveRecorder({ tracked_apps: apps.filter((x) => x !== a) }, `Stopped tracking ${a.name ?? a.exe}.`),
                },
                "×",
              ),
            ),
          )
        : h("span", { class: "meta" }, "No apps tracked."),
    ),
    h("div", { class: "add-app" }, name, exe, h("button", { class: "button", type: "button", onclick: add }, "Add")),
  );
}

function section(title, ...rows) {
  return h("section", { class: "section" }, h("div", { class: "section-head" }, h("h2", {}, title)), ...rows);
}

// ── Page ──

function recorderSection(r) {
  if (!r.installed) {
    return section(
      "Background recording",
      row("Background recording", "Takes a sample every few seconds so History has data, even when spanner is closed.", h("button", { class: "button primary", type: "button", onclick: setUpRecorder }, "Set up"), "Not set up yet."),
    );
  }
  const status = r.running
    ? r.last_sample_ms
      ? `On. Last sample ${ago(r.last_sample_ms)}.`
      : "On. Waiting for the first sample."
    : "Off. Nothing is being recorded.";
  const mb = Math.round(((86400 / r.config.interval_secs) * 40 * r.config.retention_days) / 1e6);
  return section(
    "Background recording",
    row(
      "Record in the background",
      `Takes a sample every ${r.config.interval_secs} seconds so History has data, even when spanner is closed.`,
      switchControl(r.running, "Record in the background", () => switchRecorder({ running: !r.running }, r.running ? "Background recording is off." : "Background recording is on.")),
      status,
    ),
    row(
      "Start when you sign in",
      "Turns background recording on each time you sign in to Windows.",
      switchControl(r.start_at_sign_in, "Start when you sign in", () =>
        switchRecorder({ start_at_sign_in: !r.start_at_sign_in }, r.start_at_sign_in ? "Recording won't start when you sign in." : "Recording will start when you sign in."),
      ),
    ),
    row(
      "Sample interval",
      "Shorter intervals give finer charts and use more disk space.",
      numberField("Sample interval", r.config.interval_secs, 2, 60, "seconds", (n) => saveRecorder({ interval_secs: n }, `Sampling every ${n} seconds.`)),
    ),
    row(
      "Keep history for",
      `About ${mb} MB at the current interval. Older samples are deleted automatically.`,
      numberField("Keep history for", r.config.retention_days, 1, 365, "days", (n) => saveRecorder({ retention_days: n }, `Keeping ${n} days of history.`)),
    ),
    row(
      "Tracked apps",
      "History can show only the time one of these was running. The program file is the name of its .exe.",
      trackedApps(r.config.tracked_apps),
    ),
  );
}

function presetsSection() {
  return section(
    "Presets",
    presets.length
      ? presets.map((p) =>
          row(p.name, `${Object.keys(p.values).length} settings`, h("button", { class: "button", type: "button", onclick: () => deletePreset(p) }, "Delete")),
        )
      : row("No presets yet", "On the Configure page, choose Presets, then Save current settings as preset.", null),
  );
}

function render() {
  fill(root, 
    section("Appearance", row("Theme", "System follows the Windows setting.", seg("Theme", THEMES, theme, setTheme))),
    recorder ? recorderSection(recorder) : null,
    presetsSection(),
    info
      ? section(
          "About",
          h(
            "dl",
            { class: "about" },
            h("dt", {}, "Version"),
            h("dd", {}, info.version),
            h("dt", {}, "Machine"),
            h("dd", {}, info.machine ?? "No vendor driver; generic settings only"),
            h("dt", {}, "Licence"),
            h("dd", {}, "MIT. Typefaces under the SIL Open Font License and MIT."),
          ),
        )
      : null,
    section(
      "Uninstall",
      row(
        "Uninstall spanner",
        "Removes background recording, recorded history, the Desktop shortcut and the entry in Installed apps.",
        h("button", { class: "button", type: "button", onclick: uninstall }, "Uninstall…"),
      ),
    ),
  );
}

async function load() {
  try {
    [info, recorder, presets] = await Promise.all([overview, call("recorder_status"), call("presets")]);
  } catch (e) {
    toast(`Couldn't read the settings. ${e.message}`, { ms: 7000 });
  }
  render();
}

export function show() {
  load();
}
