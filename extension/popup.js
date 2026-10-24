const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);

let tab = null;
let state = null;

// Promise form: Firefox's `browser.*` ignores callbacks; Chrome MV3 supports both.
// Bounded, so a stuck background never leaves the popup waiting forever.
async function send(msg, timeoutMs = 8000) {
  const timeout = new Promise((_, reject) =>
    setTimeout(() => reject(new Error("la extensión no responde; recárgala en la página de extensiones")), timeoutMs));
  const r = await Promise.race([api.runtime.sendMessage(msg), timeout]);
  if (!r?.ok) throw new Error(r?.error || "sin respuesta");
  return r.result;
}

function say(text, kind = "") {
  $("msg").textContent = text;
  $("msg").className = kind;
}

function human(n) {
  if (n == null) return "";
  const u = ["B", "KiB", "MiB", "GiB", "TiB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return i ? `${n.toFixed(1)} ${u[i]}` : `${n} B`;
}

function shortName(url) {
  try {
    const u = new URL(url);
    return decodeURIComponent(u.pathname.split("/").filter(Boolean).pop() || u.hostname);
  } catch {
    return url;
  }
}

function el(tag, props = {}, ...children) {
  const e = Object.assign(document.createElement(tag), props);
  e.append(...children);
  return e;
}

function mp3Kbps() {
  return Number($("mp3-quality").value);
}

async function download(d, button) {
  if (button) button.disabled = true;
  try {
    const r = await send({ type: "download", download: d });
    say(`Añadido a Boost (#${r.downloadId}).`, "ok");
    refreshDownloads();
  } catch (e) {
    say(e.message, "err");
  } finally {
    if (button) button.disabled = false;
  }
}

// ---------------------------------------------------------------- downloads

async function cancel(d, button) {
  button.disabled = true;
  try {
    await send({ type: "cancel", downloadId: d.id });
    say(`Descarga cancelada: ${d.filename || shortName(d.url)}`, "ok");
    refreshDownloads();
  } catch (e) {
    say(e.message, "err");
    button.disabled = false;
  }
}

/** Daemon errors in plain words; the raw text stays in the tooltip. */
function explain(error) {
  if (/403/.test(error)) return "el servidor denegó el acceso (enlace caducado o protegido)";
  if (/404/.test(error)) return "el vídeo ya no existe en el servidor";
  if (/50[0-9]|429/.test(error)) return "el servidor está ocupado; inténtalo en un rato";
  if (/Unsupported URL/i.test(error)) return "esta web no está soportada";
  if (/live streams/i.test(error)) return "es una emisión en directo";
  return error;
}

const RECENT_ERROR_SECS = 600;

function renderDownloads(list) {
  // In progress, plus failures of the last minutes: a download that fails at once
  // would otherwise look as if the button did nothing.
  const now = Date.now() / 1000;
  const failed = list.filter((d) => d.status === "error" && now - (d.created_at || 0) < RECENT_ERROR_SECS);
  const running = list.filter((d) => d.status === "downloading");
  $("downloads-section").hidden = running.length + failed.length === 0;
  $("downloads").replaceChildren(
    ...failed.map((d) => {
      const remove = el("button", { textContent: "Quitar", title: "Quitar de la lista" });
      remove.onclick = () => cancel(d, remove);
      return el(
        "li",
        { className: "failed" },
        el("div", { className: "line" },
          el("span", { className: "name", textContent: d.filename || shortName(d.url), title: d.url }),
          remove),
        el("div", { className: "meta", textContent: `Error: ${explain(d.error || "")}`, title: d.error || "" }),
      );
    }),
    ...running.map((d) => {
      const pct = d.size ? Math.min(100, (d.downloaded / d.size) * 100) : null;
      const size = d.size ? `${human(d.downloaded)} / ${human(d.size)}` : human(d.downloaded) || "tamaño desconocido";
      const meta = [pct != null ? `${pct.toFixed(1)} %` : null, size, d.speed ? `${human(Math.round(d.speed))}/s` : null]
        .filter(Boolean).join(" · ");
      const bar = el("div", { className: "bar" }, el("div"));
      if (pct == null) bar.classList.add("indeterminate");
      else bar.firstChild.style.width = `${pct}%`;
      const stop = el("button", { textContent: "Cancelar", title: "Detener y borrar lo descargado" });
      stop.onclick = () => cancel(d, stop);
      return el(
        "li",
        {},
        el("div", { className: "line" },
          el("span", { className: "name", textContent: d.filename || shortName(d.url), title: d.url }),
          stop),
        el("div", { className: "meta", textContent: meta, title: meta }),
        bar,
      );
    }),
  );
}

let pollTimer = null;
async function refreshDownloads() {
  clearTimeout(pollTimer);
  try {
    renderDownloads(await send({ type: "list" }));
  } catch {
    $("downloads-section").hidden = true;
  }
  pollTimer = setTimeout(refreshDownloads, 1000);
}

// -------------------------------------------------------------------- media

const MIME_TYPE = {
  "video/mp4": "MP4", "video/webm": "WEBM", "video/x-matroska": "MKV", "video/quicktime": "MOV",
  "video/x-flv": "FLV", "video/mp2t": "TS", "audio/mpeg": "MP3", "audio/mp4": "M4A", "audio/ogg": "OGG",
  "audio/flac": "FLAC", "audio/wav": "WAV", "audio/webm": "WEBA", "audio/aac": "AAC",
};
const AUDIO_EXT = /^(MP3|M4A|OGG|OPUS|FLAC|WAV|WEBA|AAC)$/;

const bare = (u) => u.split(/[?#]/)[0];

/**
 * File type the way IDM shows it ("MP4", "WEBM"...). The server's MIME type wins
 * over the URL: sites use made-up extensions (".vid") for plain MP4 files.
 */
function fileType(m) {
  if (MIME_TYPE[m.mime]) return MIME_TYPE[m.mime];
  const ext = /\.([a-z0-9]{2,5})$/i.exec(bare(m.url))?.[1]?.toUpperCase();
  return ext && /^(MP4|M4V|MKV|WEBM|MOV|AVI|FLV|TS|WMV|3GP)$/.test(ext) || AUDIO_EXT.test(ext ?? "") ? ext : "MP4";
}

const isAudio = (m) => m.kind === "file" && ((m.mime || "").startsWith("audio/") || AUDIO_EXT.test(fileType(m)));

/** CDN ids (jdq9qqo9ir1gjg.mp4) and player names (index.m3u8) say nothing; same rule as the daemon. */
function looksRandom(name) {
  const stem = name.replace(/\.[a-z0-9]{1,5}$/i, "");
  if (/^(index|master|video|videoplayback|playlist|file|download|stream|media|manifest)$/i.test(stem)) return true;
  if (/^index_/i.test(stem) || stem.includes(" ")) return /^index_/i.test(stem);
  if (stem.includes("~")) return true;
  const tokens = stem.split(/[^a-z0-9]/i);
  const hasWord = tokens.some((t) => t.length >= 4 && /^[a-z]+$/i.test(t));
  const hasId = tokens.some((t) => t.length >= 6 && (t.match(/\d/g) || []).length >= 2 && (t.match(/[a-z]/gi) || []).length >= 2);
  return !hasWord && hasId;
}

function displayName(m) {
  const name = shortName(m.url);
  return (m.kind !== "file" || looksRandom(name)) && state?.pageTitle ? state.pageTitle : name;
}

function duration(secs) {
  if (!secs) return null;
  const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60), s = Math.round(secs % 60);
  return h ? `${h} h ${m} min` : m ? `${m} min ${s} s` : `${s} s`;
}

function describe(m) {
  if (m.kind === "file") return [fileType(m), human(m.size)].filter(Boolean).join(" · ");
  const info = m.info;
  const kind = m.kind.toUpperCase();
  if (info === undefined) return `${kind} · comprobando…`;
  if (info === null) return `${kind} · no se pudo leer la lista; se intentará al descargar`;
  return [
    (info.container ?? "ts").toUpperCase(),
    info.height ? `${info.height}p` : null,
    duration(info.duration),
    info.encrypted ? "cifrado" : null,
    `${kind} → MP4`,
  ].filter(Boolean).join(" · ");
}

// The background decides what is listed (duplicates, variants, previews), so the
// popup shows exactly what the badge counts.
function renderMedia(list) {
  $("media-empty").hidden = list.length > 0;
  $("media-empty").textContent = state?.videoSite
    ? "En esta web el vídeo llega en fragmentos: usa los botones de arriba."
    : "Nada detectado todavía. Si no aparece, dale a reproducir y vuelve a abrir.";
  $("media").replaceChildren(
    ...list.map((m) => {
      const stream = m.kind !== "file";
      // What the player really sent; the tab URL is only a guess for older entries.
      const base = {
        url: m.url,
        referer: "referer" in m ? m.referer : tab?.url,
        // The daemon names streams after it, and files too when their own name is random.
        title: state?.pageTitle ?? null,
        // Lets the daemon fetch a fresh link if this one is bound to the browser's IP (VPN).
        page: tab?.url ?? null,
      };
      // Only a confirmed live stream is unusable. A failed inspection is not: the
      // daemon can still fetch a fresh link from the page when downloading.
      const usable = !m.info?.live;
      // The button names what lands on disk: streams become MP4, files keep their type.
      const type = stream ? "MP4" : fileType(m);
      const get = el("button", {
        textContent: type, className: "primary", disabled: !usable, title: `Descargar como ${type}`,
      });
      get.onclick = () => download({ ...base, video: stream }, get);
      const line = el("div", { className: "line" },
        el("span", { className: "name", textContent: displayName(m), title: m.url }),
        get,
      );
      if (!isAudio(m)) {
        const mp3 = el("button", { textContent: "MP3", title: "Descargar solo el audio en MP3", disabled: !usable });
        mp3.onclick = () => download({ ...base, video: true, audioKbps: mp3Kbps() }, mp3);
        line.append(mp3);
      }
      const meta = describe(m) + (m.info?.live ? " · directo (no soportado)" : "");
      return el("li", {}, line, el("div", { className: "meta", textContent: meta, title: m.url }));
    }),
  );
}

/** Streams still being inspected by the background: refresh the list until they are done. */
let mediaTimer = null;
function followInspections(list) {
  clearTimeout(mediaTimer);
  if (!list.some((m) => m.kind === "hls" && m.info === undefined)) return;
  mediaTimer = setTimeout(async () => {
    try {
      state.media = await send({ type: "media", tabId: tab?.id });
      renderMedia(state.media);
      followInspections(state.media);
    } catch { /* popup closing */ }
  }, 1000);
}

// ------------------------------------------------------------- page support

const PAGE_BUTTONS = ["page-best", "page-formats", "formats-go", "page-mp3"];

function setPageSupport(supported, via, checking = false) {
  for (const b of PAGE_BUTTONS) $(b).disabled = !supported;
  // Choosing among formats needs yt-dlp; resolved players offer one stream.
  $("page-formats").disabled = via !== "yt-dlp";
  const hint = checking
    ? "Comprobando si esta web se puede descargar…"
    : supported
      ? via === "player" ? "Reproductor reconocido: se descargará el vídeo que contiene." : "Usa yt-dlp: funciona en YouTube y cientos de webs."
      : "Esta web no está soportada para descarga directa. Usa «Medios detectados» (dale a reproducir si está vacío).";
  for (const id of ["page-hint", "audio-hint"]) {
    $(id).textContent = hint;
    $(id).hidden = false;
  }
}

async function checkPage() {
  setPageSupport(false, null, true);
  try {
    const info = await send({ type: "inspect", target: "page", url: tab.url });
    setPageSupport(info.supported, info.via);
  } catch {
    setPageSupport(false, null);
  }
}

// ------------------------------------------------------------- page / yt-dlp

function formatLabel(f) {
  const kind = f.vcodec && f.acodec ? "vídeo+audio" : f.vcodec ? "solo vídeo" : "solo audio";
  const res = f.resolution && f.resolution !== "audio only" ? f.resolution : "";
  return [res, f.fps ? `${Math.round(f.fps)}fps` : "", f.ext, kind, human(f.filesize), f.note || ""]
    .filter(Boolean).join(" · ");
}

async function loadFormats() {
  $("page-formats").disabled = true;
  say("Consultando formatos (yt-dlp)…");
  try {
    const info = await send({ type: "formats", url: tab.url }, 90000);
    const sel = $("formats");
    sel.replaceChildren(el("option", { value: "bv*+ba/b", textContent: "Mejor calidad disponible" }));
    for (const f of [...info.formats].reverse()) {
      // Video-only streams get the best audio merged in, as yt-dlp would do by default.
      const value = f.vcodec && !f.acodec ? `${f.id}+ba/${f.id}` : f.id;
      sel.append(el("option", { value, textContent: formatLabel(f) }));
    }
    $("formats-row").hidden = false;
    say(info.title);
  } catch (e) {
    say(e.message, "err");
  } finally {
    $("page-formats").disabled = false;
  }
}

// ------------------------------------------------------------------- refresh

async function refresh() {
  [tab] = await api.tabs.query({ active: true, currentWindow: true });
  try {
    state = await send({ type: "state", tabId: tab?.id });
  } catch (e) {
    state = { hostError: e.message, media: [] };
  }
  const st = $("status");
  // Boost closed: the only thing to do is open it.
  $("launch").hidden = state.daemonUp;
  for (const id of ["permission", "downloads-section", "page", "audio", "media-section"]) {
    if (!state.daemonUp) $(id).hidden = true;
  }
  if (!state.daemonUp) {
    st.textContent = "cerrado";
    st.className = "err";
    return;
  }
  st.textContent = "conectado";
  st.className = "ok";
  say("");
  $("media-section").hidden = false;
  $("permission").hidden = !state.needsPermission;
  const web = /^https?:/i.test(tab?.url || "");
  $("page").hidden = !web;
  $("audio").hidden = !web;
  renderMedia(state.media || []);
  followInspections(state.media || []);
  if (web) checkPage();
  refreshDownloads();
}

$("page-best").onclick = (e) => download({ url: tab.url, video: true }, e.target);
$("page-mp3").onclick = (e) => download({ url: tab.url, video: true, audioKbps: mp3Kbps() }, e.target);
$("page-formats").onclick = loadFormats;
$("formats-go").onclick = (e) => download({ url: tab.url, video: true, format: $("formats").value }, e.target);
$("launch-go").onclick = async (e) => {
  e.target.disabled = true;
  say("Abriendo Boost…");
  try {
    const r = await send({ type: "launch" }, 20_000);
    if (r.running) {
      await refresh();
    } else {
      // The browser asks first whether to open Boost; once it runs, reopen this panel.
      say("Confirma en el navegador que quieres abrir Boost.");
    }
  } catch (err) {
    say(err.message, "err");
  } finally {
    e.target.disabled = false;
  }
};
$("grant").onclick = async () => {
  if (await api.permissions.request({ origins: ["<all_urls>"] })) refresh();
};

// Remember the chosen MP3 quality between popups.
api.storage.local.get("mp3Kbps").then((v) => {
  if (v.mp3Kbps != null) $("mp3-quality").value = String(v.mp3Kbps);
});
$("mp3-quality").onchange = () => api.storage.local.set({ mp3Kbps: mp3Kbps() });

refresh();
