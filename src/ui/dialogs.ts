import { h } from "../util";

export interface Modal {
  el: HTMLElement;
  body: HTMLElement;
  footer: HTMLElement;
  close: () => void;
}

/** A centered dialog. Escape or clicking the backdrop calls `onCancel`. */
export function modal(title: string, opts: { wide?: boolean; onCancel?: () => void } = {}): Modal {
  const body = h("div", { class: "modal-body" });
  const footer = h("div", { class: "modal-footer" });
  const box = h("div", { class: `modal${opts.wide ? " wide" : ""}`, role: "dialog" }, h("h2", {}, title), body, footer);
  const backdrop = h("div", { class: "backdrop" }, box);

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      cancel();
    }
  };
  let closed = false;
  const close = () => {
    if (closed) return;
    closed = true;
    window.removeEventListener("keydown", onKey, true);
    // Fade out, then go; the timeout covers a missed animationend.
    backdrop.classList.add("closing");
    const done = () => backdrop.remove();
    backdrop.addEventListener("animationend", done, { once: true });
    window.setTimeout(done, 300);
  };
  const cancel = () => {
    close();
    opts.onCancel?.();
  };
  backdrop.addEventListener("mousedown", (e) => {
    if (e.target === backdrop) cancel();
  });
  window.addEventListener("keydown", onKey, true);
  document.body.append(backdrop);
  return { el: box, body, footer, close };
}

export function button(label: string, onclick: () => void, primary = false): HTMLButtonElement {
  return h("button", { class: primary ? "btn primary" : "btn", type: "button", onclick }, label);
}

/** Replacement for window.prompt, which webviews don't reliably support. */
export function promptText(title: string, label: string, initial = ""): Promise<string | null> {
  return new Promise((resolve) => {
    const m = modal(title, { onCancel: () => resolve(null) });
    const input = h("input", { type: "text", value: initial, class: "input" });
    const submit = () => {
      const v = input.value.trim();
      m.close();
      resolve(v || null);
    };
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") submit();
    });
    m.body.append(h("label", { class: "field" }, h("span", {}, label), input));
    m.footer.append(
      button("Cancel", () => {
        m.close();
        resolve(null);
      }),
      button("OK", submit, true),
    );
    input.focus();
    input.select();
  });
}
