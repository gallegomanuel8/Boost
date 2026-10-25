// Boost window: talks to the service through the Rust commands in src/main.rs.
"use strict";

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (id) => document.getElementById(id);

// ------------------------------------------------------------------ helpers

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "style") e.style.cssText = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) if (c != null) e.append(c);
  return e;
}

const nf1 = new Intl.NumberFormat("es-ES", { maximumFractionDigits: 1 });
function bytes(n) {
  if (n == null) return "—";
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return (i === 0 ? n : nf1.format(n)) + " " + u[i];
}

function duration(s) {
  if (!isFinite(s) || s < 0) return "";
  s = Math.round(s);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), r = s % 60;
  if (h) return `${h} h ${m} min`;
  if (m) return `${m} min ${r} s`;
  return `${r} s`;
}

let toastTimer;
function toast(msg) {
  const t = $("toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 4000);
}

async function call(cmd, args) {
  try {
    const r = await invoke(cmd, args);
    setOffline(false);
    return r;
  } catch (e) {
    const msg = String(e);
    if (/not running|no service|connect/i.test(msg)) setOffline(true);
    throw msg;
  }
}

function setOffline(off) {
  const b = $("banner");
  b.hidden = !off;
  if (off) b.textContent = "No se puede contactar con el servicio de descargas. Reintentando…";
}

// --------------------------------------------------------------- file kinds

const KINDS = {
  video: ["mp4", "mkv", "webm", "ts", "mov", "avi", "m4v", "flv", "wmv", "mpg", "mpeg", "3gp", "m3u8", "mpd"],
  audio: ["mp3", "m4a", "aac", "flac", "ogg", "opus", "wav", "wma", "alac"],
  doc: ["pdf", "doc", "docx", "odt", "ods", "odp", "xls", "xlsx", "ppt", "pptx", "txt", "epub", "rtf", "csv"],
};
const KIND_LABEL = { video: "Vídeo", audio: "Audio", prog: "Programa o archivo", doc: "Documento" };

function extOf(name) {
  const m = /\.([a-z0-9]{1,5})$/i.exec(name || "");
  return m ? m[1].toLowerCase() : "";
}

function urlFileName(url) {
  try {
    const p = new URL(url).pathname;
    return decodeURIComponent(p.slice(p.lastIndexOf("/") + 1));
  } catch { return ""; }
}

function kindOf(d) {
  const ext = extOf(d.filename) || extOf(urlFileName(d.url));
  for (const [k, list] of Object.entries(KINDS)) if (list.includes(ext)) return k;
  return d.video ? "video" : "prog";
}

function displayName(d) {
  return d.filename || urlFileName(d.url) || d.url;
}

// -------------------------------------------------------------------- state

const downloads = new Map();
// Selected downloads; `focus` is the last one clicked: shown in the details
// panel and the anchor of Shift+click ranges.
const selected = new Set();
let focus = null;
let category = "all";
let query = "";
let settings = { speed_limit_kib: 0, max_concurrent: 3, download_dir: "" };
let ownsService = true;

const STATUS = { downloading: "Descargando", queued: "En cola", paused: "En pausa", completed: "Completada", error: "Error" };
const ACTIVE = ["downloading", "queued", "paused"];
const CAN_RESUME = ["paused", "error"];
const CAN_PAUSE = ["downloading", "queued"];
const canResume = (d) => CAN_RESUME.includes(d.status);
const canPause = (d) => CAN_PAUSE.includes(d.status);
// Finished but deleted from disk, or failed: fetch it again from the start.
const canRestart = (d) => d.file_missing || d.status === "error";
const statusText = (d) => (d.file_missing ? "Fichero eliminado" : STATUS[d.status] || d.status);

const CATEGORIES = [
  { heading: "Estado" },
  { key: "all", label: "Todas", test: () => true },
  { key: "active", label: "En curso", test: (d) => ACTIVE.includes(d.status) },
  { key: "completed", label: "Completadas", test: (d) => d.status === "completed" },
  { key: "error", label: "Con errores", test: (d) => d.status === "error" },
  { heading: "Tipo" },
  { key: "video", label: "Vídeo", test: (d) => kindOf(d) === "video" },
  { key: "audio", label: "Audio", test: (d) => kindOf(d) === "audio" },
  { key: "prog", label: "Programas y archivos", test: (d) => kindOf(d) === "prog" },
  { key: "doc", label: "Documentos", test: (d) => kindOf(d) === "doc" },
];

function selection() {
  return [...selected].map((id) => downloads.get(id)).filter(Boolean);
}

function selectOnly(id) {
  selected.clear();
  if (id != null) selected.add(id);
  focus = id;
}

/// Click on a row: plain = only this one, Ctrl = toggle, Shift = range from `focus`.
function clickRow(e, id) {
  if (e.shiftKey && focus != null) {
    const list = visible().map((d) => d.id);
    const a = list.indexOf(focus), b = list.indexOf(id);
    if (a >= 0 && b >= 0) {
      if (!e.ctrlKey && !e.metaKey) selected.clear();
      for (let i = Math.min(a, b); i <= Math.max(a, b); i++) selected.add(list[i]);
    } else selectOnly(id);
  } else if (e.ctrlKey || e.metaKey) {
    if (selected.has(id)) selected.delete(id); else selected.add(id);
    focus = selected.has(id) ? id : [...selected].pop() ?? null;
  } else {
    selectOnly(id);
  }
  render();
  refreshDetails();
}

function forget(id) {
  downloads.delete(id);
  selected.delete(id);
  if (focus === id) focus = [...selected].pop() ?? null;
}

// ------------------------------------------------------------------- render

let renderQueued = false;
function render() {
  if (renderQueued) return;
  renderQueued = true;
  requestAnimationFrame(() => {
    renderQueued = false;
    renderCategories();
    renderRows();
    renderDetails();
    renderFooter();
    renderToolbar();
  });
}

function sorted() {
  // Newest first, like a download history.
  return [...downloads.values()].sort((a, b) => b.id - a.id);
}

function renderCategories() {
  const all = sorted();
  const nav = $("cats");
  nav.replaceChildren(
    ...CATEGORIES.map((c) =>
      c.heading
        ? el("h3", {}, c.heading)
        : el("button", {
            type: "button",
            "aria-current": c.key === category ? "true" : null,
            onclick: () => { category = c.key; render(); },
          }, el("span", {}, c.label), el("span", { class: "count" }, String(all.filter(c.test).length)))
    ),
    el("h3", {}, "Colas"),
    el("button", { type: "button", disabled: true, title: "El programador llega en la próxima versión" },
      el("span", {}, `Principal (${settings.max_concurrent} a la vez)`),
      el("span", { class: "count" }, String(all.filter((d) => ACTIVE.includes(d.status)).length))),
  );
}

function visible() {
  const cat = CATEGORIES.find((c) => c.key === category) || CATEGORIES[1];
  const q = query.trim().toLowerCase();
  return sorted().filter((d) => cat.test(d) && (!q || displayName(d).toLowerCase().includes(q) || d.url.toLowerCase().includes(q)));
}

function pct(d) {
  if (d.status === "completed") return 100;
  return d.size ? Math.min(100, (d.downloaded / d.size) * 100) : null;
}

function rowFor(d) {
  const kind = kindOf(d);
  const p = pct(d);
  const active = d.status === "downloading";
  const ext = extOf(displayName(d));
  const left = active && d.size && d.speed > 0 ? duration((d.size - d.downloaded) / d.speed) : "";
  const typeText = [ext ? ext.toUpperCase() : KIND_LABEL[kind], d.video ? "vídeo web" : null,
    active && d.connections > 1 ? `${d.connections} conexiones` : null].filter(Boolean).join(" · ");
  const pctText = d.status === "error" || d.file_missing ? "—" : p != null ? nf1.format(p) + " %" : d.downloaded ? bytes(d.downloaded) : "";
  return el("button", {
    type: "button", role: "row", class: "row", "aria-selected": selected.has(d.id) ? "true" : "false",
    onclick: (e) => clickRow(e, d.id),
    ondblclick: () => reveal(d.id),
    oncontextmenu: (e) => {
      e.preventDefault();
      // Right-click outside the selection acts on that row alone, as in file managers.
      if (!selected.has(d.id)) { selectOnly(d.id); render(); refreshDetails(); }
      openMenu(e.clientX, e.clientY);
    },
  },
    el("span", { role: "cell", class: "cell-name" },
      el("span", { class: `kind ${kind}` }, (ext || kind).slice(0, 4).toUpperCase()),
      el("span", { class: "name-col" },
        el("span", { class: "n", title: displayName(d) }, displayName(d)),
        el("span", { class: "muted small" }, typeText))),
    el("span", { role: "cell", class: "muted num" }, d.size ? bytes(d.size) : d.status === "completed" ? bytes(d.downloaded) : "—"),
    el("span", { role: "cell", class: "prog" },
      el("span", { class: "small num" }, pctText),
      el("span", { class: `bar ${d.status}` }, el("span", { style: `width: ${d.file_missing ? 0 : p ?? 0}%` }))),
    el("span", { role: "cell", class: "num" }, active && d.speed > 0 ? bytes(d.speed) + "/s" : ""),
    el("span", { role: "cell", class: "muted num" }, left),
    el("span", { role: "cell", class: `st ${d.file_missing ? "missing" : d.status}` }, statusText(d)),
  );
}

function renderRows() {
  const rows = visible();
  $("rows").replaceChildren(...rows.map(rowFor));
  $("empty").hidden = rows.length > 0;
}

// Segments of the focused download, refreshed while it is shown.
let detail = null;

function renderDetails() {
  const sel = selection();
  $("details").hidden = sel.length === 0;
  if (sel.length > 1) return renderMultiDetails(sel);
  const d = downloads.get(focus) || sel[0];
  if (!d) return;
  $("d-name").textContent = displayName(d);
  $("d-url").textContent = d.url;
  $("d-url").title = d.url;
  $("d-folder").hidden = false;
  $("d-folder").textContent = "Carpeta: " + d.dir;
  $("d-error").hidden = d.status !== "error";
  $("d-error").textContent = d.error ? explain(d.error) : "";
  $("btn-reveal").hidden = false;
  $("btn-restart").hidden = !canRestart(d);

  const p = pct(d);
  let conn = STATUS[d.status];
  if (d.status === "downloading") {
    conn = d.connections > 1
      ? `${d.connections} conexiones activas · segmentación dinámica`
      : "Descargando";
    if (d.speed > 0) conn += ` · ${bytes(d.speed)}/s`;
  } else if (d.status === "paused" && p != null) {
    conn = `En pausa · se reanudará desde el ${nf1.format(p)} %`;
  } else if (d.file_missing) {
    conn = "Descargado, pero el fichero ya no está en la carpeta";
  }
  $("d-conn").textContent = conn + (d.size ? ` · ${bytes(d.downloaded)} de ${bytes(d.size)}` : "");

  const map = $("d-map");
  map.className = `segmap ${d.status}`;
  const segs = detail && detail.info.id === d.id ? detail.segments : [];
  const size = d.size || (segs.length ? segs[segs.length - 1][1] : 0);
  if (segs.length && size && d.status !== "completed") {
    map.replaceChildren(...segs.map(([start, end, done]) => {
      const len = Math.max(1, end - start);
      return el("span", { style: `left: ${(start / size) * 100}%; width: ${(len / size) * 100}%` },
        el("span", { style: `width: ${Math.min(100, (done / len) * 100)}%` }));
    }));
  } else {
    map.replaceChildren(el("span", { style: "left: 0; width: 100%" }, el("span", { style: `width: ${d.file_missing ? 0 : p ?? 0}%` })));
  }
}

/// Several downloads selected: totals instead of one download's details.
function renderMultiDetails(sel) {
  $("d-name").textContent = `${sel.length} descargas seleccionadas`;
  const counts = Object.entries(STATUS)
    .map(([k, label]) => [label, sel.filter((d) => d.status === k && !d.file_missing).length])
    .concat([["Fichero eliminado", sel.filter((d) => d.file_missing).length]])
    .filter(([, n]) => n > 0)
    .map(([label, n]) => `${n} ${label.toLowerCase()}`);
  $("d-url").textContent = counts.join(" · ");
  $("d-url").title = "";
  $("d-folder").hidden = true;
  $("d-error").hidden = true;
  $("btn-reveal").hidden = true;
  $("btn-restart").hidden = !sel.some(canRestart);
  const known = sel.filter((d) => d.size);
  const size = known.reduce((s, d) => s + d.size, 0);
  const done = known.reduce((s, d) => s + (d.status === "completed" ? d.size : Math.min(d.downloaded, d.size)), 0);
  const speed = sel.reduce((s, d) => s + (d.status === "downloading" ? d.speed || 0 : 0), 0);
  $("d-conn").textContent = (size ? `${bytes(done)} de ${bytes(size)}` : "Tamaño desconocido") +
    (speed > 0 ? ` · ${bytes(speed)}/s` : "");
  const map = $("d-map");
  map.className = "segmap";
  map.replaceChildren(el("span", { style: "left: 0; width: 100%" }, el("span", { style: `width: ${size ? (done / size) * 100 : 0}%` })));
}

// Same wording as the browser popup for the usual failures.
function explain(err) {
  if (/403|forbidden/i.test(err)) return "El servidor denegó el acceso (el enlace puede haber caducado). Recarga la página y vuelve a descargar.";
  if (/404|not found/i.test(err)) return "El fichero ya no existe en el servidor.";
  if (/unsupported url|unsupported/i.test(err)) return "No se reconoce ningún vídeo en esta página.";
  if (/no space/i.test(err)) return "No queda espacio en el disco.";
  return err;
}

function renderFooter() {
  const all = [...downloads.values()];
  const running = all.filter((d) => d.status === "downloading");
  const queued = all.filter((d) => d.status === "queued").length;
  const total = running.reduce((s, d) => s + (d.speed || 0), 0);
  $("f-active").textContent = `${running.length} ${running.length === 1 ? "descarga activa" : "descargas activas"} · ${queued} en cola`;
  $("f-speed").textContent = "Velocidad total: " + (total > 0 ? bytes(total) + "/s" : "—");
  $("f-limit").textContent = "Límite: " + (settings.speed_limit_kib > 0 ? bytes(settings.speed_limit_kib * 1024) + "/s" : "sin límite");
  $("f-engine").replaceChildren(el("span", { class: "dot" }),
    ownsService ? "Motor en esta ventana" : "Motor: servicio en segundo plano");
}

function renderToolbar() {
  const sel = selection();
  $("btn-resume").disabled = !sel.some(canResume);
  $("btn-pause").disabled = !sel.some(canPause);
  $("btn-remove").disabled = sel.length === 0;
  const anyRunning = [...downloads.values()].some((x) => CAN_PAUSE.includes(x.status));
  const anyPaused = [...downloads.values()].some((x) => x.status === "paused");
  const btn = $("btn-pause-all");
  btn.textContent = anyRunning || !anyPaused ? "Pausar todas" : "Reanudar todas";
  btn.disabled = !anyRunning && !anyPaused;
}

// --------------------------------------------------------------------- data

async function reload() {
  const list = await call("list");
  downloads.clear();
  for (const d of list) downloads.set(d.id, d);
  for (const id of [...selected]) if (!downloads.has(id)) selected.delete(id);
  if (focus != null && !downloads.has(focus)) focus = [...selected].pop() ?? null;
  render();
}

async function refreshDetails() {
  const d = selected.size === 1 ? downloads.get(focus) : null;
  if (!d) { detail = null; return; }
  try {
    detail = await call("details", { id: d.id });
    downloads.set(d.id, { ...downloads.get(d.id), ...detail.info });
    render();
  } catch { /* the list still shows it */ }
}

async function reveal(id) {
  try { await call("reveal", { id }); } catch (e) { toast("No se puede abrir la carpeta: " + e); }
}

/// Runs `cmd` on every selected download it applies to; reports the failures together.
async function forSelection(cmd, applies) {
  const targets = selection().filter(applies);
  const failed = [];
  await Promise.all(targets.map((d) => call(cmd, { id: d.id }).catch((e) => failed.push(`${displayName(d)}: ${e}`))));
  if (failed.length) toast(failed.join("\n"));
}

async function act(fn) {
  try { await fn(); } catch (e) { toast(String(e)); }
}

// ------------------------------------------------------------ main actions

$("search").addEventListener("input", (e) => { query = e.target.value; render(); });
$("btn-resume").onclick = () => forSelection("resume", canResume);
$("btn-pause").onclick = () => forSelection("pause", canPause);
$("btn-restart").onclick = () => forSelection("restart", canRestart);
$("btn-reveal").onclick = () => focus != null && reveal(focus);
$("btn-pause-all").onclick = () => act(() => call($("btn-pause-all").textContent.startsWith("Pausar") ? "pause_all" : "resume_all"));

let removing = [];
$("btn-remove").onclick = () => {
  removing = selection();
  if (!removing.length) return;
  $("rm-title").textContent = removing.length === 1 ? "Eliminar descarga" : `Eliminar ${removing.length} descargas`;
  $("rm-text").textContent = removing.length === 1
    ? `«${displayName(removing[0])}» se quitará de la lista.`
    : `Las ${removing.length} descargas seleccionadas se quitarán de la lista.`;
  $("rm-file").checked = false;
  $("rm-file").parentElement.hidden = !removing.some((d) => d.status === "completed");
  $("dlg-remove").returnValue = "";
  $("dlg-remove").showModal();
};
$("dlg-remove").addEventListener("close", () => {
  if ($("dlg-remove").returnValue !== "ok") return;
  const deleteFile = $("rm-file").checked;
  const failed = [];
  act(async () => {
    await Promise.all(removing.map((d) =>
      call("remove", { id: d.id, deleteFile: deleteFile && d.status === "completed" })
        .then(() => forget(d.id))
        .catch((e) => failed.push(`${displayName(d)}: ${e}`))));
    removing = [];
    if (failed.length) toast(failed.join("\n"));
    render();
  });
});
$("rm-go").value = "ok";

// Context menu of the rows: acts on the selection.
const menu = $("ctx");
function openMenu(x, y) {
  const sel = selection();
  $("ctx-resume").disabled = !sel.some(canResume);
  $("ctx-pause").disabled = !sel.some(canPause);
  $("ctx-restart").hidden = !sel.some(canRestart);
  $("ctx-reveal").disabled = sel.length !== 1;
  $("ctx-remove").textContent = sel.length > 1 ? `Eliminar ${sel.length} descargas…` : "Eliminar…";
  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  menu.style.left = Math.min(x, innerWidth - r.width - 4) + "px";
  menu.style.top = Math.min(y, innerHeight - r.height - 4) + "px";
  menu.querySelector("button:not(:disabled)")?.focus();
}
function closeMenu() { menu.hidden = true; }
$("ctx-resume").onclick = () => { closeMenu(); forSelection("resume", canResume); };
$("ctx-pause").onclick = () => { closeMenu(); forSelection("pause", canPause); };
$("ctx-restart").onclick = () => { closeMenu(); forSelection("restart", canRestart); };
$("ctx-reveal").onclick = () => { closeMenu(); if (focus != null) reveal(focus); };
$("ctx-remove").onclick = () => { closeMenu(); $("btn-remove").click(); };
document.addEventListener("mousedown", (e) => { if (!menu.hidden && !menu.contains(e.target)) closeMenu(); });
window.addEventListener("blur", closeMenu);
$("rows").parentElement.parentElement.addEventListener("scroll", closeMenu);
// No browser menu (reload, inspect…) outside text fields.
document.addEventListener("contextmenu", (e) => { if (!e.target.matches("input, textarea")) e.preventDefault(); });

document.addEventListener("keydown", (e) => {
  if (!menu.hidden && e.key === "Escape") { closeMenu(); return; }
  if (document.querySelector("dialog[open]") || e.target.matches("input, select")) return;
  if (e.key === "Delete" && selected.size) $("btn-remove").click();
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "n") { e.preventDefault(); openAdd(); }
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
    e.preventDefault();
    for (const d of visible()) selected.add(d.id);
    render();
  }
  if ((e.key === "ArrowDown" || e.key === "ArrowUp") && visible().length) {
    e.preventDefault();
    const list = visible();
    const i = list.findIndex((d) => d.id === focus);
    const next = list[Math.max(0, Math.min(list.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
    if (e.shiftKey) { selected.add(next.id); focus = next.id; } else selectOnly(next.id);
    render();
    refreshDetails();
  }
});

for (const b of document.querySelectorAll("[data-close]")) {
  b.addEventListener("click", () => b.closest("dialog").close("cancel"));
}

// ------------------------------------------------------------ new download

const add = { mode: "file", format: null, info: null, probeSeq: 0 };
const mp3Default = () => { try { return localStorage.getItem("mp3Kbps") || "192"; } catch { return "192"; } };

async function openAdd(url = "") {
  $("add-url").value = url;
  $("add-name").value = "";
  $("add-dir").value = settings.download_dir || (await call("default_dir").catch(() => ""));
  $("add-kbps").value = mp3Default();
  resetProbe();
  $("dlg-add").showModal();
  $("add-url").focus();
  if (!url) {
    // Offer what is on the clipboard when it looks like a link.
    try {
      const clip = (await navigator.clipboard.readText()).trim();
      if (/^https?:\/\/\S+$/i.test(clip) && !$("add-url").value) { $("add-url").value = clip; $("add-url").select(); probe(); }
    } catch { /* no clipboard permission */ }
  } else probe();
}
$("btn-add").onclick = () => openAdd();

function resetProbe() {
  add.mode = "file"; add.format = null; add.info = null; add.probeSeq++;
  $("add-probe").hidden = true;
  $("add-modes").hidden = true;
  $("add-formats").hidden = true;
  $("add-mp3").hidden = true;
  $("add-go").disabled = false;
}

function setProbe(kind, title, meta) {
  const badge = $("add-badge");
  badge.textContent = { video: "VÍDEO", audio: "AUDIO", doc: "DOC", prog: "FICHERO", wait: "…" }[kind];
  badge.style.setProperty("--kc", kind === "wait" ? "var(--muted)" : `var(--k-${kind})`);
  $("add-ptitle").textContent = title;
  $("add-pmeta").textContent = meta;
  $("add-probe").hidden = false;
}

let probeTimer;
$("add-url").addEventListener("input", () => { clearTimeout(probeTimer); probeTimer = setTimeout(probe, 600); });

async function probe() {
  resetProbe();
  const url = $("add-url").value.trim();
  if (!/^https?:\/\/[^/\s]+/i.test(url)) return;
  const name = urlFileName(url);
  const ext = extOf(name);
  // A link to a file: the engine downloads it directly, no need to ask yt-dlp.
  if (ext && !["html", "htm", "php", "asp", "aspx", "m3u8", "mpd"].includes(ext)) {
    const kind = kindOf({ filename: name, url });
    setProbe(kind, name, `${KIND_LABEL[kind]} · descarga directa con varias conexiones`);
    if (kind === "video") showModes();
    return;
  }
  const seq = add.probeSeq;
  setProbe("wait", "Analizando la página…", "Buscando vídeos con yt-dlp");
  $("add-go").disabled = true;
  try {
    const info = await call("formats", { url });
    if (seq !== add.probeSeq) return;
    add.info = info;
    const host = new URL(url).hostname.replace(/^www\./, "");
    const meta = [host, info.duration ? duration(info.duration) : null, "detectado con yt-dlp"].filter(Boolean).join(" · ");
    const hasVideo = info.formats.some((f) => f.vcodec);
    setProbe(hasVideo ? "video" : "audio", info.title || name || url, meta);
    if (!$("add-name").value && info.title) $("add-name").placeholder = info.title;
    if (hasVideo) showModes(); else pickMode("audio", false);
  } catch (e) {
    if (seq !== add.probeSeq) return;
    setProbe("prog", name || url, "No se ha reconocido ningún vídeo: se descargará como fichero");
  } finally {
    if (seq === add.probeSeq) $("add-go").disabled = false;
  }
}

function showModes() {
  $("add-modes").hidden = false;
  pickMode("video");
}

function pickMode(mode, showChoices = true) {
  add.mode = mode;
  for (const b of $("add-modes").querySelectorAll(".choice")) b.setAttribute("aria-checked", String(b.dataset.mode === mode));
  if (!showChoices) $("add-modes").hidden = true;
  $("add-mp3").hidden = mode !== "audio";
  renderFormats();
  $("add-go").textContent = mode === "audio" ? "Descargar MP3" : "Descargar";
}
for (const b of document.querySelectorAll("#add-modes .choice")) b.onclick = () => pickMode(b.dataset.mode);

function height(f) {
  const m = /(\d+)x(\d+)/.exec(f.resolution || "") || /(\d+)p/.exec(f.resolution || "");
  return m ? Number(m[2] ?? m[1]) : 0;
}

/// One entry per resolution, best bitrate first; video-only formats get the best audio added.
function videoChoices(info) {
  const best = new Map();
  for (const f of info.formats) {
    if (!f.vcodec) continue;
    const h = height(f);
    const cur = best.get(h);
    if (!cur || (f.tbr || 0) > (cur.tbr || 0)) best.set(h, f);
  }
  return [...best.entries()].sort((a, b) => b[0] - a[0]).map(([h, f]) => ({
    format: f.acodec ? f.id : `${f.id}+bestaudio/best`,
    res: h ? `${h}p` : f.resolution || f.id,
    detail: [f.ext.toUpperCase(), f.fps ? `${Math.round(f.fps)} fps` : null, f.note].filter(Boolean).join(" · "),
    size: f.filesize ? (f.acodec ? "" : "≈ ") + bytes(f.filesize) : "",
  }));
}

function renderFormats() {
  const box = $("add-formats");
  if (add.mode !== "video" || !add.info) { box.hidden = true; add.format = null; return; }
  const choices = [{ format: null, res: "Mejor", detail: "La mejor calidad disponible", size: "" }, ...videoChoices(add.info)];
  box.replaceChildren(...choices.map((c) => el("button", {
    type: "button", role: "radio", "aria-checked": String(c.format === add.format),
    onclick: () => { add.format = c.format; renderFormats(); },
  }, el("span", { class: "fl" }, el("span", { class: "radio" }), el("span", { class: "res" }, c.res), el("span", { class: "muted ellipsis" }, c.detail)),
     el("span", { class: "muted num" }, c.size))));
  box.hidden = false;
}

$("add-pick").onclick = async () => {
  const dir = await call("pick_folder").catch(() => null);
  if (dir) $("add-dir").value = dir;
};

async function submitAdd(paused) {
  const url = $("add-url").value.trim();
  if (!/^https?:\/\//i.test(url)) { toast("Escribe una dirección http:// o https://"); return; }
  const name = $("add-name").value.trim();
  const ext = extOf(urlFileName(url));
  const isStream = ["m3u8", "mpd"].includes(ext);
  const video = add.mode === "audio"
    ? { format: null, audio_kbps: Number($("add-kbps").value) }
    : add.info || isStream ? { format: add.format, audio_kbps: null } : null;
  const download = {
    request: { url, headers: [] },
    dir: $("add-dir").value.trim() || null,
    filename: name && !video ? name : null,
    title: video ? name || add.info?.title || null : null,
    paused,
    video,
  };
  try {
    const id = await call("add", { download });
    $("dlg-add").close("ok");
    selectOnly(id);
    await reload();
  } catch (e) { toast("No se pudo añadir: " + e); }
}
$("add-form").addEventListener("submit", (e) => { e.preventDefault(); submitAdd(false); });
$("add-later").onclick = () => submitAdd(true);

// ---------------------------------------------------------------- settings

function showTab(name) {
  for (const b of document.querySelectorAll(".tabs button")) b.setAttribute("aria-current", String(b.dataset.tab === name));
  for (const s of document.querySelectorAll("[data-panel]")) s.hidden = s.dataset.panel !== name;
  if (name === "browsers") loadExtensions();
}

// ----------------------------------------------------------- web extensions

async function loadExtensions() {
  let list;
  try { list = await call("web_extensions"); } catch (e) { toast(String(e)); return; }
  $("ext-list").replaceChildren(...list.map((x) => {
    const state = x.installed ? "Extensión instalada"
      : x.outdated ? "Tienes la extensión antigua (raudo): instala la nueva y quita la antigua"
      : !x.present ? "No se ha encontrado este navegador"
      : x.available ? "Extensión no instalada"
      : x.browser === "firefox" ? "Extensión no instalada · falta el fichero firmado; reinstala Boost"
      : "Extensión no instalada · falta su carpeta; reinstala Boost";
    return el("div", { role: "row", class: "ext-row" },
      el("span", { role: "cell" }, el("span", { class: "strong" }, x.name), el("span", { class: "muted small" }, state)),
      el("span", { role: "cell" }, el("button", {
        type: "button",
        class: x.installed ? "btn done" : "btn primary",
        disabled: x.installed,
        onclick: () => installExtension(x),
      }, x.installed ? "Instalada" : "Instalar extensión")));
  }));
}

// Address of the extensions page in each Chromium browser.
const EXTENSIONS_PAGE = { brave: "brave://extensions", edge: "edge://extensions" };

async function installExtension(x) {
  if (!x.available) { toast("No se encuentra la extensión. Vuelve a instalar Boost."); return; }
  try {
    await call("install_extension", { browser: x.browser });
  } catch (e) { toast(String(e)); return; }
  if (x.browser === "firefox") {
    toast("Firefox pedirá confirmación para añadir la extensión.");
    return;
  }
  // Chromium: the folder is open; walk the user through "Load unpacked".
  const url = EXTENSIONS_PAGE[x.browser] || "chrome://extensions";
  $("ext-title").textContent = `Instalar la extensión en ${x.name}`;
  $("ext-browser").textContent = x.name;
  $("ext-url").textContent = url;
  $("ext-folder").textContent = x.folder;
  $("ext-copy").onclick = () => navigator.clipboard.writeText(url)
    .then(() => toast("Copiado: pégalo en la barra de direcciones"))
    .catch(() => toast(url));
  $("dlg-ext").showModal();
}

// Picks up an installation made in the browser while the tab is open.
setInterval(() => {
  if ($("dlg-settings").open && !document.querySelector('[data-panel="browsers"]').hidden) loadExtensions();
}, 3000);
for (const b of document.querySelectorAll(".tabs button")) b.onclick = () => showTab(b.dataset.tab);

$("st-autostart").onclick = (e) => {
  const sw = e.currentTarget;
  sw.setAttribute("aria-checked", String(sw.getAttribute("aria-checked") !== "true"));
};

$("btn-settings").onclick = async () => {
  await loadSettings();
  $("st-max").value = settings.max_concurrent;
  $("st-limit").value = settings.speed_limit_kib;
  $("st-kbps").value = mp3Default();
  $("st-dir").textContent = settings.download_dir;
  $("st-engine").textContent = ownsService
    ? "Las descargas se ejecutan dentro de esta aplicación: al cerrar la ventana siguen en la bandeja; «Salir» las pausa."
    : "Las descargas las ejecuta el servicio en segundo plano (raudo-daemon); esta ventana es solo un cliente.";
  const auto = await call("autostart_enabled").catch(() => false);
  $("st-autostart").setAttribute("aria-checked", String(auto));
  $("st-autostart").dataset.was = String(auto);
  showTab("general");
  $("dlg-settings").showModal();
};

$("st-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const max = Math.max(1, Math.min(20, Math.round(Number($("st-max").value) || 1)));
  const limit = Math.max(0, Math.round(Number($("st-limit").value) || 0));
  try {
    if (max !== settings.max_concurrent) await call("set_max_concurrent", { n: max });
    if (limit !== settings.speed_limit_kib) await call("set_speed_limit", { kib: limit });
    const auto = $("st-autostart").getAttribute("aria-checked") === "true";
    if (String(auto) !== $("st-autostart").dataset.was) await call("set_autostart", { enabled: auto });
    try { localStorage.setItem("mp3Kbps", $("st-kbps").value); } catch { /* per-window convenience */ }
    $("dlg-settings").close("ok");
    await loadSettings();
    render();
  } catch (err) { toast("No se pudieron guardar las preferencias: " + err); }
});

async function loadSettings() {
  try { settings = await call("settings"); } catch { /* keep the last known */ }
}

// ------------------------------------------------------------------ events

listen("raudo-event", ({ payload }) => {
  if (payload.event === "changed") {
    const info = payload.info;
    const prev = downloads.get(info.id);
    downloads.set(info.id, info);
    if (info.id === focus && (!prev || prev.status !== info.status)) refreshDetails();
  } else if (payload.event === "removed") {
    forget(payload.id);
  }
  render();
});

// Segment map of the focused download, and a periodic resync in case an event was missed.
setInterval(() => {
  const d = selected.size === 1 && downloads.get(focus);
  if (d && d.status === "downloading") refreshDetails();
}, 1000);
setInterval(() => reload().catch(() => {}), 10000);

(async function start() {
  ownsService = await call("owns_service").catch(() => true);
  // The service may still be opening its database when the window appears.
  for (let i = 0; i < 20; i++) {
    try { await loadSettings(); await reload(); break; } catch { await new Promise((r) => setTimeout(r, 500)); }
  }
  render();
})();
