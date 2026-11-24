// Self-test plugin (see smoke.rs): tries to get out of its sandbox, and reports
// every way that worked. An empty list is a pass.

const within = (ms, promise) => Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error("no answer")), ms))]);

/** A command call as the app's own page would word it, for the raw message pipes. */
const raw = (how) =>
  JSON.stringify({ cmd: "set_setting", callback: 1, error: 2, payload: { key: "probe.breach", value: how }, __TAURI_INVOKE_KEY__: "x" });

export async function activate(ctx) {
  await ctx.settings.set("step", "started");
  const got = [];
  // All at once: on some systems a refused request only fails at the timeout,
  // and one after another they'd outlast the app's wait for plugins to start.
  const tries = [];
  const left = new Set();
  const attempt = (what, fn, key = what || `pipe ${tries.length}`) =>
    left.add(key) &&
    tries.push(
      within(3000, Promise.resolve().then(fn)).then(
        (worked) => worked && got.push(what),
        () => {}, // Refused, which is the point.
      ).finally(() => left.delete(key)),
    );

  attempt("read probe-a's secret through its own API", () => ctx.secrets.get("token"));
  attempt("found the app's command bridge in its frame", async () => {
    if (typeof window.__TAURI_INTERNALS__?.invoke !== "function") return false;
    // Reported even if the call below is refused; the app checks the setting itself.
    await window.__TAURI_INTERNALS__.invoke("set_setting", { key: "probe.breach", value: "bridge" }).catch(() => {});
    return window.__TAURI_INTERNALS__.invoke("plugin_secret_get", { plugin: "probe-a", key: "token" });
  });
  for (const frame of ["parent", "top"]) {
    attempt(`reached the app's page (${frame})`, async () => window[frame].__TAURI_INTERNALS__ !== undefined || window[frame].document !== undefined);
  }
  // These can't answer; the app looks for the setting they try to write.
  attempt("", async () => void window.webkit?.messageHandlers?.ipc?.postMessage(raw("webkit")));
  attempt("", async () => void window.chrome?.webview?.postMessage(raw("webview2")));
  attempt("", async () => void window.ipc?.postMessage(raw("ipc")));
  for (const url of ["ipc://localhost/set_setting", "http://ipc.localhost/set_setting"]) {
    attempt(`sent a command to ${url}`, async () => {
      const headers = { "Content-Type": "application/json", "Tauri-Callback": "1", "Tauri-Error": "2", "Tauri-Invoke-Key": "x" };
      await fetch(url, { method: "POST", headers, body: JSON.stringify({ key: "probe.breach", value: "fetch" }) });
      return true;
    });
  }
  attempt("made a network request of its own", async () => (await fetch("https://example.com/", { mode: "no-cors" })) && true);
  attempt("made a request to a host its manifest doesn't list", () => ctx.http.request("https://example.com/"));
  attempt("imported probe-a's files", () => import(new URL("../probe-a/main.js", import.meta.url).href));
  attempt("got storage of its own", async () => (localStorage.setItem("probe", "1"), true));

  // Which attempts are still out, in case the app stops waiting first.
  const progress = setInterval(() => void ctx.settings.set("step", `waiting on: ${[...left].join(", ") || "nothing"}`), 1000);
  await Promise.all(tries);
  clearInterval(progress);
  await ctx.settings.set("crypto", typeof crypto?.subtle?.digest === "function" ? "yes" : "no");
  await ctx.settings.set("result", got.length ? got.join("; ") : "ok");
}
