// Honeycomb site: the working fragments and the motion layer.
// The library here is generated in the page, so the search timings are measured in the visitor's browser.

(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const fmt = (n) => n.toLocaleString('en-US');
  const time = (s) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;

  // ---------- a sample library of 100,000 tracks ----------
  let seed = 7;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) & 0x7fffffff) / 0x7fffffff);
  const pick = (a) => a[Math.floor(rnd() * a.length)];

  const GENRES = ['Ambient', 'Blues', 'Classical', 'Country', 'Dance', 'Drum & bass', 'Electronic', 'Folk', 'Funk', 'Hip-hop', 'House', 'Indie', 'Jazz', 'Metal', 'Pop', 'Punk', 'R&B', 'Reggae', 'Rock', 'Soul', 'Soundtrack', 'Techno', 'World'];
  const FIRST = ['Ada', 'Mira', 'Theo', 'June', 'Otis', 'Nina', 'Ezra', 'Lena', 'Rafa', 'Iris', 'Milo', 'Saul', 'Vera', 'Kofi', 'Yuki', 'Ines', 'Arlo', 'Noor', 'Hugo', 'Pia', 'Cass', 'Dov', 'Elif', 'Femi'];
  const LAST = ['Lind', 'Okada', 'Marsh', 'Vale', 'Quinn', 'Holloway', 'Brandt', 'Sato', 'Reyes', 'Kerr', 'Abara', 'Fenn', 'Castell', 'Moreau', 'Ekberg', 'Halden', 'Ruiz', 'Tanaka', 'Weiss', 'Boateng', 'Novak', 'Albers'];
  const ADJ = ['Paper', 'Velvet', 'Copper', 'Quiet', 'Northern', 'Electric', 'Hollow', 'Golden', 'Midnight', 'Silver', 'Wild', 'Glass', 'Amber', 'Lunar', 'Static', 'Lucky', 'Little', 'Western', 'Distant', 'Neon', 'Salt', 'Iron'];
  const NOUNS = ['Lanterns', 'Foxes', 'Rivers', 'Engines', 'Hours', 'Satellites', 'Gardens', 'Pilots', 'Ghosts', 'Wolves', 'Machines', 'Harbours', 'Tigers', 'Signals', 'Swallows', 'Bees', 'Comets', 'Strangers', 'Kites', 'Owls', 'Tides', 'Bells'];
  const W1 = ['Blue', 'Second', 'Low', 'Open', 'Small', 'Long', 'Night', 'Paper', 'Late', 'Slow', 'Bright', 'Empty', 'Far', 'Golden', 'Quiet', 'Last', 'Morning', 'Silver', 'Summer', 'Winter', 'Red', 'Wide', 'Half', 'Lost'];
  const W2 = ['Hours', 'Nature', 'Tide', 'Water', 'Rooms', 'Distance', 'Shift', 'Moon', 'Light', 'Season', 'Signal', 'Coast', 'Fields', 'Weather', 'Static', 'Harbour', 'Engine', 'Garden', 'Window', 'Station', 'Kingdom', 'Letters', 'Mirror', 'Motion'];
  const T1 = ['Hold', 'Carry', 'Follow', 'Leave', 'Find', 'Call', 'Keep', 'Burn', 'Wait for', 'Dance with', 'Run from', 'Dream of', 'Sing to', 'Drive past', 'Fall into', 'Count'];
  const T2 = ['the Light', 'the River', 'Me Home', 'the Weather', 'the Morning', 'Your Name', 'the Static', 'the Tide', 'the Stars', 'the Line', 'the Engine', 'Midnight', 'the Bees', 'the Window', 'Gold', 'the Sea', 'Tomorrow', 'the Dark', 'the Echo', 'the Harbour'];
  const T3 = ['Interlude', 'Reprise', 'Night Drive', 'Glasshouse', 'Low Sun', 'Honey', 'Paper Planes', 'Small Hours', 'Undertow', 'Satellite', 'Afterglow', 'Northbound', 'Slow Burn', 'Open Water', 'Sleepwalk', 'Static Bloom', 'Long Room', 'Overpass'];

  // A few albums with real pictures for covers (public-domain paintings standing in for album art).
  const FEATURED = [
    { artist: 'Mira Okada', album: 'Kanagawa', genre: 'Electronic', year: 2024, cover: 'under-the-wave-off-kanagawa-kanagawa-oki.jpg', tracks: ['Off Kanagawa', 'Three Boats', 'Fuji, Far Away', 'Spray', 'Undertow', 'Woodblock'] },
    { artist: 'The Nocturnes', album: 'Southampton Water', genre: 'Ambient', year: 2021, cover: 'nocturne-blue-and-gold-southampton-water.jpg', tracks: ['Southampton Water', 'Blue and Gold', 'Harbour Lights', 'Dusk Signal', 'The Far Shore'] },
    { artist: 'Prout’s Neck', album: 'Breaking Wave', genre: 'Rock', year: 2019, cover: 'prout-s-neck-breaking-wave.jpg', tracks: ['Breaking Wave', 'Rocks at Low Tide', 'Northeaster', 'White Water'] },
    { artist: 'Les Habitués', album: 'Rouge', genre: 'Jazz', year: 2022, cover: 'at-the-moulin-rouge.jpg', tracks: ['Rouge, after Midnight', 'Table for Five', 'Green Light', 'The Long Room (live)'] },
    { artist: 'Ada Lind', album: 'By the Window', genre: 'Folk', year: 2023, cover: 'the-girl-by-the-window.jpg', tracks: ['By the Window', 'Moonlight on the Floor', 'Curtains', 'Night Shift'] },
    { artist: 'Union City Band', album: 'Honeycomb Centre', genre: 'Country', year: 2018, cover: 'honeycomb-centre-quilt.jpg', tracks: ['Honeycomb Centre', 'Patchwork', 'Eighteen Seventy-Seven', 'Hexagon Waltz'] },
    { artist: 'Hollar & the Moths', album: 'Diversae', genre: 'Indie', year: 2020, cover: 'a-moth-butterflies-and-bees-from-diversae.jpg', tracks: ['Diversae', 'Moth Light'] },
  ];

  const N = 100000;
  const T = { title: [], artist: [], album: [], genre: [], no: new Uint8Array(N), dur: new Uint16Array(N), plays: new Uint16Array(N), rate: new Uint8Array(N), hay: [] };
  let ready = false;

  function build() {
    const artists = [];
    for (const f of FIRST) for (const l of LAST) artists.push(`${f} ${l}`);
    for (const a of ADJ) for (const n of NOUNS) artists.push(`The ${a} ${n}`);
    for (const f of FIRST) for (const n of NOUNS.slice(0, 12)) artists.push(`${f} & the ${n}`);
    const genreOf = new Map(artists.map((a) => [a, pick(GENRES)]));
    let i = 0;
    const push = (title, artist, album, genre, no) => {
      T.title[i] = title; T.artist[i] = artist; T.album[i] = album; T.genre[i] = genre; T.no[i] = no;
      T.rate[i] = rnd() < 0.35 ? 0 : 3 + Math.floor(rnd() * 8);
      T.dur[i] = 90 + Math.floor(rnd() * 330); T.plays[i] = Math.floor(rnd() * rnd() * 220);
      T.hay[i] = `${title}\u0001${artist}\u0001${album}\u0001${genre}`.toLowerCase();
      i++;
    };
    for (const f of FEATURED) f.tracks.forEach((t, k) => push(t, f.artist, f.album, f.genre, k + 1));
    while (i < N) {
      const artist = pick(artists), genre = genreOf.get(artist);
      const albums = 2 + Math.floor(rnd() * 5);
      for (let a = 0; a < albums && i < N; a++) {
        const album = `${pick(W1)} ${pick(W2)}`, count = 8 + Math.floor(rnd() * 7);
        for (let t = 0; t < count && i < N; t++) {
          const r = rnd();
          push(r < 0.45 ? `${pick(T1)} ${pick(T2)}` : r < 0.8 ? pick(T3) : `${pick(W1)} ${pick(W2)}`, artist, album, genre, t + 1);
        }
      }
    }
    T.rate[0] = 9; T.plays[0] = 148; T.dur[0] = 278; // Off Kanagawa, as the Now Playing panel shows it
    ready = true;
  }

  // ---------- shared: rows and stars drawn the way the app draws them ----------
  // Ratings are half-stars, 0-10, as the app stores them. The app shows star glyphs with the left half lit for a half star; here they are drawn as SVG.
  const STAR = 'M8 .9l2.2 4.6 5 .7-3.6 3.5.9 5L8 12.3l-4.5 2.4.9-5L.8 6.2l5-.7z';
  const STARS = `<svg viewBox="0 0 88 16" aria-hidden="true">${[0, 18, 36, 54, 72].map((x) => `<path transform="translate(${x} 0)" d="${STAR}"/>`).join('')}</svg>`;
  const starsHTML = (v) => `<span class="hc-stars">${STARS}<span class="hc-stars-on" style="width:${v * 10}%">${STARS}</span></span>`;
  const NOTE = '<svg class="hc-note" viewBox="0 0 12 14" aria-label="Playing"><path d="M4.5 1.5v8.1a2.6 2.6 0 1 0 1.4 2.3V4.6l4.6-1.2V.3z"/></svg>';
  // The app's "total" format: minutes, "h min", then days past 24 hours.
  const total = (secs) => { const h = Math.floor(secs / 3600), m = Math.round((secs % 3600) / 60); return h >= 24 ? `${(secs / 86400).toFixed(1)} days` : h ? `${h} h ${m} min` : `${m} min`; };
  const plural = (n, w) => `${fmt(n)} ${w}${n === 1 ? '' : 's'}`;
  const PLAYING = 2; // "Fuji, Far Away" plays in the hero, marked with the note like the app
  const rowHTML = (i, r, mark = (x) => esc(x)) => `<div class="hc-tl-row${r % 2 ? ' odd' : ''}${i === PLAYING ? ' playing' : ''}" style="top:${r * ROW}px"><span class="c-num">${i === PLAYING ? NOTE : T.no[i]}</span><span>${mark(T.title[i])}</span><span>${mark(T.artist[i])}</span><span class="c-album">${mark(T.album[i])}</span><span class="c-num">${time(T.dur[i])}</span><span class="c-rating">${starsHTML(T.rate[i])}</span><span class="c-num c-plays">${T.plays[i] || ''}</span></div>`;

  // ---------- frame 1: search with a virtualized list ----------
  const scroll = $('#tl-scroll'), spacer = $('#tl-spacer'), rows = $('#tl-rows'), status = $('#search-status'), input = $('#search'), empty = $('#tl-empty'), libCount = $('#lib-count');
  const ROW = 24;
  let view = null; // Int32Array of matching indexes, or null for all
  let query = '';
  let allSecs = 0;

  const hl = (text) => {
    const at = query ? text.toLowerCase().indexOf(query) : -1;
    if (at < 0) return esc(text);
    return esc(text.slice(0, at)) + '<mark>' + esc(text.slice(at, at + query.length)) + '</mark>' + esc(text.slice(at + query.length));
  };

  function paint() {
    if (!ready) return;
    const n = view ? view.length : N;
    spacer.style.height = n * ROW + 'px';
    empty.hidden = n > 0;
    const first = Math.max(0, Math.floor(scroll.scrollTop / ROW) - 4);
    const last = Math.min(n, first + Math.ceil(scroll.clientHeight / ROW) + 8);
    let html = '';
    for (let r = first; r < last; r++) html += rowHTML(view ? view[r] : r, r, hl);
    rows.innerHTML = html;
  }

  function search() {
    query = input.value.trim().toLowerCase();
    if (!query) { view = null; libCount.textContent = `${plural(N, 'track')} · ${total(allSecs)}`; status.textContent = 'Search 100,000 tracks. Go on, type something.'; scroll.scrollTop = 0; paint(); return; }
    const t0 = performance.now();
    const out = new Int32Array(N); let n = 0, secs = 0;
    for (let i = 0; i < N; i++) if (T.hay[i].includes(query)) { out[n++] = i; secs += T.dur[i]; }
    view = out.subarray(0, n);
    const ms = performance.now() - t0;
    libCount.textContent = `${plural(n, 'track')} · ${total(secs)}`;
    status.textContent = `Searched all 100,000 tracks in ${ms < 1 ? ms.toFixed(2) : ms.toFixed(1)} ms, here in your browser.`;
    scroll.scrollTop = 0;
    paint();
  }

  let rafPaint = 0;
  scroll.addEventListener('scroll', () => { if (!rafPaint) rafPaint = requestAnimationFrame(() => { rafPaint = 0; paint(); }); }, { passive: true });
  input.addEventListener('input', search);
  if ('ResizeObserver' in window) new ResizeObserver(() => paint()).observe(scroll);

  // ---------- frame 2: column browser ----------
  const paneG = $('#pane-genre'), paneA = $('#pane-artist'), paneB = $('#pane-album'), bRows = $('#browser-rows'), bCount = $('#browser-count');
  const sel = { genre: 'Jazz', artist: 'Les Habitués', album: null };
  const WORD = { genre: 'genre', artist: 'artist', album: 'album' };

  function renderBrowser() {
    const g = new Set(), a = new Set(), b = new Set(), hits = [];
    let secs = 0;
    for (let i = 0; i < N; i++) {
      g.add(T.genre[i]);
      if (sel.genre && T.genre[i] !== sel.genre) continue;
      a.add(T.artist[i]);
      if (sel.artist && T.artist[i] !== sel.artist) continue;
      b.add(T.album[i]);
      if (sel.album && T.album[i] !== sel.album) continue;
      hits.push(i); secs += T.dur[i];
    }
    const list = (el, set, key) => {
      const keys = [...set].sort((x, y) => x.localeCompare(y));
      let html = `<button type="button" class="hc-pane-item all${sel[key] ? '' : ' active'}" data-k="">All (${plural(keys.length, WORD[key])})</button>`;
      for (const k of keys.slice(0, 60)) html += `<button type="button" class="hc-pane-item${sel[key] === k ? ' active' : ''}" data-k="${esc(k)}">${esc(k)}</button>`;
      el.innerHTML = html;
      el.dataset.key = key;
    };
    list(paneG, g, 'genre'); list(paneA, a, 'artist'); list(paneB, b, 'album');
    bRows.innerHTML = hits.slice(0, 8).map((i, r) => rowHTML(i, r)).join('');
    bCount.textContent = `${plural(hits.length, 'track')} · ${total(secs)}`;
    for (const el of [paneG, paneA]) { const act = el.querySelector('.active'); if (act) el.scrollTop = Math.max(0, act.offsetTop - el.clientHeight / 2); }
  }
  for (const el of [paneG, paneA, paneB]) el.addEventListener('click', (e) => {
    const btn = e.target.closest('.hc-pane-item'); if (!btn) return;
    const key = el.dataset.key; sel[key] = btn.dataset.k || null;
    if (key === 'genre') { sel.artist = null; sel.album = null; }
    if (key === 'artist') sel.album = null;
    renderBrowser();
  });

  // ---------- frames 3 and 4: Now Playing with synced lyrics, and the Inbox ----------
  const FEAT = []; // indexes of the featured albums' tracks (pushed first by build())
  const yearOf = new Map(FEATURED.map((f) => [f.album, f.year]));
  function renderNowPlaying() {
    let k = 0; for (const f of FEATURED) for (const _ of f.tracks) FEAT.push(k++);
    $('#np-rows').innerHTML = FEAT.slice(0, 16).map((i, r) => `<div class="hc-tl-row hc-cols-np${r % 2 ? ' odd' : ''}${i === 0 ? ' playing selected' : ''}"><span>${esc(T.title[i])}</span><span class="c-num">${time(T.dur[i])}</span><span class="c-rating">${starsHTML(T.rate[i])}</span><span class="c-num c-plays">${T.plays[i] || ''}</span></div>`).join('');
    renderInbox('inbox');
  }
  $$('[data-stars]').forEach((el) => { el.innerHTML = starsHTML(+el.dataset.stars); });

  const lyr = $('#lyrics');
  if (lyr) {
    const lines = $$('.hc-lyric:not(.gap)', lyr);
    let at = 3, timer = 0;
    const show = (k) => { at = k; lines.forEach((p, n) => p.classList.toggle('now', n === k)); };
    const restart = () => { clearInterval(timer); if (!reduce) timer = setInterval(() => show((at + 1) % lines.length), 2600); };
    lines.forEach((p, n) => { p.classList.add('timed'); p.addEventListener('click', () => { show(n); restart(); }); });
    show(at);
    new IntersectionObserver(([e]) => { if (e.isIntersecting) restart(); else clearInterval(timer); }).observe(lyr);
  }

  const INBOX = new Set(['Diversae', 'Honeycomb Centre']);
  function renderInbox(viewName) {
    const ids = (viewName === 'inbox' ? FEAT.filter((i) => INBOX.has(T.album[i])) : FEAT.slice()).sort((x, y) => T.artist[x].localeCompare(T.artist[y]) || T.album[x].localeCompare(T.album[y]) || T.no[x] - T.no[y]);
    const secs = ids.reduce((s, i) => s + T.dur[i], 0);
    $('#inbox-title').textContent = viewName === 'inbox' ? 'Inbox' : viewName === 'next' ? 'Next Up' : 'Music';
    $('#inbox-count').textContent = `${plural(ids.length, 'track')} · ${total(secs)}`;
    const pane = (el, key, word) => {
      const keys = [...new Set(ids.map((i) => T[key][i]))].sort((x, y) => x.localeCompare(y));
      el.innerHTML = `<div class="hc-pane-item all active">All (${plural(keys.length, word)})</div>` + keys.map((x) => `<div class="hc-pane-item">${esc(x)}</div>`).join('');
    };
    pane($('#ib-genre'), 'genre', 'genre'); pane($('#ib-artist'), 'artist', 'artist'); pane($('#ib-album'), 'album', 'album');
    $('#ib-rows').innerHTML = ids.slice(0, 10).map((i, r) => `<div class="hc-tl-row hc-cols-ib${r % 2 ? ' odd' : ''}"><span class="c-num">${T.no[i]}</span><span>${esc(T.title[i])}</span><span>${esc(T.artist[i])}</span><span class="c-num">${time(T.dur[i])}</span><span class="c-rating">${starsHTML(T.rate[i])}</span></div>`).join('');
    $$('.hc-sidebar [data-view]').forEach((b) => b.classList.toggle('active', b.dataset.view === viewName));
  }
  $$('.hc-sidebar [data-view]').forEach((b) => b.addEventListener('click', () => renderInbox(b.dataset.view)));

  // ---------- showcase: switch between the real windows ----------
  const notes = {
    smart: 'Rules in plain words: rating greater than 3.5, not played in the last 30 days, under seven minutes. The list updates as you listen.',
    tags: 'Select several tracks and edit them together. Only the fields you change are written to the files, when you press Save.',
    next: 'What plays next, in the Now Playing panel. Shuffle and repeat change it in place, even in the last seconds of a song.',
    look: 'Dark, Light, Midnight or match the system. Nine accent colours or your own, your own background, and panels where you want them.',
  };
  const tabs = $$('.index-name');
  tabs.forEach((tab) => tab.addEventListener('click', () => {
    tabs.forEach((t) => { const on = t === tab; t.classList.toggle('is-on', on); t.setAttribute('aria-selected', on); $('#sheet-' + t.dataset.sheet).hidden = !on; });
    $('#index-note').textContent = notes[tab.dataset.sheet];
  }));
  $('#index-note').textContent = notes.smart;
  $('.index-list').addEventListener('keydown', (e) => {
    if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(e.key)) return;
    const k = tabs.indexOf(document.activeElement); if (k < 0) return;
    const n = tabs[(k + (e.key === 'ArrowDown' || e.key === 'ArrowRight' ? 1 : tabs.length - 1)) % tabs.length];
    n.focus(); n.click(); e.preventDefault();
  });

  // ---------- half-star ratings: the left half of a star gives a half star, clicking the current rating clears it ----------
  for (const el of $$('.hc-rate')) {
    let v = +el.dataset.rate;
    const draw = () => { el.innerHTML = starsHTML(v); el.setAttribute('aria-valuenow', v / 2); el.setAttribute('aria-valuetext', `${v / 2} stars`); };
    draw();
    el.addEventListener('click', (e) => {
      const r = el.querySelector('.hc-stars').getBoundingClientRect();
      const picked = Math.min(10, Math.max(1, Math.ceil(((e.clientX - r.left) / r.width) * 10)));
      v = picked === v ? 0 : picked; draw();
    });
    el.addEventListener('keydown', (e) => {
      if (e.key === 'ArrowRight' || e.key === 'ArrowUp') v = Math.min(10, v + 1); else if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') v = Math.max(0, v - 1); else return;
      e.preventDefault(); draw();
    });
  }

  // ---------- switching: the four stages, each the real dialog ----------
  const stages = [
    ['First, export your old library', 'Most desktop players can save an iTunes-format library XML. Choose it, and any M3U playlists.'],
    ['Then Honeycomb finds each track', 'It matches by the end of the file path, then by artist, title, album and length, then by file name.'],
    ['Next, choose what comes across', 'Ratings, play counts, play dates, loved tracks, and each playlist that matched.'],
    ['Done. Your plays are all here', 'Where both have data, the better one wins: the higher play count, the newer play date, your own rating.'],
  ];
  const range = $('#switch-range'), clause = $('#switch-clause'), sub = $('#switch-sub'), names = $$('#switch-names li');
  const states = $$('.sw-state');
  let cur = 0;
  const go = (k) => {
    if (k === cur && clause.textContent === stages[k][0]) return;
    cur = k; range.value = k;
    range.parentElement.style.setProperty('--p', (k / 3) * 100 + '%');
    names.forEach((n, j) => n.classList.toggle('is-on', j === k));
    const swap = () => { clause.textContent = stages[k][0]; sub.textContent = stages[k][1]; states.forEach((s) => { const on = +s.dataset.stage === k; s.hidden = !on; s.classList.toggle('is-on', on); }); clause.style.opacity = sub.style.opacity = 1; };
    if (reduce) return swap();
    clause.style.opacity = sub.style.opacity = 0; setTimeout(swap, 200);
  };
  range.addEventListener('input', () => go(+range.value));
  names.forEach((n, j) => n.addEventListener('click', () => go(j)));

  // ---------- the player bar's play button: toggles, and the seek thumb moves while 'playing' ----------
  const pbPlay = $('#pb-play'), pbThumb = $('#pb-thumb'), pbPos = $('#pb-pos');
  if (pbPlay) {
    let sec = 88, timer = 0;
    const show = () => { pbThumb.style.left = (sec / 278) * 100 + '%'; pbPos.textContent = time(sec); };
    const run = () => { clearInterval(timer); if (!pbPlay.classList.contains('paused') && !reduce) timer = setInterval(() => { sec = sec >= 278 ? 0 : sec + 1; show(); }, 1000); };
    pbPlay.addEventListener('click', () => { const paused = pbPlay.classList.toggle('paused'); pbPlay.setAttribute('aria-label', paused ? 'Play' : 'Pause'); pbPlay.setAttribute('aria-pressed', String(!paused)); run(); });
    show(); new IntersectionObserver(([e]) => { if (e.isIntersecting) run(); else clearInterval(timer); }).observe(pbPlay);
  }

  // ---------- faq ----------
  const qs = $$('.faq-q'), answer = $('#faq-answer');
  const wide = matchMedia('(min-width: 900px)');
  const choose = (q) => {
    qs.forEach((x) => { const on = x === q; x.classList.toggle('is-on', on); x.setAttribute('aria-expanded', on); x.parentElement.classList.toggle('is-open', on); });
    const text = q.nextElementSibling.textContent;
    if (answer.textContent === text) return;
    if (reduce) { answer.textContent = text; return; }
    answer.style.opacity = 0; setTimeout(() => { answer.textContent = text; answer.style.opacity = 1; }, 150);
  };
  qs.forEach((q) => { q.addEventListener('click', () => choose(q)); q.addEventListener('focus', () => wide.matches && choose(q)); q.addEventListener('mouseenter', () => wide.matches && choose(q)); });
  qs[0].parentElement.classList.add('is-open');

  // ---------- letter: perforation dots ----------
  const perf = $('#perf-dots');
  if (perf) perf.innerHTML = Array.from({ length: 40 }, (_, k) => `<circle cx="${8 + k * 16}" cy="5" r="2.5" fill="currentColor"/>`).join('');

  // ---------- hero reel: the stage holds while the frames pass ----------
  const track = $('.hero-track'), reel = $('#reel'), frames = $$('.frame', reel);
  const desk = matchMedia('(min-width: 1200px)');
  let rafReel = 0;
  function moveReel() {
    rafReel = 0;
    if (!desk.matches) { reel.style.transform = ''; return; }
    const r = track.getBoundingClientRect();
    const p = Math.min(1, Math.max(0, -r.top / (r.height - innerHeight)));
    const k = p * (frames.length - 1), i = Math.floor(k), t = k - i;
    let e = Math.min(1, Math.max(0, (t - 0.3) / 0.4));
    e = e < 0.5 ? 4 * e * e * e : 1 - Math.pow(-2 * e + 2, 3) / 2;
    const step = frames[1] ? frames[1].offsetTop - frames[0].offsetTop : 0;
    reel.style.transform = `translate3d(0, ${-(i + e) * step}px, 0)`;
  }
  addEventListener('scroll', () => { if (!rafReel) rafReel = requestAnimationFrame(moveReel); }, { passive: true });
  addEventListener('resize', () => { moveReel(); paint(); });
  moveReel();

  // ---------- entrances ----------
  const targets = $$('[data-reveal], .letter');
  if ('IntersectionObserver' in window) {
    const io = new IntersectionObserver((entries) => { for (const en of entries) if (en.isIntersecting) { en.target.classList.add('is-in'); io.unobserve(en.target); } }, { rootMargin: '0px 0px -4% 0px', threshold: 0 });
    targets.forEach((el) => io.observe(el));
  } else targets.forEach((el) => el.classList.add('is-in'));
  // reveal anything already scrolled past (fast jumps and anchor links skip the observer)
  let rafRev = 0;
  addEventListener('scroll', () => { if (rafRev) return; rafRev = requestAnimationFrame(() => { rafRev = 0; for (const el of targets) if (!el.classList.contains('is-in') && el.getBoundingClientRect().top < innerHeight) el.classList.add('is-in'); }); }, { passive: true });
  addEventListener('load', () => setTimeout(() => targets.forEach((el) => { const r = el.getBoundingClientRect(); if (r.top < innerHeight && r.bottom > 0) el.classList.add('is-in'); }), 2500));

  // ---------- build the library after first paint ----------
  const start = () => {
    build();
    for (let i = 0; i < N; i++) allSecs += T.dur[i];
    libCount.textContent = `${plural(N, 'track')} · ${total(allSecs)}`;
    paint(); renderBrowser(); renderNowPlaying();
  };
  (window.requestIdleCallback || ((f) => setTimeout(f, 50)))(start, { timeout: 600 });
})();
