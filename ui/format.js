// DOM and formatting helpers. Text always goes in via textContent, never as HTML. Every formatter
// returns "–" for missing values, so null never reaches the screen.

const SVG_NS = "http://www.w3.org/2000/svg";
export const NONE = "–";

const missing = (v) => v == null || (typeof v === "number" && !Number.isFinite(v));

export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) if (c != null && c !== false && c !== "") el.append(c);
  return el;
}

/**
 * Replaces an element's children. Unlike `replaceChildren`, which renders null as the text "null",
 * this skips null, false and empty strings, so optional parts can be passed inline.
 */
export function fill(el, ...children) {
  el.replaceChildren(...children.flat().filter((c) => c != null && c !== false && c !== ""));
  return el;
}

export function s(tag, attrs = {}) {
  const el = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) el.setAttribute(k, v);
  return el;
}

export function num(v, digits = 0) {
  if (missing(v)) return NONE;
  return v.toLocaleString("en-NZ", { minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** One decimal below 10, none above: 0.4, 12, 1,250. */
export function auto(v) {
  if (missing(v)) return NONE;
  const a = Math.abs(v);
  return num(v, a > 0 && a < 10 ? 1 : 0);
}

/** A space before every unit except percentages: "88.4 °C", "4,812 RPM", "60%", "12% of time". */
export function unitSep(unit) {
  return unit && !unit.startsWith("%") ? " " : "";
}

export function withUnit(v, unit = "") {
  return missing(v) ? NONE : `${auto(v)}${unitSep(unit)}${unit}`;
}

const timeFmt = new Intl.DateTimeFormat("en-NZ", { hour: "2-digit", minute: "2-digit", hour12: false });
const dayFmt = new Intl.DateTimeFormat("en-NZ", { weekday: "short", day: "numeric" });
const fullFmt = new Intl.DateTimeFormat("en-NZ", { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit", hour12: false });

const clockFmt = new Intl.DateTimeFormat("en-NZ", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

export const fmtTime = (ms) => (missing(ms) ? NONE : timeFmt.format(ms));
export const fmtClock = (ms) => (missing(ms) ? NONE : clockFmt.format(ms));
export const fmtDay = (ms) => (missing(ms) ? NONE : dayFmt.format(ms));
export const fmtFull = (ms) => (missing(ms) ? NONE : fullFmt.format(ms));

const dateFmt = new Intl.DateTimeFormat("en-NZ", { weekday: "short", day: "numeric", month: "short" });

const sameDay = (a, b) => new Date(a).toDateString() === new Date(b).toDateString();

/** "Thu 20 Nov, 10:15 – 11:15", or "Thu 20 Nov, 10:15 – Fri 21 Nov, 10:15" across days. */
export function fmtRange(from, to) {
  if (missing(from) || missing(to)) return NONE;
  const start = `${dateFmt.format(from)}, ${timeFmt.format(from)}`;
  const end = sameDay(from, to) ? timeFmt.format(to) : `${dateFmt.format(to)}, ${timeFmt.format(to)}`;
  return `${start} – ${end}`;
}

export function ago(ms) {
  if (missing(ms)) return NONE;
  const s = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (s < 60) return `${s} s ago`;
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return `${Math.round(s / 86400)} d ago`;
}

let toastTimer;
let shownKind = null;
/** A message that arrived while an error was showing; it waits until the error is dismissed. */
let queued = null;

const TAGS = { ok: "[ OK ]", info: "[ INFO ]", progress: "[ … ]", error: "[ ERROR ]" };

/**
 * Shows a message. `kind` is ok, info, progress or error: errors take the Ember ground, are announced
 * assertively and stay until dismissed; a later non-error message waits behind an error instead of
 * replacing it. `action` adds a button such as Undo; `details` (lines of text) adds an arrow that
 * expands the list. The toast stays up while expanded or hovered.
 */
export function toast(text, { kind = "ok", action = null, details = null, ms = action || details ? 7000 : 3200 } = {}) {
  const el = document.getElementById("toast");
  if (shownKind === "error" && kind !== "error" && el.classList.contains("show")) {
    queued = [text, { kind, action, details, ms }];
    return;
  }
  const more = document.getElementById("toast-more");
  const list = document.getElementById("toast-details");
  const btn = document.getElementById("toast-action");
  const close = document.getElementById("toast-close");
  const sticky = kind === "error";
  const hide = () => {
    clearTimeout(toastTimer);
    el.classList.remove("show");
    shownKind = null;
    if (queued) {
      const [t, o] = queued;
      queued = null;
      toast(t, o);
    }
  };
  const arm = () => {
    clearTimeout(toastTimer);
    if (!sticky) toastTimer = setTimeout(hide, ms);
  };

  shownKind = kind;
  el.classList.toggle("error", sticky);
  el.setAttribute("role", sticky ? "alert" : "status");
  el.setAttribute("aria-live", sticky ? "assertive" : "polite");
  document.getElementById("toast-tag").textContent = TAGS[kind] ?? TAGS.ok;
  document.getElementById("toast-text").textContent = text;
  close.onclick = hide;
  btn.hidden = !action;
  btn.onclick = null;
  if (action) {
    btn.textContent = action.label;
    btn.onclick = () => {
      hide();
      action.run();
    };
  }

  const hasDetails = Array.isArray(details) && details.length > 0;
  more.hidden = !hasDetails;
  more.setAttribute("aria-expanded", "false");
  list.hidden = true;
  fill(list, hasDetails ? details.map((line) => h("li", {}, line)) : []);
  more.onclick = () => {
    const open = list.hidden;
    list.hidden = !open;
    more.setAttribute("aria-expanded", String(open));
    if (open) clearTimeout(toastTimer);
    else arm();
  };
  el.onmouseenter = () => clearTimeout(toastTimer);
  el.onmouseleave = () => list.hidden && arm();

  el.classList.add("show");
  arm();
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && document.getElementById("toast")?.classList.contains("show") && !e.target.closest("dialog")) {
    document.getElementById("toast-close").click();
  }
});

export function debounce(fn, ms) {
  let t;
  return (...a) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
}

/**
 * A section whose heading folds its body. State is remembered per page and section.
 * `meta` (optional) is shown at the right of the heading, inside the toggle; `aside` (optional) sits
 * beside the heading outside it, for controls of its own (a button can't contain buttons).
 */
export function collapsible(page, key, title, body, meta = null, aside = null) {
  const pref = `collapsed.${page}.${key}`;
  let collapsed = prefs.get(pref, false);
  const content = h("div", { class: "section-body", id: `${page}-${key}-body` }, body);
  content.hidden = collapsed;
  const toggle = h(
    "button",
    { type: "button", class: "section-toggle", "aria-expanded": String(!collapsed), "aria-controls": content.id },
    h("span", { class: "chev", "aria-hidden": "true" }, "›"),
    h("h2", {}, title),
    meta,
  );
  const el = h("section", { class: `section${collapsed ? " collapsed" : ""}` }, h("div", { class: "section-head" }, toggle, aside), content);
  toggle.addEventListener("click", () => {
    collapsed = !collapsed;
    prefs.set(pref, collapsed);
    content.hidden = collapsed;
    toggle.setAttribute("aria-expanded", String(!collapsed));
    el.classList.toggle("collapsed", collapsed);
  });
  return el;
}

/** localStorage, for per-window preferences only. Failures are ignored. */
export const prefs = {
  get(key, fallback) {
    try {
      const v = localStorage.getItem(`spanner.${key}`);
      return v == null ? fallback : JSON.parse(v);
    } catch {
      return fallback;
    }
  },
  set(key, value) {
    try {
      localStorage.setItem(`spanner.${key}`, JSON.stringify(value));
    } catch {
      /* not persisted */
    }
  },
};
