const api = globalThis.browser ?? globalThis.chrome;
const $ = (id) => document.getElementById(id);

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
  const u = ["B", "KiB", "MiB", "GiB"];
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

async function download(d, button) {
  if (button) button.disabled = true;
  try {
    const r = await send({ type: "download", download: d });
    say(`Añadido a raudo (#${r.downloadId}).`, "ok");
  } catch (e) {
    say(e.message, "err");
  } finally {
    if (button) button.disabled = false;
  }
}

const KIND_LABEL = { hls: "HLS · vídeo completo", dash: "DASH · vídeo completo" };

function renderMedia(list) {
  const ul = $("media");
  ul.replaceChildren();
  $("media-empty").hidden = list.length > 0;
  $("media-empty").textContent = state?.videoSite
    ? "En esta web el vídeo llega en fragmentos: usa «Descargar» de arriba."
    : "Nada detectado todavía. Si no aparece, dale a reproducir y vuelve a abrir.";
  for (const m of list) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.className = "name";
    // A playlist file name (index.m3u8, master.m3u8) says nothing; the page title does.
    name.textContent = m.kind !== "file" && state?.pageTitle ? state.pageTitle : shortName(m.url);
    name.title = m.url;
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = KIND_LABEL[m.kind] ?? (human(m.size) || m.mime || "vídeo");
    const btn = document.createElement("button");
    btn.textContent = "Descargar";
    const stream = m.kind !== "file";
    // Stream playlists have no useful name of their own (index.m3u8): name the file after the page.
    btn.onclick = () =>
      download({
        url: m.url,
        // What the player really sent; the tab URL is only a guess for older entries.
        referer: "referer" in m ? m.referer : tab?.url,
        video: stream,
        filename: stream ? state?.pageTitle : null,
      }, btn);
    li.append(name, meta, btn);
    ul.append(li);
  }
}

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
    sel.replaceChildren();
    // Video-only streams get the best audio merged in, as yt-dlp would do by default.
    const best = document.createElement("option");
    best.value = "bv*+ba/b";
    best.textContent = "Mejor calidad disponible";
    sel.append(best);
    for (const f of [...info.formats].reverse()) {
      const o = document.createElement("option");
      o.value = f.vcodec && !f.acodec ? `${f.id}+ba/${f.id}` : f.id;
      o.textContent = formatLabel(f);
      sel.append(o);
    }
    $("formats-row").hidden = false;
    say(info.title);
  } catch (e) {
    say(e.message, "err");
  } finally {
    $("page-formats").disabled = false;
  }
}

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
  for (const b of ["page-best", "page-formats", "formats-go"]) $(b).disabled = !state.daemonUp;
  renderMedia(state.media || []);
}

$("page-best").onclick = (e) => download({ url: tab.url, video: true }, e.target);
$("page-formats").onclick = loadFormats;
$("formats-go").onclick = (e) => download({ url: tab.url, video: true, format: $("formats").value }, e.target);
$("grant").onclick = async () => {
  if (await api.permissions.request({ origins: ["<all_urls>"] })) refresh();
};

refresh();
