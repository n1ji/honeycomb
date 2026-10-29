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
  "apiVersion": 1,
  "hosts": ["api.example.com"],
  "loopbackPorts": [47391]
}
```

- `id`: lowercase letters, digits, `-` and `_`.
- `main`: the ES module Honeycomb imports, relative to the folder (default `main.js`). It can import other files from the folder with relative paths.
- `hosts`: the hosts `ctx.http.request` may reach. HTTPS only, exact host names, and redirects aren't followed.
- `loopbackPorts`: ports on 127.0.0.1 that `ctx.waitForRedirect` may listen on, for OAuth sign-ins.

## The module

```js
export function activate(ctx) {
  ctx.ui.addSidebarItem({ label: "Hello", icon: "☺", onClick: () => ctx.ui.toast(`${ctx.library.tracks().length} tracks`) });
}

// Optional: called when the plugin is turned off.
export function deactivate() {}
```

`ctx` is a `PluginContext`; its full shape, with comments, is in [`src/plugin-api.ts`](../src/plugin-api.ts). In short:

- `library`: the tracks, `match()` to find library tracks for tracks from another service (same artist and title, similar length; version notes like "- 2011 Remaster" and "(feat. …)" are ignored), and `onChange()`.
- `playlists`: list, create, replace tracks, show.
- `player`: play or queue tracks, and `onNowPlaying()` / `onListened()` to follow what's playing. A song counts as listened to once half of it or 4 minutes have been heard, for songs over 30 seconds (Last.fm's rule), and fires once per play.
- `http.request(url, { method, headers, body })`: requests go through Honeycomb, so CORS doesn't apply. Pass an object as `body` to send a form.
- `settings` (plain strings) and `secrets` (tokens, kept in the system keychain, not in `library.db`), both private to the plugin.
- `openUrl()` and `waitForRedirect(port, path)` for OAuth: open the provider's sign-in page with `redirect_uri=http://127.0.0.1:<port><path>`, and `waitForRedirect` resolves with the redirect's query parameters.
- `ui`: sidebar entries, a section under Settings → Plugins, modals, buttons, `h()` for building DOM, toasts, a text prompt.

## Trust

Plugins run in Honeycomb's own window. The host list and port list stop mistakes, but they aren't a sandbox: a plugin could reach everything the app can. Only install plugins you trust. Plugins are off until turned on in Settings, and if one fails to load it shows the error there and the others still load.

## Type checking

Plain JavaScript works with no build step. For editor checking, a `jsconfig.json` with `"checkJs": true` and a JSDoc import of `PluginContext` from a Honeycomb checkout is enough:

```js
/** @typedef {import("../honeycomb/src/plugin-api").PluginContext} PluginContext */
```
