// Time-series chart: min–max band, mean line, gaps where no data. Hover cursors are synchronised by
// the caller through onHover/setCursor.

import { s, h, auto, withUnit, unitSep, fmtTime, fmtDay, fmtFull, fill } from "./format.js";

const dayKey = (ms) => new Date(ms).toDateString();

const HEIGHT = 162;
const PAD = { top: 8, right: 4, bottom: 30, left: 40 };

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const TIME_STEPS = [5 * MINUTE, 15 * MINUTE, 30 * MINUTE, HOUR, 3 * HOUR, 6 * HOUR, 12 * HOUR, DAY, 2 * DAY, 7 * DAY];

/** Rounds a raw step to 1, 2 or 5 × 10^n. */
function niceStep(raw) {
  const p = 10 ** Math.floor(Math.log10(raw));
  const f = raw / p;
  return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10) * p;
}

function yDomain(series) {
  let lo = Infinity;
  let hi = -Infinity;
  for (const b of series.buckets) {
    if (!b) continue;
    lo = Math.min(lo, b[0]);
    hi = Math.max(hi, b[1]);
  }
  if (!Number.isFinite(lo)) return null;
  if (series.unit.startsWith("%")) [lo, hi] = [0, Math.max(100, hi)];
  if (hi - lo < 1e-9) [lo, hi] = [lo - 1, hi + 1];
  const step = niceStep((hi - lo) / 3);
  return { lo: Math.floor(lo / step) * step, hi: Math.ceil(hi / step) * step, step };
}

function timeTicks(from, to, width) {
  const step = TIME_STEPS.find((t) => (to - from) / t <= Math.max(2, width / 110)) ?? 7 * DAY;
  // Align to local midnight/hours.
  const offset = new Date(from).getTimezoneOffset() * MINUTE;
  const first = Math.ceil((from - offset) / step) * step + offset;
  const ticks = [];
  for (let t = first; t <= to; t += step) ticks.push(t);
  return { ticks, daily: step >= DAY };
}

/** Contiguous runs of non-null buckets, as index ranges. */
function runs(buckets) {
  const out = [];
  let start = -1;
  buckets.forEach((b, i) => {
    if (b && start < 0) start = i;
    if (!b && start >= 0) out.push([start, i - 1]), (start = -1);
  });
  if (start >= 0) out.push([start, buckets.length - 1]);
  return out;
}

export function chart(series, meta, width, onHover) {
  const n = series.buckets.length;
  const plotW = Math.max(10, width - PAD.left - PAD.right);
  const plotH = HEIGHT - PAD.top - PAD.bottom;
  const x = (i) => PAD.left + ((i + 0.5) / n) * plotW;
  const dom = yDomain(series);
  const y = (v) => PAD.top + (1 - (v - dom.lo) / (dom.hi - dom.lo)) * plotH;

  const stats = h("span", { class: "stats" });
  const st = series.stats;
  const summary = () =>
    fill(stats, 
      ...(st
        ? series.id === "gpu_limited"
          ? [h("b", {}, `${auto(st.avg)}%`), " of the time"]
          : ["avg ", h("b", {}, auto(st.avg)), " · p95 ", h("b", {}, auto(st.p95)), " · max ", h("b", {}, auto(st.max)), `${unitSep(series.unit)}${series.unit}`]
        : ["No data"]),
    );
  summary();

  const svg = s("svg", { viewBox: `0 0 ${width} ${HEIGHT}`, role: "img", "aria-label": `${series.label} over time` });
  if (!dom) {
    return { el: h("figure", { class: "chart" }, h("figcaption", {}, h("span", { class: "title" }, series.label), stats), svg), setCursor() {} };
  }

  const grid = s("g", { class: "grid" });
  const axis = s("g", { class: "axis" });
  for (let v = dom.lo; v <= dom.hi + dom.step / 2; v += dom.step) {
    grid.append(s("line", { x1: PAD.left, x2: PAD.left + plotW, y1: y(v), y2: y(v) }));
    const t = s("text", { x: PAD.left - 8, y: y(v) + 3.5, "text-anchor": "end" });
    t.textContent = auto(v);
    axis.append(t);
  }
  // Time on the first line; the day underneath on the first tick and wherever the day changes,
  // so every position on the axis can be read as a date and a time.
  const { ticks, daily } = timeTicks(meta.from_ms, meta.to_ms, plotW);
  let lastDay = null;
  for (const tm of ticks) {
    const tx = PAD.left + ((tm - meta.from_ms) / (meta.to_ms - meta.from_ms)) * plotW;
    const t = s("text", { x: tx, y: HEIGHT - 17, "text-anchor": "middle" });
    t.textContent = daily ? fmtDay(tm) : fmtTime(tm);
    axis.append(t);
    if (!daily && dayKey(tm) !== lastDay) {
      const d = s("text", { x: tx, y: HEIGHT - 4, "text-anchor": "middle", class: "day" });
      d.textContent = fmtDay(tm);
      axis.append(d);
    }
    lastDay = dayKey(tm);
  }

  let band = "";
  let line = "";
  for (const [a, b] of runs(series.buckets)) {
    let top = "";
    let bottom = "";
    for (let i = a; i <= b; i++) {
      const [mn, mx, mean] = series.buckets[i];
      top += `${i === a ? "M" : "L"}${x(i).toFixed(1)} ${y(mx).toFixed(1)}`;
      bottom = `L${x(i).toFixed(1)} ${y(mn).toFixed(1)}` + bottom;
      line += `${i === a ? "M" : "L"}${x(i).toFixed(1)} ${y(mean).toFixed(1)}`;
    }
    // A single bucket has no width; widen it so it stays visible.
    if (a === b) line += `l0.01 0`;
    band += `${top}${bottom}Z`;
  }

  const cursor = s("line", { class: "cursor", y1: PAD.top, y2: PAD.top + plotH, visibility: "hidden" });
  const dot = s("circle", { class: "dot", r: 3, visibility: "hidden" });
  svg.append(grid, s("path", { class: "band", d: band }), s("path", { class: "line", d: line }), axis, cursor, dot);

  const indexAt = (e) => {
    const r = svg.getBoundingClientRect();
    const px = ((e.clientX - r.left) / r.width) * width;
    return Math.min(n - 1, Math.max(0, Math.floor(((px - PAD.left) / plotW) * n)));
  };
  svg.addEventListener("pointermove", (e) => onHover(indexAt(e)));
  svg.addEventListener("pointerleave", () => onHover(null));

  function setCursor(i) {
    const b = i == null ? null : series.buckets[i];
    if (!b) {
      cursor.setAttribute("visibility", "hidden");
      dot.setAttribute("visibility", "hidden");
      summary();
      return;
    }
    const cx = x(i);
    cursor.setAttribute("x1", cx);
    cursor.setAttribute("x2", cx);
    dot.setAttribute("cx", cx);
    dot.setAttribute("cy", y(b[2]));
    cursor.setAttribute("visibility", "visible");
    dot.setAttribute("visibility", "visible");
    const t = meta.from_ms + (i + 0.5) * meta.bucket_ms;
    const range = b[0] === b[1] ? "" : ` (${auto(b[0])}–${auto(b[1])})`;
    fill(stats, `${fmtFull(t)} · `, h("b", {}, withUnit(b[2], series.unit)), range);
  }

  const el = h("figure", { class: "chart" }, h("figcaption", {}, h("span", { class: "title" }, series.label), stats), svg);
  return { el, setCursor };
}
