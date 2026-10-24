// raudo browser integration: forwards downloads and detected media to raudo-nmhost.
// Capture rules come from the host on connect, so all browsers behave alike.

const api = globalThis.browser ?? globalThis.chrome;
const IS_FIREFOX = typeof globalThis.browser?.runtime?.getBrowserInfo === "function";
const HOST = "org.raudo.nmhost";
const CALL_TIMEOUT_MS = 90_000; // `formats` runs yt-dlp, which can take a while

let port = null;
let nextId = 1;
const pending = new Map();
let config = null;
let daemonUp = false;
let hostError = null;
/** URLs we re-handed to the browser after a failed send: never capture them again. */
const passThrough = new Set();

// ---------------------------------------------------------------- native host

function connect() {
  port = api.runtime.connectNative(HOST);
  port.onMessage.addListener((m) => {
    const p = pending.get(m.id);
    if (!p) return;
    pending.delete(m.id);
    clearTimeout(p.timer);
    m.ok ? p.resolve(m) : p.reject(new Error(m.error || "error desconocido"));
  });
  port.onDisconnect.addListener((p) => {
    hostError = api.runtime.lastError?.message || p?.error?.message || "conexión con raudo-nmhost cerrada";
    port = null;
    daemonUp = false;
    for (const req of pending.values()) {
      clearTimeout(req.timer);
      req.reject(new Error(hostError));
    }
    pending.clear();
  });
}

function call(msg) {
  if (!port) connect();
  const id = nextId++;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error("raudo-nmhost no responde"));
    }, CALL_TIMEOUT_MS);
    pending.set(id, { resolve, reject, timer });
    port.postMessage({ ...msg, id });
  });
}

async function hello() {
  try {
    const r = await call({ type: "hello", browser: navigator.userAgent });
    config = r.config;
    daemonUp = r.daemon;
    hostError = r.daemon ? null : "raudo-daemon no está en ejecución";
  } catch (e) {
    hostError = e.message;
  }
  return { daemonUp, hostError };
}

async function cookieHeader(url) {
  try {
    const query = IS_FIREFOX ? { url, firstPartyDomain: null } : { url };
    const list = await api.cookies.getAll(query);
    return list.length ? list.map((c) => `${c.name}=${c.value}`).join("; ") : null;
  } catch {
    return null;
  }
}

async function sendDownload({
  url, referer = null, filename = null, title = null, video = false, format = null, audioKbps = null, page = null,
}) {
  const cookies = await cookieHeader(url);
  const r = await call({
    type: "download", url, referer, cookies, userAgent: navigator.userAgent,
    filename, title, video, format, audioKbps, page,
  });
  flashBadge();
  return r.downloadId;
}

// ---------------------------------------------------------- download capture

function extOf(name) {
  const m = /\.([a-z0-9]{1,9})$/i.exec(name || "");
  return m ? m[1].toLowerCase() : "";
}

/** Extension implied by the MIME type, for URLs like download.php?id=1. */
const MIME_EXT = {
  "application/zip": "zip", "application/x-zip-compressed": "zip", "application/x-7z-compressed": "7z",
  "application/vnd.rar": "rar", "application/x-rar-compressed": "rar", "application/gzip": "gz",
  "application/x-iso9660-image": "iso", "application/pdf": "pdf", "application/vnd.android.package-archive": "apk",
  "application/x-msdownload": "exe", "application/x-rpm": "rpm", "application/vnd.debian.binary-package": "deb",
  "video/mp4": "mp4", "video/webm": "webm", "video/x-matroska": "mkv", "audio/mpeg": "mp3", "audio/flac": "flac",
};

function shouldCapture(item) {
  const c = config?.capture;
  if (!c?.enabled || !daemonUp) return false;
  const url = item.finalUrl || item.url;
  if (!/^https?:/i.test(url) || passThrough.has(url)) return false;
  let path = "";
  try { path = new URL(url).pathname; } catch { /* keep empty */ }
  // Chromium often reports an empty filename at creation; the MIME type is known.
  const ext = extOf(item.filename) || extOf(path) || MIME_EXT[(item.mime || "").toLowerCase()] || "";
  if (!c.exts.includes(ext)) return false;
  // 0 or -1 means "not known yet" (Chromium reports 0 at creation): judge by type alone.
  const size = item.totalBytes > 0 ? item.totalBytes : item.fileSize > 0 ? item.fileSize : -1;
  return size < 0 || size >= c.minSize;
}

api.downloads.onCreated.addListener(async (item) => {
  if (!config) await hello();
  if (!shouldCapture(item)) return;
  const url = item.finalUrl || item.url;
  try {
    await api.downloads.cancel(item.id);
    await api.downloads.erase({ id: item.id });
  } catch {
    return; // already finished or not cancellable: leave it to the browser
  }
  try {
    await sendDownload({ url, referer: item.referrer || null });
  } catch (e) {
    // Never lose a download: give it back to the browser.
    console.warn("raudo: send failed, falling back to the browser:", e.message);
    passThrough.add(url);
    api.downloads.download({ url });
  }
});

// ------------------------------------------------------------ media detection

/** tabId -> array of {url, kind, mime, size} */
const media = new Map();
const mediaKey = (tabId) => `media:${tabId}`;

function header(headers, name) {
  return headers?.find((h) => h.name.toLowerCase() === name)?.value ?? null;
}

function classify(d) {
  const mime = (header(d.responseHeaders, "content-type") || "").split(";")[0].trim().toLowerCase();
  let path = "";
  try { path = new URL(d.url).pathname.toLowerCase(); } catch { return null; }
  if (path.endsWith(".m3u8") || mime.includes("mpegurl")) return { kind: "hls", mime, size: null };
  if (path.endsWith(".mpd") || mime === "application/dash+xml") return { kind: "dash", mime, size: null };
  if (!(mime.startsWith("video/") || mime.startsWith("audio/")) || mime.endsWith("/mp2t")) return null;

  // Only whole files: 200, or 206 starting at byte 0 (how <video> opens a file).
  let size = Number(header(d.responseHeaders, "content-length")) || null;
  if (d.statusCode === 206) {
    const m = /^bytes 0-\d+\/(\d+)/.exec(header(d.responseHeaders, "content-range") || "");
    if (!m) return null;
    size = Number(m[1]);
  } else if (d.statusCode !== 200) {
    return null;
  }
  if (size !== null && size < (config?.mediaMinSize ?? 512 * 1024)) return null;
  return { kind: "file", mime, size };
}

/**
 * Referer actually sent with each request. Players usually live in an iframe of
 * another site, and CDNs check that referer: the tab URL gets 403.
 */
const sentReferer = new Map();

api.webRequest.onBeforeSendHeaders.addListener(
  (d) => {
    if (d.tabId < 0) return;
    sentReferer.set(d.requestId, header(d.requestHeaders, "referer"));
    if (sentReferer.size > 500) sentReferer.delete(sentReferer.keys().next().value);
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "object", "other"] },
  // Chrome hides Referer from listeners unless asked for extraHeaders; Firefox rejects the value.
  IS_FIREFOX ? ["requestHeaders"] : ["requestHeaders", "extraHeaders"],
);

api.webRequest.onHeadersReceived.addListener(
  (d) => {
    if (d.tabId < 0) return;
    const sent = sentReferer.get(d.requestId);
    sentReferer.delete(d.requestId);
    const found = classify(d);
    if (!found) return;
    const list = media.get(d.tabId) ?? [];
    const bare = d.url.split("#")[0];
    if (list.some((m) => m.url === bare)) return;
    // null = the browser sent no Referer, and the server accepted that.
    const referer = sent !== undefined ? sent : (d.documentUrl ?? d.initiator ?? null);
    list.push({ url: bare, ...found, referer });
    media.set(d.tabId, list);
    api.storage.session?.set({ [mediaKey(d.tabId)]: list });
    updateBadge(d.tabId);
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"],
);

/** Media referenced by <video>/<audio> elements, so it shows up before playback starts. */
async function scanPage(tabId) {
  if (!api.scripting) return [];
  try {
    const results = await api.scripting.executeScript({
      target: { tabId, allFrames: true },
      func: () => {
        const urls = new Set();
        for (const el of document.querySelectorAll("video, audio, video source, audio source")) {
          for (const u of [el.currentSrc, el.getAttribute("src")]) {
            if (!u) continue;
            const abs = new URL(u, location.href).href;
            if (/^https?:/i.test(abs)) urls.add(abs); // blob: = MediaSource player, not a file
          }
        }
        // The frame holding the player is the referer its media requests carry.
        return [...urls].map((url) => ({ url, referer: location.href }));
      },
    });
    return results.flatMap((r) => r.result || []);
  } catch {
    return []; // restricted page (about:, store) or frame we cannot reach
  }
}

function kindOfUrl(url) {
  const path = url.split(/[?#]/)[0].toLowerCase();
  return path.endsWith(".m3u8") ? "hls" : path.endsWith(".mpd") ? "dash" : "file";
}

async function mediaFor(tabId) {
  if (!media.has(tabId) && api.storage.session) {
    // The service worker may have been restarted since the requests were seen.
    const stored = await api.storage.session.get(mediaKey(tabId));
    if (stored[mediaKey(tabId)]) media.set(tabId, stored[mediaKey(tabId)]);
  }
  return media.get(tabId) ?? [];
}

function forgetTab(tabId) {
  media.delete(tabId);
  api.storage.session?.remove(mediaKey(tabId));
  updateBadge(tabId);
}

api.tabs.onUpdated.addListener((tabId, info) => {
  if (info.status === "loading" && info.url) forgetTab(tabId);
});
api.tabs.onRemoved.addListener((tabId) => forgetTab(tabId));

function updateBadge(tabId) {
  const n = media.get(tabId)?.length ?? 0;
  api.action.setBadgeBackgroundColor({ color: "#2f6fde", tabId }).catch(() => {});
  api.action.setBadgeText({ text: n ? String(n) : "", tabId }).catch(() => {});
}

function flashBadge() {
  api.action.setBadgeText({ text: "✓" }).catch(() => {});
  setTimeout(() => api.action.setBadgeText({ text: "" }).catch(() => {}), 1500);
}

// --------------------------------------------------------------- context menu

api.runtime.onInstalled.addListener(async () => {
  await api.contextMenus.removeAll();
  api.contextMenus.create({ id: "raudo-link", title: "Descargar enlace con raudo", contexts: ["link"] });
  api.contextMenus.create({ id: "raudo-media", title: "Descargar este medio con raudo", contexts: ["video", "audio"] });
  api.contextMenus.create({ id: "raudo-page", title: "Descargar vídeo de esta página con raudo", contexts: ["page"] });
});

api.contextMenus.onClicked.addListener(async (info, tab) => {
  if (!config) await hello();
  try {
    if (info.menuItemId === "raudo-link") {
      await sendDownload({ url: info.linkUrl, referer: info.pageUrl });
    } else if (info.menuItemId === "raudo-media" && /^https?:/i.test(info.srcUrl || "")) {
      await sendDownload({
        url: info.srcUrl,
        referer: info.frameUrl || info.pageUrl,
        title: tab?.title ?? null,
        page: info.pageUrl ?? null,
        video: /\.(m3u8|mpd)(\?|$)/i.test(info.srcUrl),
      });
    } else {
      // blob:/MediaSource players or the page itself: let yt-dlp resolve the page.
      await sendDownload({ url: info.pageUrl || tab?.url, video: true });
    }
  } catch (e) {
    console.warn("raudo:", e.message);
  }
});

// -------------------------------------------------------------- popup bridge

function isVideoSite(url) {
  try {
    const host = new URL(url).hostname;
    return (config?.videoSites ?? []).some((s) => host === s || host.endsWith(`.${s}`));
  } catch {
    return false;
  }
}

api.runtime.onMessage.addListener((msg, _sender, reply) => {
  (async () => {
    if (msg.type === "state") {
      const { daemonUp, hostError } = await hello();
      const tab = msg.tabId != null ? await api.tabs.get(msg.tabId).catch(() => null) : null;
      const needsPermission = IS_FIREFOX && !(await api.permissions.contains({ origins: ["<all_urls>"] }));
      const list = msg.tabId != null ? [...(await mediaFor(msg.tabId))] : [];
      if (msg.tabId != null) {
        for (const { url, referer } of await scanPage(msg.tabId)) {
          if (!list.some((m) => m.url === url)) {
            list.push({ url, kind: kindOfUrl(url), mime: "", size: null, referer, fromPage: true });
          }
        }
      }
      return {
        daemonUp, hostError, needsPermission,
        media: list,
        pageUrl: tab?.url ?? null,
        pageTitle: tab?.title ?? null,
        videoSite: tab?.url ? isVideoSite(tab.url) : false,
      };
    }
    if (msg.type === "download") {
      return { downloadId: await sendDownload(msg.download) };
    }
    if (msg.type === "list") {
      return (await call({ type: "list" })).downloads;
    }
    if (msg.type === "cancel") {
      return (await call({ type: "cancel", downloadId: msg.downloadId })).cancelled;
    }
    if (msg.type === "formats") {
      const cookies = await cookieHeader(msg.url);
      return (await call({ type: "formats", url: msg.url, cookies, userAgent: navigator.userAgent })).media;
    }
    throw new Error(`mensaje desconocido: ${msg.type}`);
  })().then(
    (r) => reply({ ok: true, result: r }),
    (e) => reply({ ok: false, error: e.message }),
  );
  return true; // async reply
});

hello();
