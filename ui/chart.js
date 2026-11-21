// History chart, one metric: the title states the finding, the min-max range is a sparse dither with the
// mean drawn over it, limits are dashed rules, gaps where there is no data. Drawn on canvas in device
// pixels so 1 px rules and the dither stay crisp at any Windows scaling.
//
// The caller owns the shared cursor: `on.hover(i)` reports the bucket under the pointer or keyboard and
// every chart's `setCursor(i)` follows it. Zooming is reported as fractions of the plot width:
// `on.select(from, to)` after a drag, `on.zoom(at, factor)` for Ctrl + scroll; Shift + scroll calls
// `on.pan(±1)` and a double-click `on.reset()`.

import { h, auto, withUnit, unitSep, fmtTime, fmtDay, fmtFull, fill } from "./format.js";

const HEIGHT = 150;
const PAD = { top: 10, right: 8, bottom: 34, left: 42 };
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const TIME_STEPS = [5 * MINUTE, 15 * MINUTE, 30 * MINUTE, HOUR, 3 * HOUR, 6 * HOUR, 12 * HOUR, DAY, 2 * DAY, 7 * DAY];
const TABLE_ROWS = 48;

/** Where the fan policy acts; drawn as dashed limits on the temperature charts. */
const LIMITS = { cpu_temp: [90, "Full speed at 90"], gpu_temp: [83, "Full speed at 83"] };

const cssVar = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const dayKey = (ms) => new Date(ms).toDateString();

/** Rounds a raw step to 1, 2 or 5 × 10^n. */
function niceStep(raw) {
  const p = 10 ** Math.floor(Math.log10(raw));
  const f = raw / p;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * p;
}

function yDomain(series, limit) {
  let lo = Infinity;
  let hi = -Infinity;
  for (const b of series.buckets) {
    if (!b) continue;
    lo = Math.min(lo, b[0]);
    hi = Math.max(hi, b[1]);
  }
  if (!Number.isFinite(lo)) return null;
  // Keep the limit in view, so "how close" can be read at a glance.
  if (limit != null) hi = Math.max(hi, limit + 2);
  if (series.unit.startsWith("%")) [lo, hi] = [0, Math.max(100, hi)];
  if (hi - lo < 1e-9) [lo, hi] = [lo - 1, hi + 1];
  const step = niceStep((hi - lo) / 3);
  return { lo: Math.floor(lo / step) * step, hi: Math.ceil(hi / step) * step, step };
}

function timeTicks(from, to, width) {
  const step = TIME_STEPS.find((t) => (to - from) / t <= Math.max(2, width / 110)) ?? 7 * DAY;
  // Align to local midnight and hours.
  const offset = new Date(from).getTimezoneOffset() * MINUTE;
  const first = Math.ceil((from - offset) / step) * step + offset;
  const ticks = [];
  for (let t = first; t <= to; t += step) ticks.push(t);
  return { ticks, daily: step >= DAY };
}

/** The finding, from the series stats: what a reader would want to know first. */
function finding(series) {
  const st = series.stats;
  if (!st) return "No data in this range";
  if (series.id === "gpu_limited") return `Held back ${auto(st.avg)}% of the time`;
  return `Peaked at ${withUnit(st.max, series.unit)}; 95% of the time at or below ${withUnit(st.p95, series.unit)}`;
}

export function chart(series, meta, width, on) {
  const n = series.buckets.length;
  const plotW = Math.max(10, width - PAD.left - PAD.right);
  const plotH = HEIGHT - PAD.top - PAD.bottom;
  const limit = LIMITS[series.id];
  const dom = yDomain(series, limit?.[0]);
  const st = series.stats;
  const unitLine = [series.label, series.unit, st ? `average ${withUnit(st.avg, series.unit)}` : null].filter(Boolean).join(" · ");

  const readout = h("span", { class: "eyebrow readout" });
  const tableBtn = h("button", { type: "button", class: "button", "aria-expanded": "false" }, "Table");
  const head = h(
    "div",
    { class: "head" },
    h("div", {}, h("h3", { class: "title" }, finding(series)), h("span", { class: "eyebrow unit" }, unitLine)),
    h("div", { class: "tools" }, readout, tableBtn),
  );
  const canvas = h("canvas", {
    tabindex: "0",
    role: "img",
    "aria-label": `${series.label}. ${finding(series)}. Arrow keys read values; plus and minus zoom.`,
  });
  const tip = h("div", { class: "tip", "aria-hidden": "true" });
  tip.hidden = true;
  const table = h("div", { class: "table-view" });
  table.hidden = true;
  const el = h("figure", { class: "chart" }, head, h("div", { class: "plot" }, canvas, tip), table);

  if (!dom) {
    canvas.hidden = true;
    tableBtn.hidden = true;
    return { el, setCursor() {}, redraw() {} };
  }

  const xOf = (i) => PAD.left + ((i + 0.5) / n) * plotW;
  const yOf = (v) => PAD.top + (1 - (v - dom.lo) / (dom.hi - dom.lo)) * plotH;
  const timeOf = (i) => meta.from_ms + (i + 0.5) * meta.bucket_ms;
  let cursor = null;
  let drag = null;
  let active = false;

  function draw() {
    const d = Math.max(1, Math.round(devicePixelRatio || 1));
    canvas.width = Math.round(width * d);
    canvas.height = Math.round(HEIGHT * d);
    const g = canvas.getContext("2d");
    g.setTransform(d, 0, 0, d, 0, 0);
    g.clearRect(0, 0, width, HEIGHT);
    const ink = cssVar("--ink");
    const ink3 = cssVar("--ink-3");
    const rule = cssVar("--rule");
    const d1 = cssVar("--d1");
    const mono = `500 10.5px ${cssVar("--mono")}`;
    // Snap 1 px rules to device pixels.
    const px = (v) => (Math.round(v * d) + 0.5) / d;

    g.font = mono;
    g.textBaseline = "middle";
    g.textAlign = "right";
    for (let v = dom.lo; v <= dom.hi + dom.step / 2; v += dom.step) {
      g.fillStyle = rule;
      g.fillRect(PAD.left, px(yOf(v)) - 0.5 / d, plotW, 1 / d);
      g.fillStyle = ink3;
      g.fillText(auto(v), PAD.left - 8, yOf(v));
    }

    if (limit) {
      const y = px(yOf(limit[0]));
      g.fillStyle = ink3;
      for (let x = PAD.left; x < PAD.left + plotW; x += 6) g.fillRect(x, y - 0.5 / d, 3, 1 / d);
      g.textAlign = "right";
      g.textBaseline = "bottom";
      g.fillText(limit[1].toUpperCase(), PAD.left + plotW, y - 3);
    }

    // Range: one device pixel in four, in data cobalt, so it reads as a wash and stays crisp.
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.fillStyle = d1;
    for (let i = 0; i < n; i++) {
      const b = series.buckets[i];
      if (!b) continue;
      const x0 = Math.round((PAD.left + (i / n) * plotW) * d);
      const x1 = Math.round((PAD.left + ((i + 1) / n) * plotW) * d);
      const y0 = Math.round(yOf(b[1]) * d);
      const y1 = Math.round(yOf(b[0]) * d);
      for (let x = x0; x < Math.max(x1, x0 + 1); x++) {
        if (Math.floor(x / d) % 2) continue;
        for (let y = y0; y <= y1; y++) if (Math.floor(y / d) % 2 === 0) g.fillRect(x, y, 1, 1);
      }
    }
    g.setTransform(d, 0, 0, d, 0, 0);

    g.strokeStyle = d1;
    g.lineWidth = 2;
    g.lineJoin = g.lineCap = "round";
    g.beginPath();
    let pen = false;
    for (let i = 0; i < n; i++) {
      const b = series.buckets[i];
      if (!b) {
        pen = false;
        continue;
      }
      pen ? g.lineTo(xOf(i), yOf(b[2])) : g.moveTo(xOf(i), yOf(b[2]));
      pen = true;
    }
    g.stroke();

    // Time on the first line; the day underneath on the first tick and wherever the day changes.
    g.fillStyle = ink;
    g.fillRect(PAD.left, px(PAD.top + plotH) - 0.5 / d, plotW, 1 / d);
    const { ticks, daily } = timeTicks(meta.from_ms, meta.to_ms, plotW);
    let lastDay = null;
    g.textAlign = "center";
    g.textBaseline = "top";
    g.font = mono;
    for (const tm of ticks) {
      const tx = PAD.left + ((tm - meta.from_ms) / (meta.to_ms - meta.from_ms)) * plotW;
      g.fillStyle = ink;
      g.fillRect(px(tx) - 0.5 / d, PAD.top + plotH, 1 / d, 4);
      g.fillStyle = ink3;
      g.fillText(daily ? fmtDay(tm) : fmtTime(tm), tx, PAD.top + plotH + 8);
      if (!daily && dayKey(tm) !== lastDay) g.fillText(fmtDay(tm).toUpperCase(), tx, PAD.top + plotH + 21);
      lastDay = dayKey(tm);
    }

    if (cursor != null && series.buckets[cursor]) {
      const b = series.buckets[cursor];
      const cx = px(xOf(cursor));
      g.fillStyle = ink3;
      g.fillRect(cx - 0.5 / d, PAD.top, 1 / d, plotH);
      g.fillStyle = cssVar("--paper");
      g.beginPath();
      g.arc(xOf(cursor), yOf(b[2]), 6, 0, 2 * Math.PI);
      g.fill();
      g.fillStyle = d1;
      g.beginPath();
      g.arc(xOf(cursor), yOf(b[2]), 4, 0, 2 * Math.PI);
      g.fill();
    }

    if (drag && drag.to != null) {
      const [a, z] = drag.from < drag.to ? [drag.from, drag.to] : [drag.to, drag.from];
      const x0 = PAD.left + a * plotW;
      const w = (z - a) * plotW;
      g.fillStyle = cssVar("--tint");
      g.globalAlpha = 0.7;
      g.fillRect(x0, PAD.top, w, plotH);
      g.globalAlpha = 1;
      g.fillStyle = cssVar("--accent");
      g.fillRect(px(x0) - 0.5 / d, PAD.top, 1 / d, plotH);
      g.fillRect(px(x0 + w) - 0.5 / d, PAD.top, 1 / d, plotH);
      // Say what the drag will show before it is released.
      const t = (f) => meta.from_ms + f * (meta.to_ms - meta.from_ms);
      const label = `${fmtTime(t(a))} – ${fmtTime(t(z))}`;
      g.font = mono;
      g.textBaseline = "top";
      g.textAlign = x0 + w / 2 < PAD.left + plotW / 2 ? "left" : "right";
      g.fillStyle = cssVar("--ink");
      g.fillText(label, g.textAlign === "left" ? x0 + 6 : x0 + w - 6, PAD.top + 4);
    }
  }

  function showTip() {
    const b = cursor == null ? null : series.buckets[cursor];
    if (!active || !b) {
      tip.hidden = true;
      return;
    }
    const sep = unitSep(series.unit);
    const row = (key, name, value) => h("div", { class: "row-t" }, key ? h("i") : null, h("span", {}, name), h("b", {}, value));
    fill(
      tip,
      h("div", { class: "when" }, fmtFull(timeOf(cursor))),
      row(true, "Mean", withUnit(b[2], series.unit)),
      b[0] === b[1] ? null : row(false, "Range", `${auto(b[0])} to ${auto(b[1])}${sep}${series.unit}`),
    );
    tip.hidden = false;
    const x = xOf(cursor);
    const w = tip.offsetWidth;
    tip.style.left = `${x + 14 + w > width ? x - 14 - w : x + 14}px`;
  }

  function setCursor(i) {
    cursor = i;
    const b = i == null ? null : series.buckets[i];
    readout.textContent = b ? `${fmtTime(timeOf(i))} · ${withUnit(b[2], series.unit)}` : "";
    draw();
    showTip();
  }

  /** Pointer position as a fraction of the plot width, clamped to 0..1. */
  const fractionAt = (e) => {
    const r = canvas.getBoundingClientRect();
    const x = ((e.clientX - r.left) / r.width) * width;
    return Math.min(1, Math.max(0, (x - PAD.left) / plotW));
  };
  const indexAt = (e) => Math.min(n - 1, Math.floor(fractionAt(e) * n));

  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    drag = { from: fractionAt(e), to: null };
    canvas.setPointerCapture(e.pointerId);
  });
  canvas.addEventListener("pointermove", (e) => {
    active = true;
    if (drag) {
      drag.to = fractionAt(e);
      draw();
      return;
    }
    on.hover(indexAt(e));
  });
  canvas.addEventListener("pointerup", () => {
    if (!drag) return;
    const { from, to } = drag;
    drag = null;
    draw();
    if (to != null && Math.abs(to - from) * plotW >= 6) on.select(Math.min(from, to), Math.max(from, to));
  });
  canvas.addEventListener("pointerleave", () => {
    if (drag) return;
    active = false;
    on.hover(null);
  });
  canvas.addEventListener(
    "wheel",
    (e) => {
      if (e.ctrlKey) {
        e.preventDefault();
        on.zoom(fractionAt(e), e.deltaY < 0 ? 0.5 : 2);
      } else if (e.shiftKey) {
        e.preventDefault();
        on.pan(Math.sign(e.deltaY || e.deltaX));
      }
    },
    { passive: false },
  );
  canvas.addEventListener("dblclick", () => on.reset());
  // Keyboard reading: the arrows move the shared cursor; + and − zoom through the page's handler.
  canvas.addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const step = (e.key === "ArrowRight" ? 1 : -1) * (e.shiftKey ? 10 : 1);
    let i = Math.min(n - 1, Math.max(0, (cursor ?? Math.floor(n / 2)) + step));
    // Skip over gaps so every press lands on a reading.
    while (i > 0 && i < n - 1 && !series.buckets[i]) i += Math.sign(step);
    on.hover(i);
  });
  canvas.addEventListener("focus", () => {
    active = true;
    on.hover(cursor ?? series.buckets.findLastIndex((b) => b));
  });
  canvas.addEventListener("blur", () => {
    active = false;
    on.hover(null);
  });

  tableBtn.addEventListener("click", () => {
    const open = table.hidden;
    table.hidden = !open;
    tableBtn.setAttribute("aria-expanded", String(open));
    if (open && !table.firstChild) {
      // Readings grouped to at most TABLE_ROWS rows, so the table stays readable at any range.
      const per = Math.max(1, Math.ceil(n / TABLE_ROWS));
      const rows = [];
      for (let a = 0; a < n; a += per) {
        let lo = Infinity;
        let hi = -Infinity;
        let sum = 0;
        let count = 0;
        for (let i = a; i < Math.min(n, a + per); i++) {
          const b = series.buckets[i];
          if (!b) continue;
          lo = Math.min(lo, b[0]);
          hi = Math.max(hi, b[1]);
          sum += b[2];
          count++;
        }
        if (count) rows.push(h("tr", {}, h("td", {}, fmtFull(timeOf(a))), h("td", {}, auto(sum / count)), h("td", {}, auto(lo)), h("td", {}, auto(hi))));
      }
      const u = series.unit ? ` (${series.unit})` : "";
      fill(table, h("table", {}, h("thead", {}, h("tr", {}, h("th", {}, "From"), h("th", {}, `Mean${u}`), h("th", {}, "Min"), h("th", {}, "Max"))), h("tbody", {}, rows)));
    }
  });

  draw();
  return { el, setCursor, redraw: draw };
}
