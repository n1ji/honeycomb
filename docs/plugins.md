# Writing a Honeycomb plugin

A plugin is a folder in Honeycomb's plugins folder (Settings → Plugins → Show plugins folder):

- macOS: `~/Library/Application Support/dev.n1ji.honeycomb/plugins/`
- Linux: `~/.local/share/dev.n1ji.honeycomb/plugins/`
- Windows: `%APPDATA%\dev.n1ji.honeycomb\plugins\`

With `HONEYCOMB_DATA_DIR` set, it's `$HONEYCOMB_DATA_DIR/plugins/`. Use that to try a plugin on a test library.

The folder name must match the plugin's `id`. A symlink to a folder elsewhere works too, which is handy while developing.

Plugins that come with Honeycomb live in [`plugins/`](../plugins) in the repo and are bundled into the app (Scrobbling is one). They're off until turned on, like any other. A plugin with the same id in your plugins folder is used instead of the bundled one.

## plugin.json

```json
{
  "id": "my-plugin",
  "name": "My Plugin",
  "version": "0.1.0",
  "description": "One line shown in Settings.",
  "main": "main.js",
  "apiVersion": 2,
  "hosts": ["api.example.com"],
  "loopbackPorts": [47391]
}
```

- `id`: lowercase letters, digits, `-` and `_`.
- `main`: the ES module Honeycomb loads, relative to the folder (default `main.js`). It can import other `.js` files from the folder with relative paths.
- `apiVersion`: the plugin API the plugin was written for. This Honeycomb has 2, and doesn't load plugins written for 1 (see [Coming from plugin API 1](#coming-from-plugin-api-1)).
- `hosts`: the hosts `ctx.http.request` may reach. HTTPS only, exact host names, and redirects aren't followed.
- `loopbackPorts`: ports on 127.0.0.1 that `ctx.waitForRedirect` may listen on, for OAuth sign-ins.

## The module

```js
export async function activate(ctx) {
  ctx.ui.addSidebarItem({
    label: "Hello",
    icon: "☺",
    onClick: async () => ctx.ui.toast(`${(await ctx.library.tracks()).length} tracks`),
  });
}

// Optional: called when the plugin is turned off.
export function deactivate() {}
```

`ctx` is a `PluginContext`; its full shape, with comments, is in [`src/plugin-api.ts`](../src/plugin-api.ts). In short:

- `library`: the tracks, `match()` to find library tracks for tracks from another service (same artist and title, similar length; version notes like "- 2011 Remaster" and "(feat. …)" are ignored), and `onChange()`.
- `playlists`: list, create, replace tracks, show.
- `player`: play or queue tracks, and `onNowPlaying()` / `onListened()` to follow what's playing. A song counts as listened to once half of it or 4 minutes have been heard, for songs over 30 seconds (Last.fm's rule), and fires once per play.
- `http.request(url, { method, headers, body })`: the plugin's only way to the internet, and only to the manifest's `hosts`. Pass an object as `body` to send a form.
- `settings` (plain strings; `get` answers straight away) and `secrets` (tokens, kept in the system keychain, not in `library.db`), both private to the plugin.
- `openUrl()` and `waitForRedirect(port, path)` for OAuth: open the provider's sign-in page with `redirect_uri=http://127.0.0.1:<port><path>`, and `waitForRedirect` resolves with the redirect's query parameters.
- `ui`: sidebar entries, a section under Settings → Plugins, dialogs, buttons, `h()` for building the interface, toasts, a text prompt, `copyText()`.

Almost everything returns a promise, because each call is a message to Honeycomb.

## The sandbox

Each plugin runs in a sandbox of its own: a hidden frame with no access to Honeycomb's window, to other plugins, or to the app's internals. All it has is `ctx`. In particular:

- No network of its own. `fetch`, `XMLHttpRequest`, images and WebSockets are blocked; use `ctx.http.request`.
- No storage of its own (`localStorage`, cookies, IndexedDB). Use `ctx.settings` and `ctx.secrets`. A plugin only ever sees its own.
- No files, and no way to run programs.
- Plain JavaScript still works: timers, `crypto.getRandomValues`, `crypto.subtle`, `TextEncoder`, `URLSearchParams`, `btoa`.

What a plugin can do is what `ctx` offers: read the library, create and change playlists, control playback, and talk to the hosts its manifest lists. Settings shows that list under each plugin as "Access". Plugins are off until turned on in Settings, and if one fails to load it shows the error there and the others still load.

## Building an interface

A plugin can't touch Honeycomb's page, so it doesn't get real DOM nodes. `ctx.ui.h()` and `ctx.ui.button()` return stand-ins (`PluginElement`) that Honeycomb draws for it, in the app's own look:

```js
ctx.ui.addSettingsSection((el) => {
  const name = ctx.ui.h("input", { class: "input", value: ctx.settings.get("name") ?? "", placeholder: "Name" });
  name.addEventListener("change", () => ctx.settings.set("name", name.value));
  el.append(
    ctx.ui.h("p", { class: "muted" }, "Who should be greeted?"),
    ctx.ui.h("div", { class: "row" }, name, ctx.ui.button("Greet", () => ctx.ui.toast(`Hello, ${name.value}`), true)),
  );
});
```

They work like a small part of the DOM:

- `append()`, `replaceChildren()`, `remove()`, `textContent`, `setAttribute()`, `className`.
- `value`, `checked`, `disabled`, `hidden`, `title`. A field's `value` and `checked` are kept up to date as the user types, so they can be read straight away.
- `addEventListener()` for `click`, `dblclick`, `input`, `change`, `keydown`, `keyup`, `focus` and `blur`. The event has `type`, `target` and, for keys, `key`. There's no `preventDefault()`.
- `isConnected` turns false once the section or dialog the element is in has left the screen (within a second for Settings). Use it to stop polling.
- `focus()` and `select()`.

Tags are limited to layout, text, table and form elements (`div`, `span`, `p`, `strong`, `code`, `h3`, `label`, `input`, `textarea`, `select`, `button`, `table`, `ul` and the like). There are no links, images or frames, and no HTML strings: text always goes in as text. Honeycomb's own classes work (`row`, `muted`, `error`, `input`, `btn`, `field`, `check`), and the `style` attribute takes layout and text properties (`display`, `gap`, `width`, `margin`, `font-size`, …).

`ctx.ui.modal(title)` returns `{ body, footer, close }`, with `body` and `footer` to fill the same way.

## Coming from plugin API 1

Plugins used to run inside Honeycomb's own window. To move one to API 2:

- Set `"apiVersion": 2` in `plugin.json`.
- `await` these, which now return promises: `library.tracks()`, `library.track()`, `playlists.list()`.
- Replace direct `fetch()` calls with `ctx.http.request()`, and `navigator.clipboard.writeText()` with `ctx.ui.copyText()`.
- Interface code written with `ctx.ui.h()` mostly works as it is. Anything that used `document`, `innerHTML`, `querySelector`, `classList` or `el.style` needs rewriting with the members listed above.
- `ctx.openUrl()` only opens `http://` and `https://` links.
- Only `.js`, `.mjs` and `.json` files in the plugin's folder can be loaded.

## Type checking

Plain JavaScript works with no build step. For editor checking, a `jsconfig.json` with `"checkJs": true` and a JSDoc import of `PluginContext` from a Honeycomb checkout is enough:

```js
/** @typedef {import("../honeycomb/src/plugin-api").PluginContext} PluginContext */
```
