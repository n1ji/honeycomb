// Self-test plugin (see smoke.rs): tries to get out of its sandbox, and reports
// every way that worked. An empty list is a pass.

const within = (ms, promise) => Promise.race([promise, new Promise((_, reject) => setTimeout(() => reject(new Error("no answer")), ms))]);

/** A command call as the app's own page would word it, for the raw message pipes. */
const raw = (how) =>
  JSON.stringify({ cmd: "set_setting", callback: 1, error: 2, payload: { key: "probe.breach", value: how }, __TAURI_INVOKE_KEY__: "x" });

export async function activate(ctx) {
  const got = [];
  const attempt = async (what, fn) => {
    try {
      if (await within(3000, fn())) got.push(what);
    } catch {
      // Refused, which is the point.
    }
  };

  await attempt("read probe-a's secret through its own API", () => ctx.secrets.get("token"));
  await attempt("found the app's command bridge in its frame", async () => {
    if (typeof window.__TAURI_INTERNALS__?.invoke !== "function") return false;
    // Reported even if the call below is refused; the app checks the setting itself.
    await window.__TAURI_INTERNALS__.invoke("set_setting", { key: "probe.breach", value: "bridge" }).catch(() => {});
    return window.__TAURI_INTERNALS__.invoke("plugin_secret_get", { plugin: "probe-a", key: "token" });
  });
  for (const frame of ["parent", "top"]) {
    await attempt(`reached the app's page (${frame})`, async () => window[frame].__TAURI_INTERNALS__ !== undefined || window[frame].document !== undefined);
  }
  // These can't answer; the app looks for the setting they try to write.
  await attempt("", async () => void window.webkit?.messageHandlers?.ipc?.postMessage(raw("webkit")));
  await attempt("", async () => void window.chrome?.webview?.postMessage(raw("webview2")));
  await attempt("", async () => void window.ipc?.postMessage(raw("ipc")));
  for (const url of ["ipc://localhost/set_setting", "http://ipc.localhost/set_setting"]) {
    await attempt(`sent a command to ${url}`, async () => {
      const headers = { "Content-Type": "application/json", "Tauri-Callback": "1", "Tauri-Error": "2", "Tauri-Invoke-Key": "x" };
      await fetch(url, { method: "POST", headers, body: JSON.stringify({ key: "probe.breach", value: "fetch" }) });
      return true;
    });
  }
  await attempt("made a network request of its own", async () => (await fetch("https://example.com/", { mode: "no-cors" })) && true);
  await attempt("made a request to a host its manifest doesn't list", () => ctx.http.request("https://example.com/"));
  await attempt("imported probe-a's files", () => import(new URL("../probe-a/main.js", import.meta.url).href));
  await attempt("got storage of its own", async () => (localStorage.setItem("probe", "1"), true));

  await ctx.settings.set("crypto", typeof crypto?.subtle?.digest === "function" ? "yes" : "no");
  await ctx.settings.set("result", got.length ? got.join("; ") : "ok");
}
