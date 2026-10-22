// Kaseki page: the pinned stack, the output-rate picker (the same rule as pick_rate in kaseki.rs),
// the volume curve and the entrances.
(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;

  // ---------- the stack: light the layer whose details are on screen ----------
  const links = $$('#k-layers a');
  const byLayer = new Map(links.map((a) => [a.dataset.layer, a]));
  const light = (name) => links.forEach((a) => a.classList.toggle('is-on', a.dataset.layer === name));
  light('ui');
  if ('IntersectionObserver' in window) {
    const io = new IntersectionObserver((entries) => {
      for (const e of entries) if (e.isIntersecting) light(e.target.dataset.layer);
    }, { rootMargin: '-45% 0px -50% 0px' });
    $$('.k-layer').forEach((el) => io.observe(el));
  }
  links.forEach((a) => a.addEventListener('click', (e) => {
    const t = document.getElementById(a.getAttribute('href').slice(1));
    if (!t) return;
    e.preventDefault();
    t.scrollIntoView({ behavior: reduce ? 'auto' : 'smooth', block: 'start' });
    light(a.dataset.layer);
    history.replaceState(null, '', a.getAttribute('href'));
  }));

  // ---------- output rate: pick_rate, as in kaseki.rs ----------
  const STANDARD = [384000, 352800, 192000, 176400, 96000, 88200, 48000, 44100, 32000, 22050];
  function pickRate(ranges, want, def) {
    const ok = (r) => ranges.some(([lo, hi]) => lo <= r && r <= hi);
    if (ok(want)) return [want, 'own'];
    for (let k = 2; k <= 8; k++) if (want % k === 0 && want / k >= 44100 && ok(want / k)) return [want / k, 'fraction'];
    for (let k = 2; k <= 8; k++) if (want * k <= 384000 && ok(want * k)) return [want * k, 'multiple'];
    const r = STANDARD.find((x) => x <= want && x > def && ok(x));
    return r ? [r, 'common'] : [def, 'default'];
  }
  const khz = (r) => `${(r / 1000).toFixed(r % 1000 ? 2 : 0).replace(/0$/, '')} kHz`;
  const khz1 = (r) => `${(r / 1000).toFixed(1)} kHz`;
  const WHY = {
    own: 'The device can run at the file’s own rate, so nothing is resampled.',
    fraction: 'The device can’t run at the file’s rate, but it can at an exact fraction of it, so the conversion is as clean as it gets.',
    multiple: 'The device can’t run at the file’s rate, but it can at an exact multiple of it.',
    common: 'No exact match, so Kaseki takes the highest common rate up to the file’s that the device offers above its default.',
    default: 'Nothing fits better, so Kaseki uses the device’s default rate and resamples.',
  };
  let file = 96000, dev = $('#k-device .is-on');
  function update() {
    const ranges = dev.dataset.ranges.split(',').map((p) => p.split('-').map(Number));
    const [out, why] = pickRate(ranges, file, +dev.dataset.default);
    $('#k-out').textContent = khz(out);
    $('#k-why').textContent = WHY[why];
    $('#k-tech').innerHTML = `FLAC · ${khz1(file)} → output <b>${khz1(out)}</b>, ${out === file ? 'no resampling' : 'resampled with a sinc filter'}`;
  }
  const chips = (box, onPick) => box.addEventListener('click', (e) => {
    const b = e.target.closest('button'); if (!b) return;
    $$('button', box).forEach((x) => { x.classList.toggle('is-on', x === b); x.setAttribute('aria-pressed', x === b); });
    onPick(b); update();
  });
  $$('#k-file button, #k-device button').forEach((b) => b.setAttribute('aria-pressed', b.classList.contains('is-on')));
  chips($('#k-file'), (b) => { file = +b.dataset.rate; });
  chips($('#k-device'), (b) => { dev = b; });
  update();

  // ---------- the volume curve: gain = volume³ (perceptual_gain in kaseki.rs) ----------
  const vol = $('#k-vol'), gain = $('#k-gain');
  const showGain = () => { const v = vol.value / 100; gain.textContent = `gain ${(v ** 3).toFixed(2)}`; };
  vol.addEventListener('input', showGain); showGain();
  const play = $('#k-play');
  play.addEventListener('click', () => { const p = play.classList.toggle('paused'); play.setAttribute('aria-label', p ? 'Play' : 'Pause'); });

  // ---------- entrances ----------
  const targets = $$('[data-reveal], .k-chart, .k-layer');
  if ('IntersectionObserver' in window) {
    const io = new IntersectionObserver((entries) => { for (const en of entries) if (en.isIntersecting) { en.target.classList.add('is-in'); io.unobserve(en.target); } }, { rootMargin: '0px 0px -4% 0px', threshold: 0 });
    targets.forEach((el) => io.observe(el));
  } else targets.forEach((el) => el.classList.add('is-in'));
  // reveal anything already scrolled past (fast jumps and anchor links skip the observer)
  let rafRev = 0;
  addEventListener('scroll', () => { if (rafRev) return; rafRev = requestAnimationFrame(() => { rafRev = 0; for (const el of targets) if (!el.classList.contains('is-in') && el.getBoundingClientRect().top < innerHeight) el.classList.add('is-in'); }); }, { passive: true });
  addEventListener('load', () => setTimeout(() => targets.forEach((el) => { const r = el.getBoundingClientRect(); if (r.top < innerHeight && r.bottom > 0) el.classList.add('is-in'); }), 2500));
})();
