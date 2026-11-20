// History: filters, one query, then one chart per metric with a shared hover cursor.

import { call, Superseded } from "./ipc.js";
import { overview } from "./shared.js";
import { debounce, fmtRange, h, prefs, toast, fill } from "./format.js";
import { chart } from "./chart.js";

const HOUR = 3_600_000;
const RANGES = [
  ["1h", "1 h", HOUR],
  ["6h", "6 h", 6 * HOUR],
  ["24h", "24 h", 24 * HOUR],
  ["7d", "7 days", 7 * 24 * HOUR],
  ["30d", "30 days", 30 * 24 * HOUR],
  ["custom", "Custom", null],
];
const POWER = [["any", "Any power"], ["ac", "Plugged in"], ["battery", "On battery"]];
const MIN_SPAN = 5 * 60_000;
const MAX_SPAN = 400 * 24 * HOUR;
const DEFAULT_METRICS = ["cpu_temp", "gpu_temp", "gpu_limited", "cpu_fan", "gpu_power", "cpu_util"];

/** "Data from" values: everything, any tracked app, or one app by display name. */
const ALL = "all";
const ANY_APP = "any-app";
const appKey = (label) => `app:${label}`;

const state = {
  range: prefs.get("history.range", "24h"),
  from: null,
  to: null,
  power: prefs.get("history.power", "any"),
  source: prefs.get("history.source", ALL),
  metrics: new Set(prefs.get("history.metrics", DEFAULT_METRICS)),
  /** A zoomed window, overriding the range until reset. */
  zoom: null,
};

let catalogue = [];
let built = false;
let visible = false;
let charts = [];
let intervalSecs = 5;
/** Tracked apps grouped by display name (two executables can be one game). */
let apps = []; // [{ label, exes: [..], samples }]
const chartsEl = document.getElementById("charts");

function save() {
  prefs.set("history.range", state.range);
  prefs.set("history.power", state.power);
  prefs.set("history.source", state.source);
  prefs.set("history.metrics", [...state.metrics]);
}

function seg(name, options, value, onPick) {
  return h(
    "div",
    { class: "seg", role: "radiogroup", "aria-label": name },
    options.map(([key, label]) => h("button", { type: "button", role: "radio", "aria-checked": String(key === value), onclick: () => onPick(key) }, label)),
  );
}

const toLocalInput = (ms) => new Date(ms - new Date(ms).getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
const fromLocalInput = (v) => (v ? new Date(v).getTime() : null);

/** "2 h 15 min", "40 min", "under a minute". */
function duration(samples) {
  const mins = Math.round((samples * intervalSecs) / 60);
  if (mins < 1) return "under a minute";
  if (mins < 60) return `${mins} min`;
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

function changed() {
  build();
  save();
  query();
}

function sourceSelect() {
  const sel = h(
    "select",
    {
      id: "history-source",
      "aria-label": "Data from",
      onchange: (e) => {
        state.source = e.target.value;
        save();
        query();
      },
    },
    h("option", { value: ALL, selected: state.source === ALL }, "All the time"),
    h("option", { value: ANY_APP, selected: state.source === ANY_APP }, "While any tracked app is running"),
    apps.length
      ? h(
          "optgroup",
          { label: "While this app is running" },
          apps.map((a) =>
            h(
              "option",
              { value: appKey(a.label), selected: state.source === appKey(a.label), disabled: a.samples === 0 && state.source !== appKey(a.label) },
              a.samples ? `${a.label} · ${duration(a.samples)}` : `${a.label} · no data in this range`,
            ),
          ),
        )
      : null,
  );
  return h("label", { class: "inline-field" }, h("span", {}, "Data from"), sel);
}

// ── Zoom ──

/** Zooms the current window by `factor` (below 1 zooms in) around a point given as a 0..1 fraction. */
function zoomBy(factor, at = 0.5) {
  const [from, to] = timeWindow();
  const span = Math.min(MAX_SPAN, Math.max(MIN_SPAN, (to - from) * factor));
  const pivot = from + (to - from) * at;
  let next = [pivot - span * at, pivot + span * (1 - at)];
  // Never past now: shift back instead of showing empty future.
  const now = Date.now();
  if (next[1] > now) next = [next[0] - (next[1] - now), now];
  setZoom(next);
}

function setZoom(window) {
  state.zoom = window;
  buildZoom();
  query();
}

function resetZoom() {
  if (!state.zoom) return;
  setZoom(null);
}

function buildZoom() {
  fill(
    document.getElementById("zoom"),
    h("button", { type: "button", class: "button", title: "Zoom out (−)", "aria-label": "Zoom out", onclick: () => zoomBy(2) }, "−"),
    h("button", { type: "button", class: "button", title: "Zoom in (+)", "aria-label": "Zoom in", onclick: () => zoomBy(0.5) }, "+"),
    state.zoom ? h("button", { type: "button", class: "button", title: "Back to the selected range (0)", onclick: resetZoom }, "Reset zoom") : null,
  );
}

document.addEventListener("keydown", (e) => {
  if (!visible || e.ctrlKey || e.altKey || e.metaKey || e.target.closest("input, select, textarea")) return;
  if (e.key === "+" || e.key === "=") zoomBy(0.5);
  else if (e.key === "-") zoomBy(2);
  else if (e.key === "0") resetZoom();
});

function build() {
  fill(
    document.getElementById("range"),
    seg("Time range", RANGES, state.range, (k) => {
      if (k === "custom" && state.from == null) [state.from, state.to] = [Date.now() - 24 * HOUR, Date.now()];
      state.range = k;
      state.zoom = null;
      changed();
    }),
    h("span", { class: "zoom", id: "zoom", role: "group", "aria-label": "Zoom" }),
  );
  buildZoom();

  const custom =
    state.range === "custom"
      ? h(
          "div",
          { class: "range-custom" },
          h(
            "label",
            { class: "inline-field" },
            h("span", {}, "From"),
            h("input", { type: "datetime-local", value: toLocalInput(state.from), onchange: (e) => ((state.from = fromLocalInput(e.target.value)), query()) }),
          ),
          h(
            "label",
            { class: "inline-field" },
            h("span", {}, "To"),
            h("input", { type: "datetime-local", value: toLocalInput(state.to), onchange: (e) => ((state.to = fromLocalInput(e.target.value)), query()) }),
          ),
        )
      : null;

  const groups = [...new Set(catalogue.map((m) => m.group))];
  fill(document.getElementById("filters"),
    custom,
    sourceSelect(),
    seg("Power source", POWER, state.power, (k) => ((state.power = k), changed())),
    h(
      "div",
      { class: "chips", role: "group", "aria-label": "Charts to show" },
      groups.flatMap((g) =>
        catalogue
          .filter((m) => m.group === g)
          .map((m) =>
            h(
              "button",
              {
                type: "button",
                class: "chip",
                "aria-pressed": String(state.metrics.has(m.id)),
                title: g,
                onclick: () => {
                  if (state.metrics.has(m.id)) {
                    if (state.metrics.size === 1) return toast("At least one chart has to stay on.");
                    state.metrics.delete(m.id);
                  } else state.metrics.add(m.id);
                  changed();
                },
              },
              m.label,
            ),
          ),
      ),
    ),
  );
}

function timeWindow() {
  if (state.zoom) return state.zoom;
  if (state.range !== "custom") {
    const span = RANGES.find(([k]) => k === state.range)[2];
    const to = Date.now();
    return [to - span, to];
  }
  return [state.from, state.to];
}

/** Query parameters for the "Data from" choice. */
function sourceParams() {
  if (state.source === ANY_APP) return { tracked_only: true, apps: [] };
  const app = apps.find((a) => appKey(a.label) === state.source);
  if (app) return { tracked_only: false, apps: app.exes };
  return { tracked_only: false, apps: [] };
}

function sourceLabel() {
  if (state.source === ANY_APP) return "while any tracked app was running";
  const app = apps.find((a) => appKey(a.label) === state.source);
  return app ? `while ${app.label} was running` : null;
}

async function query() {
  if (!visible) return;
  const [from_ms, to_ms] = timeWindow();
  if (!(from_ms < to_ms)) return toast("The end of the range has to be after the start.");
  const width = Math.max(320, chartsEl.clientWidth);
  // One bucket per pixel of plot width: the finest detail the chart can show.
  const buckets = Math.min(2000, Math.round(width - 44));
  const metrics = catalogue.map((m) => m.id).filter((id) => state.metrics.has(id));
  chartsEl.setAttribute("aria-busy", "true");
  try {
    const res = await call("history", { from_ms, to_ms, buckets, metrics, power: state.power, ...sourceParams() });
    updateApps(res.apps);
    render(res, width);
  } catch (e) {
    if (!(e instanceof Superseded)) fill(chartsEl, h("p", { class: "empty" }, `Couldn't read the history. ${e.message}`));
  } finally {
    chartsEl.setAttribute("aria-busy", "false");
  }
}

/** Groups the response's per-executable usage by display name and refreshes the picker. */
function updateApps(usage) {
  const byLabel = new Map();
  for (const u of usage) {
    const label = u.name ?? u.exe;
    const a = byLabel.get(label) ?? { label, exes: [], samples: 0 };
    a.exes.push(u.exe);
    a.samples += u.samples;
    byLabel.set(label, a);
  }
  apps = [...byLabel.values()];
  // A previously chosen app that is no longer tracked falls back to everything.
  if (state.source.startsWith("app:") && !apps.some((a) => appKey(a.label) === state.source)) state.source = ALL;
  const old = document.getElementById("history-source")?.closest("label");
  if (old) old.replaceWith(sourceSelect());
}

function render(res, width) {
  const sub = document.getElementById("history-sub");
  const scope = sourceLabel();
  sub.textContent = [fmtRange(res.from_ms, res.to_ms), state.zoom ? "zoomed" : null, scope, res.samples ? `${res.samples.toLocaleString("en-NZ")} samples` : null].filter(Boolean).join(" · ");
  if (res.samples === 0) {
    charts = [];
    const filtered = state.source !== ALL || state.power !== "any";
    fill(chartsEl, h("p", { class: "empty" }, filtered ? "Nothing matches these filters in this range." : "No samples in this range."));
    return;
  }
  const span = res.to_ms - res.from_ms;
  const handlers = {
    hover: (i) => charts.forEach((c) => c.setCursor(i)),
    select: (a, b) => setZoom([res.from_ms + a * span, res.from_ms + Math.max(b * span, a * span + MIN_SPAN)]),
    zoom: (at, factor) => zoomBy(factor, at),
  };
  charts = res.series.map((sr) => chart(sr, res, width, handlers));
  fill(chartsEl, ...charts.map((c) => c.el));
}

new ResizeObserver(debounce(() => visible && query(), 200)).observe(chartsEl);

export async function show() {
  visible = true;
  if (!built) {
    try {
      catalogue = (await overview).metrics;
      intervalSecs = (await call("recorder_status")).config.interval_secs;
    } catch (e) {
      toast(e.message);
    }
    for (const id of [...state.metrics]) if (!catalogue.some((m) => m.id === id)) state.metrics.delete(id);
    if (state.metrics.size === 0) DEFAULT_METRICS.forEach((id) => state.metrics.add(id));
    build();
    built = true;
  }
  query();
}

export function hide() {
  visible = false;
}
