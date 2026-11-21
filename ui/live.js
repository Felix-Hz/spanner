// Live: reads at the chosen rate while visible; each tile keeps the last 120 readings as a trend.

import { call } from "./ipc.js";
import { overview } from "./shared.js";
import { auto, collapsible, fmtClock, h, NONE, num, prefs, toast, fill } from "./format.js";

const RATES = [1, 5, 15, 30, 60];
const CUSTOM_RANGE = [1, 600];
const WINDOW = 120;
let periodSecs = prefs.get("live.period", 5);

/** Fixed-size ring buffer: O(1) push, no allocation per sample. */
class Ring {
  constructor(n) {
    this.buf = new Float64Array(n).fill(NaN);
    this.head = 0;
  }
  push(v) {
    this.buf[this.head] = v ?? NaN;
    this.head = (this.head + 1) % this.buf.length;
  }
  *values() {
    for (let i = 0; i < this.buf.length; i++) yield this.buf[(this.head + i) % this.buf.length];
  }
}

const status = (d, label) => d.status.find(([l]) => l === label)?.[1];
const join = (...parts) => parts.filter(Boolean).join(" · ");

// [key, label, unit, value, note, alert]
const GROUPS = [
  ["Processor", [
    ["cpu_temp", "Temperature", "°C", (d) => d.cpu_temp_c, (d) => (d.cpu_temp_c >= 95 ? "At its limit" : d.cpu_temp_c >= 90 ? "Near its limit" : ""), (d) => d.cpu_temp_c >= 90],
    ["cpu_util", "Load", "%", (d) => d.cpu_util_pct],
    ["cpu_mhz", "Clock", "MHz", (d) => d.cpu_mhz],
  ]],
  ["Graphics", [
    ["gpu_temp", "Temperature", "°C", (d) => d.gpu_temp_c, (d) => (d.gpu_asleep ? "Asleep" : d.gpu_temp_c >= 85 ? "Running hot" : ""), (d) => d.gpu_temp_c >= 85],
    ["gpu_util", "Load", "%", (d) => d.gpu_util_pct, (d) => (d.gpu_limits.length ? `Limited by ${d.gpu_limits.join(", ")}` : ""), (d) => d.gpu_limits.length > 0],
    ["gpu_power", "Power", "W", (d) => d.gpu_power_w],
    ["gpu_mhz", "Clock", "MHz", (d) => d.gpu_mhz],
    ["gpu_vram", "Memory used", "MB", (d) => d.gpu_vram_mb, (d) => (d.gpu_vram_mb >= 5600 ? "Nearly full" : ""), (d) => d.gpu_vram_mb >= 5600],
  ]],
  ["Cooling", [
    ["cpu_fan", "CPU fan", "RPM", (d) => d.cpu_fan_rpm, (d) => (d.max_cooling ? "Full speed" : status(d, "Fan profile") ?? "")],
    ["gpu_fan", "GPU fan", "RPM", (d) => d.gpu_fan_rpm, (d) => (d.cpu_fan_rpm === 0 && d.gpu_fan_rpm === 0 ? "Fans stopped" : "")],
  ]],
  ["Memory and storage", [
    ["mem", "Memory used", "GB", (d) => usedGb(d), (d) => (d.mem_total_mb ? `${num(usedPct(d))}% of ${num(d.mem_total_mb / 1024)} GB` : ""), (d) => usedPct(d) >= 90],
    ["disk", "Disk activity", "%", (d) => d.disk_active_pct, (d) => (d.disk_active_pct >= 90 ? "Saturated" : "")],
  ]],
  ["Power", [
    ["battery", "Battery", "%", (d) => d.battery_pct, (d) => join(d.on_ac ? "Plugged in" : "On battery", status(d, "Charge limit") && `limit ${status(d, "Charge limit")}`)],
    ["battery_rate", "Battery power", "W", (d) => (d.battery_rate_w == null ? null : Math.abs(d.battery_rate_w)), (d) => batteryState(d)],
  ]],
];

/** Memory in use: total minus available, from the same reading. */
const usedGb = (d) => (d.mem_total_mb == null || d.mem_avail_mb == null ? null : (d.mem_total_mb - d.mem_avail_mb) / 1024);
const usedPct = (d) => (d.mem_total_mb ? ((d.mem_total_mb - d.mem_avail_mb) / d.mem_total_mb) * 100 : null);
const batteryState = (d) => (d.battery_rate_w == null ? "" : d.battery_rate_w > 0 ? "Charging" : d.battery_rate_w < 0 ? "Discharging" : "Not charging");

/** Tiles the user turned off, by key. */
const hiddenTiles = new Set(prefs.get("live.hidden", []));

const tiles = new Map();
let built = false;
let timer = null;
let generation = 0;

function build() {
  fill(document.getElementById("figures"),
    ...GROUPS.map(([title, items]) => {
      const grid = h(
        "div",
        { class: "tiles" },
        items.map(([key, label]) => {
          const numEl = h("span", { class: "num" }, NONE);
          const sub = h("span", { class: "sub" });
          const spark = h("canvas", { class: "spark", "aria-hidden": "true" });
          const el = h("article", { class: "tile" }, h("span", { class: "label" }, label), numEl, sub, spark);
          el.hidden = hiddenTiles.has(key);
          tiles.set(key, { el, numEl, sub, spark, ring: new Ring(WINDOW) });
          return el;
        }),
      );
      // Which tiles show, beside the heading: the same chips as History's chart picker.
      const chips = h(
        "div",
        { class: "chips aside", role: "group", "aria-label": `${title} readings to show` },
        items.map(([key, label]) =>
          h(
            "button",
            {
              type: "button",
              class: "chip",
              "aria-pressed": String(!hiddenTiles.has(key)),
              onclick: (e) => {
                const showing = items.filter(([k]) => !hiddenTiles.has(k));
                if (!hiddenTiles.has(key) && showing.length === 1) {
                  toast("At least one reading has to stay on in each section.", { kind: "info" });
                  return;
                }
                if (hiddenTiles.has(key)) hiddenTiles.delete(key);
                else hiddenTiles.add(key);
                prefs.set("live.hidden", [...hiddenTiles]);
                e.currentTarget.setAttribute("aria-pressed", String(!hiddenTiles.has(key)));
                tiles.get(key).el.hidden = hiddenTiles.has(key);
              },
            },
            label,
          ),
        ),
      );
      return collapsible("live", title.toLowerCase().replace(/\s+/g, "-"), title, grid, null, chips);
    }),
    machineSection(),
  );
  built = true;
}

// ── Machine: what the hardware is, read once ──

const machineBody = h("dl", { class: "spec" }, h("dt", {}, "Reading"), h("dd", {}, "Asking Windows…"));

function machineSection() {
  return collapsible("live", "machine", "Machine", machineBody);
}

const gb = (mb) => `${num(mb / 1024)} GB`;
const ghz = (mhz) => `${num(mhz / 1000, 1)} GHz`;

function uptime(bootMs) {
  const mins = Math.max(0, Math.round((Date.now() - bootMs) / 60000));
  const days = Math.floor(mins / 1440);
  const hrs = Math.floor((mins % 1440) / 60);
  return days ? `${days} d ${hrs} h` : `${hrs} h ${mins % 60} min`;
}

async function loadMachine() {
  let info;
  let machine = null;
  try {
    [info, { machine }] = await Promise.all([call("system_info"), overview]);
  } catch (e) {
    fill(machineBody, h("dt", {}, "Hardware"), h("dd", {}, `Couldn't read it. ${e.message}`));
    return;
  }
  const rows = [];
  const add = (label, ...lines) => {
    const vals = lines.filter(Boolean);
    if (vals.length) rows.push(h("dt", {}, label), h("dd", {}, vals.map((v) => h("span", { class: "line" }, v))));
  };
  add("Model", info.model);
  const p = info.processor;
  if (p) add("Processor", join(p.name, p.cores && `${p.cores} cores, ${p.threads} threads`, p.base_mhz && `${ghz(p.base_mhz)} base`));
  const m = info.memory;
  if (m) {
    const same = new Set(m.modules_mb).size === 1;
    const layout = same ? `${m.modules_mb.length} × ${gb(m.modules_mb[0])}` : m.modules_mb.map(gb).join(" + ");
    add("Memory", join(`${gb(m.total_mb)}${m.kind ? ` ${m.kind}` : ""}`, m.speed_mts && `${num(m.speed_mts)} MT/s`, layout));
  }
  add("Graphics", ...info.graphics.map((g) => join(g.name, g.vram_mb && gb(g.vram_mb), g.driver && `driver ${g.driver}`)));
  add("Displays", ...info.displays.map((d) => join(`${d.width} × ${d.height}`, d.hz && `${d.hz} Hz`)));
  add("Storage", ...info.drives.map((d) => join(d.model, `${num(d.size_gb)} GB`)));
  const b = info.battery;
  if (b) {
    const health = (b.full_mwh / b.design_mwh) * 100;
    add("Battery", join(`${num(health)}% of design capacity`, `${num(b.full_mwh / 1000, 1)} of ${num(b.design_mwh / 1000, 1)} Wh`, b.cycles && `${num(b.cycles)} cycles`));
  }
  const os = info.os;
  if (os) add("Windows", join(os.name, os.build && `build ${os.build}`, os.boot_ms && `up ${uptime(os.boot_ms)}`));
  add("Firmware", join(info.bios && `BIOS ${info.bios}`, machine));
  fill(machineBody, rows.length ? rows : [h("dt", {}, "Hardware"), h("dd", {}, "Windows didn't describe this machine.")]);
}

const cssVar = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();

/** Trend line in ink-3, latest reading marked in data cobalt, or Ember while the tile is in alert. */
function drawSpark(canvas, ring, alert) {
  const d = Math.max(1, Math.round(devicePixelRatio || 1));
  const w = canvas.clientWidth;
  const ht = canvas.clientHeight;
  if (!w || !ht) return;
  canvas.width = Math.round(w * d);
  canvas.height = Math.round(ht * d);
  const g = canvas.getContext("2d");
  g.clearRect(0, 0, canvas.width, canvas.height);
  const vals = [...ring.values()];
  const finite = vals.filter(Number.isFinite);
  if (finite.length < 2) return;
  let lo = Math.min(...finite);
  let hi = Math.max(...finite);
  if (hi - lo < 1e-9) [lo, hi] = [lo - 1, hi + 1];
  const pad = 4 * d;
  // Scale across the readings taken so far, so a fresh window doesn't bunch into one corner.
  const first = vals.findIndex(Number.isFinite);
  const x = (i) => pad + ((i - first) / Math.max(1, vals.length - 1 - first)) * (canvas.width - 2 * pad);
  const y = (v) => canvas.height - pad - ((v - lo) / (hi - lo)) * (canvas.height - 2 * pad);
  g.strokeStyle = cssVar("--ink-3");
  g.lineWidth = 1.25 * d;
  g.lineJoin = g.lineCap = "round";
  g.beginPath();
  let pen = false;
  let last = null;
  vals.forEach((v, i) => {
    if (!Number.isFinite(v)) return void (pen = false);
    pen ? g.lineTo(x(i), y(v)) : g.moveTo(x(i), y(v));
    pen = true;
    last = [x(i), y(v)];
  });
  g.stroke();
  if (!last) return;
  g.fillStyle = cssVar("--paper");
  g.beginPath();
  g.arc(last[0], last[1], 4.5 * d, 0, 2 * Math.PI);
  g.fill();
  g.fillStyle = cssVar(alert ? "--ember" : "--d1");
  g.beginPath();
  g.arc(last[0], last[1], 3 * d, 0, 2 * Math.PI);
  g.fill();
}

function update(d) {
  for (const [, items] of GROUPS) {
    for (const [key, , unit, value, note, alert] of items) {
      const t = tiles.get(key);
      const v = value(d);
      t.ring.push(v);
      fill(t.numEl, v == null ? NONE : unit === "GB" ? num(v, 1) : auto(v), v == null ? "" : h("small", {}, unit));
      t.sub.textContent = note?.(d) || "";
      const on = Boolean(alert?.(d));
      t.el.classList.toggle("alert", on);
      drawSpark(t.spark, t.ring, on);
    }
  }
  document.getElementById("live-sub").textContent = join(`Updated ${fmtClock(d.ts_ms)}`, d.tracked_app && "Tracked app running");
}

async function tick(gen) {
  const started = performance.now();
  try {
    const d = await call("live");
    if (gen === generation) update(d);
  } catch (e) {
    if (gen === generation) document.getElementById("live-sub").textContent = `Couldn't read sensors. ${e.message}`;
  }
  if (gen === generation) timer = setTimeout(() => tick(gen), Math.max(0, periodSecs * 1000 - (performance.now() - started)));
}

function setPeriod(secs) {
  periodSecs = secs;
  prefs.set("live.period", secs);
  renderRate();
  // Restart so the new rate applies now, not after the current wait.
  hide();
  show();
}

function renderRate() {
  const custom = !RATES.includes(periodSecs);
  const [min, max] = CUSTOM_RANGE;
  fill(document.getElementById("live-rate"),
    h("span", { class: "meta" }, "Refresh every"),
    h(
      "div",
      { class: "seg", role: "radiogroup", "aria-label": "Refresh rate" },
      RATES.map((r) => h("button", { type: "button", role: "radio", "aria-checked": String(r === periodSecs), onclick: () => setPeriod(r) }, `${r} s`)),
      h(
        "button",
        { type: "button", role: "radio", "aria-checked": String(custom), onclick: () => !custom && setPeriod(10) },
        "Custom",
      ),
    ),
    custom
      ? h(
          "span",
          { class: "number-unit" },
          h("input", {
            type: "number",
            min,
            max,
            value: periodSecs,
            "aria-label": "Refresh interval in seconds",
            onchange: (e) => {
              const n = Math.round(Number(e.target.value));
              if (Number.isFinite(n) && n >= min && n <= max) setPeriod(n);
              else {
                toast(`Choose between ${min} and ${max} seconds.`, { kind: "info" });
                e.target.value = periodSecs;
              }
            },
          }),
          "s",
        )
      : null,
  );
}

export function show() {
  if (!built) {
    build();
    renderRate();
    loadMachine();
  }
  if (timer !== null) return;
  timer = 0;
  tick(++generation);
}

export function hide() {
  clearTimeout(timer);
  timer = null;
  generation++;
}
