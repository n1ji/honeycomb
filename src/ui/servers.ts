// Servers (Navidrome and other OpenSubsonic servers) in Settings → Library:
// their rows next to the folders, adding one, and how they stream.
import { ask, message, open } from "@tauri-apps/plugin-dialog";
import { api, Server } from "../api";
import { store } from "../store";
import { fmtDate, h, toast } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";

/** Rows for the Settings → Library list; `refresh` redraws the list. */
export async function serverRows(refresh: () => Promise<void>): Promise<HTMLElement[]> {
  const servers = await api.servers();
  return servers.map((s) => {
    const info = [
      `${s.tracks.toLocaleString()} songs`,
      s.pinned ? `${s.pinned.toLocaleString()} offline` : "",
      s.lastSync ? `synced ${fmtDate(s.lastSync)}` : "not synced yet",
      s.waiting ? `${s.waiting} changes waiting to be sent` : "",
    ].filter(Boolean);
    const lines = [h("span", { title: s.url }, `${s.name} · ${s.url}`), h("span", { class: "muted server-info" }, `${info.join(" · ")} · keeps your ratings, plays and playlists`)];
    if (s.error) lines.push(h("span", { class: "server-error" }, s.error));
    if (scrobblingOn()) {
      lines.push(
        h(
          "span",
          { class: "muted server-info" },
          "Honeycomb's scrobbling plugin is on. If this server also scrobbles to Last.fm, plays count twice: turn one of them off.",
        ),
      );
    }
    return h(
      "li",
      { class: "server-row" },
      h("div", { class: "server-lines" }, ...lines),
      h(
        "div",
        { class: "row" },
        button("Sync", () =>
          run(async () => {
            await api.serverSync(s.id);
            toast(`Syncing ${s.name}…`);
          }),
        ),
        button("Disconnect", () => run(async () => (await disconnect(s)) && (await refresh()))),
      ),
    );
  });
}

function scrobblingOn(): boolean {
  try {
    return (JSON.parse(store.settings["plugins.enabled"] || "[]") as string[]).includes("scrobbling");
  } catch {
    return false;
  }
}

async function disconnect(s: Server): Promise<boolean> {
  const offline = s.pinned
    ? `\n\nYou have ${s.pinned.toLocaleString()} songs from it on this computer. Keep them as files in Music/Honeycomb (with their ratings and play counts), or delete them?`
    : "";
  if (!(await ask(`Disconnect from ${s.name}? Its songs leave your library; the music on the server isn't touched.${offline}`, { title: "Disconnect", kind: "warning", okLabel: "Disconnect" })))
    return false;
  let keep = true;
  if (s.pinned) {
    keep = await ask(`Keep the ${s.pinned.toLocaleString()} songs that are on this computer?`, {
      title: "Disconnect",
      okLabel: "Keep the music on this computer",
      cancelLabel: "Delete it",
    });
  }
  const r = await api.serverRemove(s.id, keep);
  toast(r.kept ? `Disconnected. ${r.kept} songs are kept in ${r.folder}.` : `Disconnected from ${s.name}.`);
  return true;
}

/** The "Add a server…" dialog; `onAdded` runs once one was added. */
export function openAddServer(onAdded: () => void) {
  const m = modal("Add a server");
  const field = (label: string, input: HTMLInputElement) => h("label", { class: "field" }, h("span", {}, label), input);
  const url = h("input", { class: "input", type: "text", placeholder: "music.example.com or 192.168.1.10:4533" });
  const user = h("input", { class: "input", type: "text", autocomplete: "username" });
  const pass = h("input", { class: "input", type: "password", autocomplete: "current-password" });
  let certPath = "";
  const certPass = h("input", { class: "input", type: "password", autocomplete: "off", placeholder: "Certificate password" });
  const certName = h("span", { class: "muted" }, "None");
  const certField = h("label", { class: "field" }, h("span", {}, "Client certificate (only if your server asks for one)"), h("div", { class: "row" }, button("Choose .p12…", () =>
    run(async () => {
      const f = await open({ multiple: false, title: "Choose a client certificate", filters: [{ name: "Certificate", extensions: ["p12", "pfx"] }] });
      if (typeof f !== "string") return;
      certPath = f;
      certName.textContent = f.split(/[\\/]/).pop() || f;
      certPass.hidden = false;
      certPass.focus();
    }),
  ), certName), certPass);
  certPass.hidden = true;
  const status = h("p", { class: "muted" }, "Navidrome and other Subsonic or OpenSubsonic servers. Honeycomb mirrors the song list, streams the music and keeps nothing else unless you make it available offline. The password goes in the system keychain.");
  const add = button(
    "Add",
    () =>
      run(async () => {
        if (!url.value.trim()) return url.focus();
        add.disabled = true;
        status.textContent = "Connecting…";
        status.classList.remove("server-error");
        try {
          await api.serverAdd(url.value.trim(), user.value.trim(), pass.value, certPath, certPass.value);
          m.close();
          toast("Server added. Syncing its library…");
          onAdded();
          if (scrobblingOn()) {
            await message(
              "Honeycomb's scrobbling plugin is on. If this server also sends your plays to Last.fm or ListenBrainz, every song you play from it will be counted twice.\n\n" +
                "Pick one to do the scrobbling: turn it off in the server's settings, or turn off Honeycomb's plugin in Settings → Plugins.",
              { title: "Scrobbling twice?", kind: "warning", okLabel: "Got it" },
            );
          }
        } catch (e) {
          status.textContent = String(e);
          status.classList.add("server-error");
          add.disabled = false;
        }
      }),
    true,
  );
  pass.addEventListener("keydown", (e) => e.key === "Enter" && add.click());
  m.body.append(field("Address", url), field("Username", user), field("Password", pass), certField, status);
  m.footer.append(button("Cancel", () => m.close()), add);
  url.focus();
}

/** The "Stream at lower quality" choice, for servers. */
export function streamQuality(setSetting: (k: string, v: string) => Promise<void>): HTMLElement {
  const sel = h("select", { class: "input" });
  for (const [v, label] of [
    ["", "Original files (best, gapless)"],
    ["opus-320", "Opus 320 kbps"],
    ["opus-192", "Opus 192 kbps"],
    ["opus-128", "Opus 128 kbps"],
    ["mp3-192", "MP3 192 kbps"],
  ]) {
    sel.append(h("option", { value: v, selected: (store.settings.serverStream || "") === v }, label));
  }
  sel.addEventListener("change", () => run(() => setSetting("serverStream", sel.value)));
  return h(
    "label",
    { class: "field" },
    h("span", {}, "Stream from servers"),
    sel,
    h("span", { class: "muted" }, "Lower quality saves data away from home. The server converts the music, so changes between songs may not be gapless. Offline copies are always the original files."),
  );
}
