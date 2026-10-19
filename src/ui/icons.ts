/**
 * Player icons on one 24×24 grid, so they line up exactly (text glyphs like
 * ⏮ and ↻ come from different fonts and sit at different heights).
 * Filled shapes use currentColor; line icons use a 2px stroke.
 */
const PATHS = {
  play: `<path d="M8 5.5v13a1 1 0 0 0 1.52.85l10.4-6.5a1 1 0 0 0 0-1.7L9.52 4.65A1 1 0 0 0 8 5.5z" fill="currentColor"/>`,
  pause: `<rect x="6.5" y="5" width="4" height="14" rx="1.2" fill="currentColor"/><rect x="13.5" y="5" width="4" height="14" rx="1.2" fill="currentColor"/>`,
  prev: `<rect x="5" y="5.5" width="2.6" height="13" rx="1" fill="currentColor"/><path d="M19 6.7v10.6a1 1 0 0 1-1.55.83L9.6 12.83a1 1 0 0 1 0-1.66l7.85-5.3A1 1 0 0 1 19 6.7z" fill="currentColor"/>`,
  next: `<rect x="16.4" y="5.5" width="2.6" height="13" rx="1" fill="currentColor"/><path d="M5 6.7v10.6a1 1 0 0 0 1.55.83l7.85-5.3a1 1 0 0 0 0-1.66l-7.85-5.3A1 1 0 0 0 5 6.7z" fill="currentColor"/>`,
  back: `<path d="M11 6.6v10.8L3.8 12z M20 6.6v10.8L12.8 12z" fill="currentColor" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>`,
  forward: `<path d="M4 6.6v10.8L11.2 12z M13 6.6v10.8L20.2 12z" fill="currentColor" stroke="currentColor" stroke-width="1.4" stroke-linejoin="round"/>`,
  shuffle: `<g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7h3.5c2.2 0 3.4 1.1 4.5 3l1 1.8c1.1 2 2.3 3.2 4.5 3.2H20"/><path d="M4 17h3.5c1.4 0 2.4-.5 3.2-1.4"/><path d="M13.3 8.4C14.1 7.5 15.1 7 16.5 7H20"/><path d="M17.5 4.5 20 7l-2.5 2.5"/><path d="M17.5 14.5 20 17l-2.5 2.5"/></g>`,
  repeat: `<g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3.5 20 6.5l-3 3"/><path d="M4 11.5v-1a4 4 0 0 1 4-4h12"/><path d="M7 20.5l-3-3 3-3"/><path d="M20 12.5v1a4 4 0 0 1-4 4H4"/></g>`,
  repeatOne: `<g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 3.5 20 6.5l-3 3"/><path d="M4 11.5v-1a4 4 0 0 1 4-4h12"/><path d="M7 20.5l-3-3 3-3"/><path d="M20 12.5v1a4 4 0 0 1-4 4H4"/></g><path d="M11.2 10.6 12.6 9.8h.9v4.6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/>`,
  volume: `<path d="M4 9.5v5a1 1 0 0 0 1 1h2.8l4.1 3.3a.7.7 0 0 0 1.1-.55V5.75a.7.7 0 0 0-1.1-.55L7.8 8.5H5a1 1 0 0 0-1 1z" fill="currentColor"/><g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M16 9a4.2 4.2 0 0 1 0 6"/><path d="M18.6 6.4a8 8 0 0 1 0 11.2"/></g>`,
  volumeLow: `<path d="M4 9.5v5a1 1 0 0 0 1 1h2.8l4.1 3.3a.7.7 0 0 0 1.1-.55V5.75a.7.7 0 0 0-1.1-.55L7.8 8.5H5a1 1 0 0 0-1 1z" fill="currentColor"/><path d="M16 9a4.2 4.2 0 0 1 0 6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/>`,
  mute: `<path d="M4 9.5v5a1 1 0 0 0 1 1h2.8l4.1 3.3a.7.7 0 0 0 1.1-.55V5.75a.7.7 0 0 0-1.1-.55L7.8 8.5H5a1 1 0 0 0-1 1z" fill="currentColor"/><g fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="m16 9.5 5 5"/><path d="m21 9.5-5 5"/></g>`,
} as const;

export type IconName = keyof typeof PATHS;

/** An inline SVG icon, sized by the surrounding button's CSS. */
export function icon(name: IconName): SVGSVGElement {
  const tpl = document.createElement("template");
  tpl.innerHTML = `<svg class="icon icon-${name}" viewBox="0 0 24 24" aria-hidden="true">${PATHS[name]}</svg>`;
  return tpl.content.firstElementChild as SVGSVGElement;
}

/** Swap a button's icon in place (skips the DOM work when it's already showing). */
export function setIcon(el: HTMLElement, name: IconName) {
  if (el.dataset.icon === name) return;
  el.dataset.icon = name;
  el.replaceChildren(icon(name));
}
