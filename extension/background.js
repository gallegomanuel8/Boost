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

/**
 * Pieces of a stream rather than whole videos: DASH/HLS fragments (.m4s, .ts,
 * seg-12, frag5, init.mp4) and byte-range requests in the URL. Many players serve
 * fMP4 fragments as video/mp4, which would otherwise list each one as a video.
 */
function isFragment(url, path) {
  if (/\.(m4s|ts|aac|cmfv|cmfa)$/.test(path)) return true;
  // Not "part": real titles use it ("movie-part-2.mp4").
  if (/(^|[\/_.-])(seg|segment|frag|fragment|chunk)[-_]?\d+/.test(path)) return true;
  if (/(^|\/)init[^\/]*\.(mp4|m4s)$/.test(path)) return true;
  return /[?&](range|bytes|byterange)=\d/i.test(url);
}

function classify(d) {
  const mime = (header(d.responseHeaders, "content-type") || "").split(";")[0].trim().toLowerCase();
  let path = "";
  try { path = new URL(d.url).pathname.toLowerCase(); } catch { return null; }
  if (isFragment(d.url, path)) return null;
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

/**
 * First URL of each redirected request: players often point <video> at an address
 * like streamtape's /get_video?... that redirects to the real file. Both name the
 * same video.
 */
const redirectedFrom = new Map();

api.webRequest.onBeforeRedirect.addListener(
  (d) => {
    if (d.tabId < 0) return;
    if (!redirectedFrom.has(d.requestId)) redirectedFrom.set(d.requestId, d.url.split("#")[0]);
    if (redirectedFrom.size > 500) redirectedFrom.delete(redirectedFrom.keys().next().value);
  },
  { urls: ["<all_urls>"], types: ["media", "xmlhttprequest", "object", "other"] },
);

function saveTab(tabId, list) {
  media.set(tabId, list);
  api.storage.session?.set({ [mediaKey(tabId)]: list });
  updateBadge(tabId);
}

/** Asks the daemon what a playlist contains: quality, duration, its variants. */
async function inspectStream(tabId, item) {
  try {
    const cookies = await cookieHeader(item.url);
    const r = await call({
      type: "inspect", target: "hls", url: item.url, referer: item.referer, cookies, userAgent: navigator.userAgent,
    });
    item.info = r.info;
  } catch {
    item.info = null; // unreadable here; the download can still re-resolve it
  }
  const list = media.get(tabId);
  if (list?.includes(item)) saveTab(tabId, list);
}

api.webRequest.onHeadersReceived.addListener(
  (d) => {
    if (d.tabId < 0) return;
    const sent = sentReferer.get(d.requestId);
    sentReferer.delete(d.requestId);
    const origin = redirectedFrom.get(d.requestId);
    redirectedFrom.delete(d.requestId);
    const found = classify(d);
    if (!found) return;
    const list = media.get(d.tabId) ?? [];
    const bare = d.url.split("#")[0];
    if (list.some((m) => m.url === bare)) return;
    // null = the browser sent no Referer, and the server accepted that.
    const referer = sent !== undefined ? sent : (d.documentUrl ?? d.initiator ?? null);
    const item = { url: bare, ...found, referer, aliases: origin && origin !== bare ? [origin] : [] };
    // A <video> found by the page scan that turns out to redirect here is the same video.
    saveTab(d.tabId, list.filter((m) => !(m.fromPage && item.aliases.includes(m.url))).concat(item));
    if (item.kind === "hls") inspectStream(d.tabId, item);
  },
  { urls: ["<all_urls>"] },
  ["responseHeaders"],
);

const bareUrl = (u) => u.split(/[?#]/)[0];
const baseName = (u) => decodeURIComponent(bareUrl(u).split("/").pop() || "");

/**
 * What the badge counts and the popup lists:
 * - variant playlists of a master that is listed are hidden (the master downloads the best one);
 * - the same file from several servers or with a new token counts once (name + size, or path);
 * - page-scan entries that are another name of a detected request are dropped.
 */
function visibleMedia(list) {
  const variants = new Set();
  for (const m of list) for (const v of m.info?.variants ?? []) variants.add(bareUrl(v));
  const aliases = new Set(list.flatMap((m) => m.aliases ?? []));
  const seen = new Set();
  return list.filter((m) => {
    if (m.kind === "hls" && variants.has(bareUrl(m.url)) && !m.info?.master) return false;
    if (m.fromPage && aliases.has(m.url)) return false;
    const key = m.size ? `${baseName(m.url)}|${m.size}` : bareUrl(m.url);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}

/** Media referenced by <video>/<audio> elements, so it shows up before playback starts. */
const within = (ms, promise, fallback) =>
  Promise.race([promise, new Promise((resolve) => setTimeout(() => resolve(fallback), ms))]);

/**
 * One injection per frame, each with its own deadline: `allFrames: true` waits for
 * every frame, and ad iframes blocked by Brave Shields (or still loading) may never
 * answer, which left the popup stuck on "conectando…".
 */
async function scanPage(tabId) {
  if (!api.scripting || !api.webNavigation) return [];
  const frames = await within(1500, api.webNavigation.getAllFrames({ tabId }).catch(() => null), null);
  if (!frames) return [];
  const perFrame = frames
    .filter((f) => /^https?:/i.test(f.url))
    .slice(0, 20)
    .map((f) => within(1500, scanFrame(tabId, f.frameId), []));
  return (await Promise.all(perFrame)).flat();
}

async function scanFrame(tabId, frameId) {
  try {
    const results = await api.scripting.executeScript({
      target: { tabId, frameIds: [frameId] },
      func: () => {
        const urls = new Set();
        for (const media of document.querySelectorAll("video, audio")) {
          if (media.localName === "video") {
            // Hover previews of related videos: small, or muted looping clips, or named so.
            const r = media.getBoundingClientRect();
            const small = r.width < 320 || r.height < 180;
            const preview = media.muted && media.loop;
            if (small || preview) continue;
          }
          const sources = [media.currentSrc, media.getAttribute("src")];
          for (const s of media.querySelectorAll("source")) sources.push(s.getAttribute("src"));
          for (const u of sources) {
            if (!u) continue;
            const abs = new URL(u, location.href).href;
            // blob: = MediaSource player, not a file.
            if (/^https?:/i.test(abs) && !/thumb|preview|teaser|sprite/i.test(abs)) urls.add(abs);
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
  const n = visibleMedia(media.get(tabId) ?? []).length;
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
      let list = [];
      if (msg.tabId != null) {
        list = [...(await mediaFor(msg.tabId))];
        const known = new Set(list.flatMap((m) => [m.url, ...(m.aliases ?? [])]));
        const found = (await scanPage(msg.tabId))
          .filter(({ url }) => !known.has(url))
          .map(({ url, referer }) => ({ url, kind: kindOfUrl(url), mime: "", size: null, referer, fromPage: true }));
        if (found.length) {
          // Kept with the network finds, so the badge counts them too.
          list = list.concat(found);
          saveTab(msg.tabId, list);
          for (const item of found) if (item.kind === "hls") inspectStream(msg.tabId, item);
        }
      }
      return {
        daemonUp, hostError, needsPermission,
        media: visibleMedia(list),
        pageUrl: tab?.url ?? null,
        pageTitle: tab?.title ?? null,
        videoSite: tab?.url ? isVideoSite(tab.url) : false,
      };
    }
    if (msg.type === "download") {
      return { downloadId: await sendDownload(msg.download) };
    }
    if (msg.type === "media") {
      // Light refresh while stream inspections finish; no page scan.
      return visibleMedia(await mediaFor(msg.tabId));
    }
    if (msg.type === "list") {
      return (await call({ type: "list" })).downloads;
    }
    if (msg.type === "inspect") {
      const cookies = await cookieHeader(msg.url);
      return (await call({
        type: "inspect", target: msg.target, url: msg.url, referer: msg.referer ?? null,
        cookies, userAgent: navigator.userAgent,
      })).info;
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
