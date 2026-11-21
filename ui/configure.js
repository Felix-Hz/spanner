// Configure: every control applies immediately; the confirmation offers Undo.

import { call } from "./ipc.js";
import { collapsible, h, NONE, num, toast, fill } from "./format.js";

const SECTIONS = [
  ["performance", "Performance"],
  ["cooling", "Cooling"],
  ["battery", "Battery"],
];

/** Settings that stop mattering while another one is in effect. `when` gets the other setting's view. */
const OVERRIDES = [
  { id: "msi.fan_mode", by: "full_speed", when: (o) => o.active === true, text: "Full-speed fans is on right now, so the fan profile has no effect." },
  { id: "boost_mode", by: "proc_max", when: (o) => o.current != null && o.current < 100, text: "Turbo boost is off, so this has no effect." },
];

/** How long to wait before checking that a change wasn't put back by other software. */
const RECHECK_MS = 4000;
const REFRESH_MS = 3000;

const root = document.getElementById("settings");
let views = [];
const openAdvanced = new Set();
const busy = new Set();
let timer = null;
let interacting = false;
let fanText = "";

const byId = (id) => views.find((v) => v.id === id);

function optionLabel(v, value) {
  if (value == null) return NONE;
  if (v.control.kind === "choice") return v.control.options.find((o) => o.value === value)?.label ?? String(value);
  return `${value}${v.control.unit}`;
}

const isSwitch = (v) =>
  v.control.kind === "choice" && v.control.options.length === 2 && v.control.options.every((o) => o.value === 0 || o.value === 1);

// ── Applying ──

/** "Fan profile: Auto → Advanced". Turbo boost is shown as the switch it appears as. */
function describeChange(c) {
  const v = byId(c.id);
  if (!v) return c.id;
  if (v.id === "proc_max" && [99, 100].includes(c.value) && [99, 100].includes(v.current)) {
    return `Turbo boost: ${v.current >= 100 ? "On" : "Off"} → ${c.value >= 100 ? "On" : "Off"}`;
  }
  return `${v.name}: ${optionLabel(v, v.current)} → ${optionLabel(v, c.value)}`;
}

async function commit(changes, message, undoable = true) {
  const details = changes.length > 1 ? changes.map(describeChange) : null;
  const before = changes.map((c) => ({ id: c.id, value: byId(c.id)?.current })).filter((c) => c.value != null);
  changes.forEach((c) => busy.add(c.id));
  render();
  try {
    const outcomes = await call("apply", { changes });
    const failed = outcomes.filter((o) => !o.ok);
    await reload();
    if (failed.length) {
      const names = failed.map((f) => byId(f.id)?.name ?? f.id).join(", ");
      toast(`Couldn't change ${names}. ${failed[0].error ?? ""}`.trim(), { kind: "error" });
    } else {
      toast(message, { details, action: undoable ? { label: "Undo", run: () => commit(before, "Changes undone.", false) } : null });
      setTimeout(() => recheck(changes), RECHECK_MS);
    }
  } catch (e) {
    await reload();
    toast(`Nothing was changed. ${e.message}`, { kind: "error" });
  } finally {
    changes.forEach((c) => busy.delete(c.id));
    render();
  }
}

/** Reports changes that were reverted shortly after being applied, e.g. by MSI Center's service. */
async function recheck(changes) {
  await reload();
  const reverted = changes.filter((c) => {
    const v = byId(c.id);
    return v && v.current != null && v.current !== c.value;
  });
  if (!reverted.length) return;
  const v = byId(reverted[0].id);
  const who = v.id.startsWith("msi.") ? "MSI Center" : "other software";
  toast(`${v.name} was set back to ${optionLabel(v, v.current)}, probably by ${who}. Change it there as well, or turn off its control of this setting.`, { kind: "info", ms: 12000 });
}

function set(v, value, name = v.name, label = optionLabel(v, value)) {
  if (value === v.current) return;
  commit([{ id: v.id, value }], `${name}: ${label}`);
}

// ── Presets ──

let userPresets = [];

/** Built-in presets read their values from each setting; user presets carry their own. */
function presetValues(p) {
  if (p.builtin) return Object.fromEntries(views.filter((v) => v[p.builtin] != null).map((v) => [v.id, v[p.builtin]]));
  return p.values;
}

/** Changes needed to reach a preset, skipping settings this machine doesn't have. */
function presetChanges(p) {
  return Object.entries(presetValues(p))
    .map(([id, value]) => ({ id, value, v: byId(id) }))
    .filter(({ v, value }) => v && v.unavailable == null && v.current !== value)
    .map(({ id, value }) => ({ id, value }));
}

function allPresets() {
  return [
    { name: "Gaming", builtin: "gaming", hint: "Built in" },
    { name: "Defaults", builtin: "default", hint: "Built in" },
    ...userPresets.map((p) => ({ ...p, hint: `${Object.keys(p.values).length} settings` })),
  ];
}

function applyPreset(p) {
  const changes = presetChanges(p);
  if (!changes.length) return toast(`Everything already matches ${p.name}.`, { kind: "info" });
  commit(changes, `${p.name} applied: ${changes.length} change${changes.length === 1 ? "" : "s"}.`);
}

async function loadPresets() {
  try {
    userPresets = await call("presets");
  } catch {
    userPresets = [];
  }
}

function closeMenu() {
  const list = document.querySelector("#presets-menu .menu-list");
  if (!list) return;
  list.remove();
  document.querySelector("#presets-menu > .menu > button").setAttribute("aria-expanded", "false");
}

function openMenu(button) {
  const items = [];
  const item = (label, hint, onPick, ticked = false) => {
    const el = h(
      "button",
      { type: "button", role: "menuitem", onclick: () => (closeMenu(), onPick()) },
      h("span", { class: "tick", "aria-hidden": "true" }, ticked ? "✓" : ""),
      label,
      hint ? h("span", { class: "hint" }, hint) : null,
    );
    items.push(el);
    return el;
  };
  const presets = allPresets();
  const builtins = presets.filter((p) => p.builtin);
  const mine = presets.filter((p) => !p.builtin);
  const list = h(
    "div",
    { class: "menu-list", role: "menu", "aria-label": "Presets" },
    builtins.map((p) => item(p.name, p.hint, () => applyPreset(p), presetChanges(p).length === 0)),
    mine.length ? [h("hr"), h("div", { class: "group" }, "Your presets"), mine.map((p) => item(p.name, p.hint, () => applyPreset(p), presetChanges(p).length === 0))] : null,
    h("hr"),
    item("Save current settings as preset…", null, savePresetDialog),
  );
  list.addEventListener("keydown", (e) => {
    const i = items.indexOf(document.activeElement);
    const next = { ArrowDown: i + 1, ArrowUp: i - 1, Home: 0, End: items.length - 1 }[e.key];
    if (e.key === "Escape") {
      closeMenu();
      button.focus();
    } else if (next != null) {
      e.preventDefault();
      items[(next + items.length) % items.length].focus();
    }
  });
  button.after(list);
  button.setAttribute("aria-expanded", "true");
  items[0]?.focus();
}

function renderPresetsMenu() {
  const button = h("button", { class: "button", type: "button", "aria-haspopup": "menu", "aria-expanded": "false" }, "Presets ▾");
  button.addEventListener("click", async () => {
    if (button.getAttribute("aria-expanded") === "true") return closeMenu();
    await loadPresets();
    openMenu(button);
  });
  fill(document.getElementById("presets-menu"), h("div", { class: "menu" }, button));
}

document.addEventListener("pointerdown", (e) => {
  if (!e.target.closest("#presets-menu")) closeMenu();
});

function savePresetDialog() {
  const dlg = document.getElementById("preset-dialog");
  const name = document.getElementById("preset-name");
  const replace = document.getElementById("preset-replace");
  const save = document.getElementById("preset-save");
  const exists = () => userPresets.some((p) => p.name.toLowerCase() === name.value.trim().toLowerCase());
  name.value = "";
  name.oninput = () => {
    replace.hidden = !exists();
    save.textContent = exists() ? "Replace" : "Save";
  };
  name.oninput();
  dlg.returnValue = "";
  dlg.showModal();
  name.focus();
  dlg.addEventListener(
    "close",
    async () => {
      if (dlg.returnValue !== "save" || !name.value.trim()) return;
      const values = Object.fromEntries(views.filter((v) => v.unavailable == null && v.current != null).map((v) => [v.id, v.current]));
      try {
        userPresets = await call("save_preset", { name: name.value.trim(), values });
        toast(`Saved preset "${name.value.trim()}" with ${Object.keys(values).length} settings.`, {
          details: Object.entries(values).map(([id, value]) => `${byId(id)?.name ?? id}: ${optionLabel(byId(id), value)}`),
        });
      } catch (e) {
        toast(`Couldn't save the preset. ${e.message}`, { kind: "error" });
      }
    },
    { once: true },
  );
}

// ── Controls ──

function switchControl(checked, label, onToggle, disabled) {
  return h("button", { type: "button", class: "switch", role: "switch", "aria-checked": String(checked), "aria-label": label, disabled, onclick: onToggle });
}

function control(v) {
  const disabled = v.unavailable != null || busy.has(v.id);
  const c = v.control;
  if (isSwitch(v)) return switchControl(v.current === 1, v.name, () => set(v, v.current === 1 ? 0 : 1), disabled);
  if (c.kind === "choice" && c.options.length <= 4) {
    return h(
      "div",
      { class: "seg", role: "radiogroup", "aria-label": v.name },
      c.options.map((o) =>
        h("button", { type: "button", role: "radio", "aria-checked": String(o.value === v.current), disabled, onclick: () => set(v, o.value) }, o.label),
      ),
    );
  }
  if (c.kind === "choice") {
    return h(
      "select",
      { "aria-label": v.name, disabled, onchange: (e) => set(v, Number(e.target.value)) },
      v.current == null ? h("option", { value: "", selected: true }, NONE) : null,
      c.options.map((o) => h("option", { value: o.value, selected: o.value === v.current }, o.label)),
    );
  }
  const out = h("output", {}, optionLabel(v, v.current));
  const input = h("input", {
    type: "range",
    min: c.min,
    max: c.max,
    step: c.step,
    value: v.current ?? c.min,
    disabled,
    "aria-label": v.name,
    oninput: (e) => (out.textContent = `${e.target.value}${c.unit}`),
    onchange: (e) => set(v, Number(e.target.value)),
  });
  return h("div", { class: "slider" }, input, out, c.ends ? h("div", { class: "ends" }, h("span", {}, c.ends[0]), h("span", {}, c.ends[1])) : null);
}

function hintFor(v) {
  if (v.unavailable) return [v.unavailable, false];
  const o = OVERRIDES.find((x) => x.id === v.id);
  const by = o && byId(o.by);
  if (by && o.when(by)) return [o.text, true];
  if (v.status) return [v.status, v.active === true];
  if (v.control.kind === "choice" && !isSwitch(v)) {
    const help = v.control.options.find((x) => x.value === v.current)?.help;
    if (help) return [help, false];
  }
  if (v.current_dc != null) return [`${optionLabel(v, v.current_dc)} on battery.`, false];
  return [null, false];
}

function row(v) {
  const [hint, warn] = hintFor(v);
  const overridden = warn;
  const resettable = v.default != null && v.current != null && v.current !== v.default && !v.unavailable;
  return h(
    "div",
    { class: `row${v.unavailable ? " off" : ""}${busy.has(v.id) ? " busy" : ""}${overridden ? " overridden" : ""}` },
    h("div", {}, h("h3", {}, v.name), h("p", { class: "help" }, v.help), hint ? h("p", { class: `hint${warn ? " warn" : ""}` }, hint) : null),
    h(
      "div",
      { class: "control" },
      resettable
        ? h("button", { type: "button", class: "link reset", title: `Default: ${optionLabel(v, v.default)}`, onclick: () => set(v, v.default, v.name, `${optionLabel(v, v.default)} (default)`) }, "Reset")
        : null,
      control(v),
    ),
  );
}

/** Turbo boost is the maximum processor state at 100% (on) or 99% (off), shown as a switch. */
function turboRow(max) {
  const on = max.current != null && max.current >= 100;
  const limited = max.current != null && max.current < 99;
  return h(
    "div",
    { class: `row${max.unavailable ? " off" : ""}${busy.has(max.id) ? " busy" : ""}` },
    h(
      "div",
      {},
      h("h3", {}, "Turbo boost"),
      h("p", { class: "help" }, "Lets the CPU run above its base clock under load. Off runs much cooler and slightly slower."),
      limited ? h("p", { class: "hint" }, `CPU limited to ${max.current}% under Advanced.`) : null,
    ),
    h(
      "div",
      { class: "control" },
      switchControl(on, "Turbo boost", () => set(max, on ? 99 : 100, "Turbo boost", on ? "Off" : "On"), max.unavailable != null || busy.has(max.id)),
    ),
  );
}

// ── Rendering ──

function section([key, title]) {
  const rows = views.filter((v) => v.section === key);
  if (!rows.length) return null;
  const basic = rows.filter((v) => !v.advanced);
  const advanced = rows.filter((v) => v.advanced);
  const max = byId("proc_max");
  const details = advanced.length
    ? h(
        "details",
        {
          class: "advanced",
          open: openAdvanced.has(key),
          ontoggle: (e) => (e.target.open ? openAdvanced.add(key) : openAdvanced.delete(key)),
        },
        h("summary", {}, "Advanced"),
        advanced.map(row),
      )
    : null;
  const meta = key === "cooling" ? h("span", { class: "meta", id: "fan-speeds" }, fanText) : null;
  return collapsible("configure", key, title, [key === "performance" && max ? turboRow(max) : null, ...basic.map(row), details], meta);
}

function render() {
  const scroll = root.scrollTop;
  fill(root, ...SECTIONS.map(section));
  root.scrollTop = scroll;
  root.setAttribute("aria-busy", "false");
}

async function reload() {
  try {
    views = await call("settings");
    render();
  } catch (e) {
    fill(root, h("p", { class: "empty" }, `Couldn't read the current settings. ${e.message}`));
  }
}

/** Picks up changes made elsewhere, unless the user is in the middle of using a control. */
async function refresh() {
  if (interacting || busy.size || root.contains(document.activeElement) && document.activeElement.matches("select, input")) return;
  await reload();
}

root.addEventListener("pointerdown", () => (interacting = true));
window.addEventListener("pointerup", () => (interacting = false));

async function fans() {
  try {
    const d = await call("live");
    const el = document.getElementById("fan-speeds");
    if (d.cpu_fan_rpm != null || d.gpu_fan_rpm != null) {
      // Kept so a re-render (every few seconds) doesn't blank the reading.
      fanText = `CPU fan ${num(d.cpu_fan_rpm)} RPM · GPU fan ${num(d.gpu_fan_rpm)} RPM`;
      if (el) el.textContent = fanText;
    }
  } catch {
    /* shown again on the next tick */
  }
}

renderPresetsMenu();
loadPresets();

export function show() {
  reload();
  fans();
  timer ??= setInterval(() => {
    fans();
    refresh();
  }, REFRESH_MS);
}

export function hide() {
  clearInterval(timer);
  timer = null;
}
