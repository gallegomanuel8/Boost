const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);
const VIDEO_EXT = /\.(mp4|m4v|mkv|webm|mov|avi|flv|ts)(\?|$)/i;

let tab = null;
let state = null;

// Promise form: Firefox's `browser.*` ignores callbacks; Chrome MV3 supports both.
async function send(msg) {
  const r = await api.runtime.sendMessage(msg);
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
    say(`Añadido a raudo (#${r.downloadId}).`, "ok");
    refreshDownloads();
  } catch (e) {
    say(e.message, "err");
  } finally {
    if (button) button.disabled = false;
  }
}

// ---------------------------------------------------------------- downloads

const STATUS_LABEL = { queued: "En cola", paused: "Pausada", error: "Error", completed: "Completada" };

function renderDownloads(list) {
  // Everything unfinished, plus the five most recent finished ones.
  const active = list.filter((d) => d.status !== "completed");
  const shown = active.concat(list.filter((d) => d.status === "completed").slice(0, 5));
  $("downloads-section").hidden = shown.length === 0;
  const ul = $("downloads");
  ul.replaceChildren(
    ...shown.map((d) => {
      const pct = d.size ? Math.min(100, (d.downloaded / d.size) * 100) : null;
      const size = d.size ? `${human(d.downloaded)} / ${human(d.size)}` : human(d.downloaded) || "tamaño desconocido";
      let meta;
      if (d.status === "downloading") {
        meta = [pct != null ? `${pct.toFixed(1)} %` : null, size, d.speed ? `${human(Math.round(d.speed))}/s` : null]
          .filter(Boolean).join(" · ");
      } else if (d.status === "error") {
        meta = `Error: ${d.error ?? "desconocido"}`;
      } else {
        meta = [STATUS_LABEL[d.status], d.status === "completed" ? human(d.size) : size].filter(Boolean).join(" · ");
      }
      const bar = el("div", { className: "bar" }, el("div"));
      if (d.status === "downloading" && pct == null) bar.classList.add("indeterminate");
      else bar.firstChild.style.width = `${d.status === "completed" ? 100 : pct ?? 0}%`;
      const li = el(
        "li",
        { className: d.status === "completed" ? "done" : d.status === "error" ? "failed" : "" },
        el("div", { className: "line" },
          el("span", { className: "name", textContent: d.filename || shortName(d.url), title: d.url })),
        el("div", { className: "meta", textContent: meta, title: meta }),
        bar,
      );
      return li;
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

const KIND_LABEL = { hls: "HLS · vídeo completo", dash: "DASH · vídeo completo" };

function isVideo(m) {
  return m.kind !== "file" || (m.mime || "").startsWith("video/") || VIDEO_EXT.test(m.url);
}

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
      };
      const get = el("button", { textContent: "Descargar", className: "primary" });
      get.onclick = () => download({ ...base, video: stream }, get);
      const line = el("div", { className: "line" },
        el("span", {
          className: "name",
          // A playlist file name (index.m3u8) says nothing; the page title does.
          textContent: stream && state?.pageTitle ? state.pageTitle : shortName(m.url),
          title: m.url,
        }),
        el("span", { className: "meta", textContent: KIND_LABEL[m.kind] ?? (human(m.size) || m.mime || "vídeo") }),
        get,
      );
      if (isVideo(m)) {
        const mp3 = el("button", { textContent: "MP3", title: "Descargar solo el audio en MP3" });
        mp3.onclick = () => download({ ...base, video: true, audioKbps: mp3Kbps() }, mp3);
        line.append(mp3);
      }
      return el("li", {}, line);
    }),
  );
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
    const info = await send({ type: "formats", url: tab.url });
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
  if (state.daemonUp) {
    st.textContent = "conectado";
    st.className = "ok";
  } else {
    st.textContent = "sin conexión";
    st.className = "err";
    say(state.hostError || "raudo no disponible", "err");
  }
  $("permission").hidden = !state.needsPermission;
  const web = /^https?:/i.test(tab?.url || "");
  $("page").hidden = !web;
  $("page-hint").hidden = !web || state.videoSite;
  for (const b of ["page-best", "page-mp3", "page-formats", "formats-go"]) $(b).disabled = !state.daemonUp;
  renderMedia(state.media || []);
  if (state.daemonUp) refreshDownloads();
}

$("page-best").onclick = (e) => download({ url: tab.url, video: true }, e.target);
$("page-mp3").onclick = (e) => download({ url: tab.url, video: true, audioKbps: mp3Kbps() }, e.target);
$("page-formats").onclick = loadFormats;
$("formats-go").onclick = (e) => download({ url: tab.url, video: true, format: $("formats").value }, e.target);
$("grant").onclick = async () => {
  if (await api.permissions.request({ origins: ["<all_urls>"] })) refresh();
};

// Remember the chosen MP3 quality between popups.
api.storage.local.get("mp3Kbps").then((v) => {
  if (v.mp3Kbps != null) $("mp3-quality").value = String(v.mp3Kbps);
});
$("mp3-quality").onchange = () => api.storage.local.set({ mp3Kbps: mp3Kbps() });

refresh();
