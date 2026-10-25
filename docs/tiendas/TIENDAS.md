# Publicar la extensión en las tiendas

> **Decisión actual:** la extensión se publica solo en Firefox (AMO). Para Chrome, Brave y
> Edge se instala a mano desde la carpeta que deja el instalador (Boost guía el proceso en
> *Preferencias → Extensiones web*). Lo demás de esta guía queda para si se publica más
> adelante en esas tiendas.

Guía para subir la extensión de Boost a:

| Tienda | Navegadores | Coste | Revisión habitual |
|---|---|---|---|
| [Chrome Web Store](#chrome-web-store) | Chrome, Brave, Vivaldi, Opera | 5 $ una vez | de días a 3 semanas |
| [Microsoft Edge Add-ons](#microsoft-edge-add-ons) | Edge | gratis | hasta 7 días hábiles |
| [Firefox Add-ons (AMO)](#firefox-add-ons-amo) | Firefox | gratis | de horas a días |

Cada navegador instala solo desde su propia tienda. Chrome y Brave no pueden instalar
extensiones de la tienda de Edge (Edge sí puede instalar de la Chrome Web Store, pero no al
revés), así que para Chrome y Brave hace falta la Chrome Web Store.

## Antes de enviar

### 1. YouTube y la Chrome Web Store

Las [normas de la Chrome Web Store](https://developer.chrome.com/docs/webstore/program-policies/)
no permiten extensiones que faciliten descargar vídeos de YouTube, y la rechazan aunque solo
lo mencione. Por eso ni las capturas ni los textos de esta guía hablan de YouTube.

La extensión actual ofrece «Vídeo de esta página» también en YouTube (a través de yt-dlp),
así que **para la Chrome Web Store hay que generar una versión que lo desactive** en
youtube.com. La aplicación Boost seguiría pudiendo hacerlo con «Añadir URL». Microsoft
Edge Add-ons puede aplicar el mismo criterio; para Firefox (AMO) no hace falta.

### 2. Los ID de la extensión

Cada tienda asigna su propio ID a la extensión, distinto del que se usa al cargarla
descomprimida. La aplicación solo acepta conexiones de las extensiones que conoce, así que:

1. Sube la extensión a cada tienda (como borrador si quieres) y anota el ID que te asigna.
   - Chrome Web Store: aparece en el panel de desarrollador nada más subir el paquete.
   - Edge Add-ons: aparece en *Partner Center → la extensión → Overview* tras crearla.
2. Esos ID se añaden a Boost (`raudo-nmhost` y la pestaña *Extensiones web*, que además
   llevará a la tienda) y se publica una versión nueva de la aplicación **antes** de que se
   aprueben las extensiones, para que funcionen desde el primer día.

El ID de Firefox no cambia: es `boost-downloader@boost.app`.

### 3. Paquetes

```bash
scripts/package-stores.sh
```

Genera en `dist/`:

| Fichero | Para |
|---|---|
| `boost-extension-<versión>-chrome-web-store.zip` | Chrome Web Store |
| `boost-extension-<versión>-edge-addons.zip` | Edge Add-ons |
| `boost-extension-<versión>-amo-source.zip` | Firefox Add-ons |

Los zip de Chrome y Edge no llevan el campo `key` (la tienda pone el ID) ni los ajustes
exclusivos de Firefox.

Cada versión subida a una tienda debe tener un número nuevo en `extension/manifest.json`.
En AMO, la 0.5.0 y la 0.5.1 ya están usadas por la versión no listada: la primera versión
pública allí tendrá que ser al menos la 0.5.2.

### 4. Imágenes (en esta carpeta)

| Fichero | Tamaño | Chrome | Edge | Firefox |
|---|---|---|---|---|
| `icono-128x128.png` | 128×128 | Icono de la tienda | — | — |
| `logo-300x300.png` | 300×300 | — | Logo de la extensión | — |
| `captura-1-extension-1280x800.png` | 1280×800 | Captura | Captura | Captura |
| `captura-2-aplicacion-1280x800.png` | 1280×800 | Captura | Captura | Captura |
| `promocional-440x280.png` | 440×280 | Imagen promocional pequeña (obligatoria) | Imagen promocional pequeña (opcional) | — |

### 5. Enlaces que piden los formularios

- **Política de privacidad:** <https://github.com/gallegomanuel8/Boost/blob/main/docs/PRIVACIDAD.md>
- **Web del producto:** <https://github.com/gallegomanuel8/Boost>
- **Soporte:** <https://github.com/gallegomanuel8/Boost/issues>

---

## Textos para copiar

Las tiendas permiten ficha en varios idiomas. Pon el español como idioma principal y añade
el inglés para llegar a más gente.

### Nombre

```
Boost — Gestor de descargas
```
```
Boost — Download Manager
```

### Descripción corta (máx. 132 caracteres)

```
Detecta vídeos y ficheros de cualquier web y los descarga con la app Boost: varias conexiones, HLS y audio MP3.
```
```
Detects videos and files on any website and downloads them with the Boost app: multiple connections, HLS and MP3 audio.
```

### Descripción larga (español)

```
Boost conecta tu navegador con la aplicación de escritorio Boost, un gestor de descargas libre y gratuito para Linux y Windows.

Qué hace la extensión:
• Detecta los vídeos y audios que carga la página, también las listas de reproducción HLS (.m3u8), y te muestra su formato, resolución y duración.
• Descarga el vídeo de la página con un clic, o solo su audio convertido a MP3 con la calidad que elijas.
• Envía a Boost las descargas que inicias en el navegador, para que se bajen más rápido con varias conexiones a la vez.
• Muestra el progreso de las descargas en curso y permite cancelarlas.
• Añade al menú contextual «Descargar enlace con Boost» y «Descargar este medio con Boost».
• Si Boost está cerrado, lo abre en segundo plano con un botón.

Qué hace la aplicación Boost:
• Descargas segmentadas con segmentación dinámica, pausar y reanudar incluso tras reiniciar.
• Cola de descargas, límite de velocidad y descargas simultáneas configurables.
• Nunca sobrescribe ficheros y avisa si un fichero descargado se ha borrado.

Requisito: la extensión necesita la aplicación Boost instalada en el mismo equipo. Descárgala gratis en https://github.com/gallegomanuel8/Boost/releases

Privacidad: la extensión no recoge ni envía datos a ningún servidor. Solo se comunica con la aplicación Boost de tu equipo.

Boost es software libre (GPL-3.0). Código fuente: https://github.com/gallegomanuel8/Boost
```

### Descripción larga (inglés)

```
Boost connects your browser with the Boost desktop application, a free and open-source download manager for Linux and Windows.

What the extension does:
• Detects the videos and audio the page loads, including HLS playlists (.m3u8), and shows their format, resolution and duration.
• Downloads the page's video in one click, or just its audio converted to MP3 at the quality you choose.
• Sends the downloads you start in the browser to Boost, so they finish faster using several connections at once.
• Shows the progress of running downloads and lets you cancel them.
• Adds "Download link with Boost" and "Download this media with Boost" to the context menu.
• If Boost is closed, opens it in the background with one button.

What the Boost app does:
• Segmented downloads with dynamic splitting; pause and resume, even after a restart.
• Download queue, speed limit and configurable parallel downloads.
• Never overwrites files, and warns you when a downloaded file has been deleted.

Requirement: the extension needs the Boost application installed on the same computer. Get it for free at https://github.com/gallegomanuel8/Boost/releases

Privacy: the extension collects no data and sends nothing to any server. It only talks to the Boost application on your computer.

Boost is free software (GPL-3.0). Source code: https://github.com/gallegomanuel8/Boost
```

La interfaz de la extensión solo está en español: en la ficha en inglés puedes añadir
«The interface is in Spanish» hasta que se traduzca.

### Propósito único (Chrome: *Single purpose*)

```
Send the files and media found in web pages to the Boost desktop download manager, and show the progress of its downloads.
```

### Justificación de cada permiso

Chrome y Edge piden explicar cada permiso. Los formularios se revisan en inglés:

| Permiso | Justificación |
|---|---|
| `nativeMessaging` | Communicates with the Boost desktop application installed on the same computer, which performs the downloads. This is the extension's core function. |
| `downloads` | Detects downloads the user starts in the browser so they can be handed to Boost instead, and falls back to the browser's own download if Boost is not available. |
| `cookies` | Reads the cookies of the file being downloaded, only to pass them to the local Boost app, so that sites requiring login serve the same file as in the browser. They are never sent anywhere else. |
| `webRequest` | Observes the media requests a page makes (video, audio, HLS playlists) to list them in the popup, and the referer the player used, which some servers require to allow the download. |
| `contextMenus` | Adds "Download link with Boost" and "Download this media with Boost" to the right-click menu. |
| `storage` | Remembers the chosen MP3 quality and keeps the media detected in each tab while it is open. |
| `tabs` | Reads the active tab's address and title to offer downloading its video and to give the file a readable name; opens a tab with the boost:// link to launch the app when it is closed. |
| `scripting` | Looks for video and audio elements in the page when the user opens the popup, to find media that was not caught while loading. |
| `webNavigation` | Lists the frames of the page so that media inside embedded players (iframes) is found too. |
| Acceso a todos los sitios (`<all_urls>`) | Videos and downloads can be on any website, and the user decides where to use the extension. Page content is only read when the user opens the popup or uses the context menu. |

### Código remoto

```
No, I am not using remote code. All JavaScript is included in the extension package.
```

### Uso de datos (cuestionario de Chrome y Edge)

No marques ningún tipo de dato. Marca las tres certificaciones: no se venden datos a
terceros, no se usan para fines ajenos al propósito único y no se usan para evaluar
solvencia crediticia ni conceder préstamos.

### Instrucciones para los revisores

```
This extension is the browser companion of Boost, a free open-source desktop download manager (https://github.com/gallegomanuel8/Boost).

To test it:
1. Install the Boost app for your OS from https://github.com/gallegomanuel8/Boost/releases/latest (Linux: run ./install.sh; Windows: run the installer). It registers the native messaging host for this extension.
2. Open Boost. The extension's popup should show "conectado" (connected).
3. Open https://hlsjs.video-dev.org/demo/ (a public HLS test player), let the video play and open the popup: the stream appears under "Medios detectados" (detected media) as "HLS → MP4". Click "MP4" to send it to Boost, which downloads it.
4. Right-click any link and choose "Descargar enlace con Boost".

Without the app, the popup only shows a button to launch it ("Lanzar aplicación"). The interface is in Spanish.
```

---

## Chrome Web Store

1. Entra en <https://chrome.google.com/webstore/devconsole> con tu cuenta de Google, acepta
   el acuerdo de desarrollador y paga la cuota de registro (5 $, una sola vez).
2. Verifica el correo de contacto en *Cuenta*.
3. *Nuevo elemento* → sube `boost-extension-<versión>-chrome-web-store.zip`. Anota el ID.
4. **Ficha de la tienda:** descripción larga, categoría *Herramientas* (*Tools*), idioma
   *Español*, icono `icono-128x128.png`, las dos capturas, imagen promocional pequeña
   `promocional-440x280.png`, web y URL de soporte.
5. **Prácticas de privacidad:** propósito único, justificación de permisos, código remoto,
   uso de datos y URL de la política de privacidad (textos de arriba).
6. **Distribución:** *Público*, todos los países.
7. *Enviar para revisión*. Puedes elegir publicar automáticamente al aprobarse o
   manualmente; elige **manualmente** hasta tener lista la versión de Boost con el nuevo ID.

## Microsoft Edge Add-ons

1. Entra en <https://partner.microsoft.com/dashboard/microsoftedge> con una cuenta de
   Microsoft y regístrate en el programa de Microsoft Edge (gratis, cuenta *Individual*).
2. *Create new extension* → sube `boost-extension-<versión>-edge-addons.zip`. Anota el ID
   en *Overview*.
3. **Availability:** *Public*, todos los mercados.
4. **Properties:** categoría *Productivity*, URL de la política de privacidad, web y
   soporte. Marca que la extensión no tiene contenido para adultos.
5. **Store listings:** añade *Spanish* (y *English*): descripción, descripción corta, logo
   `logo-300x300.png`, capturas e imagen promocional pequeña.
6. **Submit**, añadiendo las instrucciones para los revisores en *Notes for certification*.

## Firefox Add-ons (AMO)

La extensión ya existe en AMO como «no listada» (firmada, sin ficha pública). Para que
aparezca en el buscador de complementos:

1. Sube la versión en `extension/manifest.json` a 0.5.2 o superior.
2. Entra en <https://addons.mozilla.org/developers/addons> → *Boost* → *Upload New Version*
   → elige **On this site** (listada) y sube `boost-extension-<versión>-amo-source.zip`.
   El código no está minificado, así que AMO no pide código fuente aparte.
3. Rellena la ficha (*Edit Product Page*): nombre, resumen (máx. 250 caracteres; vale la
   descripción corta), descripción larga, categoría *Download Management*, capturas,
   política de privacidad, web y soporte. AMO permite mencionar YouTube.
4. En *Notes to Reviewer* pega las instrucciones para los revisores.

En AMO no hace falta cambiar nada en Boost: el ID sigue siendo el mismo.

---

## Después de publicar

- Añadir a Boost los enlaces a cada tienda en *Preferencias → Extensiones web*, en lugar de
  la carga descomprimida.
- Actualizar el README: instalar la extensión desde la tienda de cada navegador.
- Cada versión nueva de la extensión se sube a las tres tiendas con un número de versión
  mayor. Las tiendas actualizan la extensión a los usuarios automáticamente.
