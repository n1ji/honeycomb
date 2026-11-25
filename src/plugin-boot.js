// The only script a plugin's frame starts with. Its frame fetches nothing:
// Honeycomb sends the runtime and the plugin's files over, and they run from
// blob: URLs. The CSP hash of this text is in tauri.conf.json (tests check it).
export const BOOT = `onmessage = async (e) => {
  if (e.source !== parent || e.data?.honeycomb !== "boot" || !e.ports[0]) return;
  onmessage = null;
  const { createRuntime, link } = await import(URL.createObjectURL(new Blob([e.data.runtime], { type: "text/javascript" })));
  const port = e.ports[0];
  try {
    const main = link(e.data.files, e.data.main);
    createRuntime(port, e.data, () => import(main)).start();
  } catch (err) {
    port.postMessage({ ready: false, error: String(err?.message ?? err) });
  }
};
parent.postMessage({ honeycomb: "listening" }, "*");`;

export async function bootHash() {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(BOOT));
  return `sha256-${btoa(String.fromCharCode(...new Uint8Array(digest)))}`;
}
