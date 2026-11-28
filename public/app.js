const $ = (selector) => document.querySelector(selector);
const view = $('#view');
const account = $('#account');
const toastEl = $('#toast');
const scrim = $('#scrim');
const sidebar = $('#sidebar');
const searchForm = $('#search-form');
const searchInput = $('#search-input');

const VERSION = '1.1.3';
const CATEGORIES = {
  sponsor: 'Sponsor',
  intro: 'Intro',
  outro: 'Outro',
  selfpromo: 'Self promotion',
  interaction: 'Interaction',
  music_offtopic: 'Music segment',
  poi_highlight: 'Highlight chapter',
};
const SKIP_CATEGORIES = new Set([
  'sponsor', 'intro', 'outro', 'selfpromo', 'interaction', 'music_offtopic',
]);

const DEFAULT_SETTINGS = {
  sponsorblock: true,
  autoSkip: true,
  skipToast: true,
  adblock: true,
  categories: Object.fromEntries(Object.keys(CATEGORIES).map((key) => [key, true])),
};

function loadSettings() {
  try {
    const saved = JSON.parse(localStorage.getItem('sourcetube.settings') || '{}');
    return {
      ...DEFAULT_SETTINGS,
      ...saved,
      categories: { ...DEFAULT_SETTINGS.categories, ...(saved.categories || {}) },
    };
  } catch {
    return { ...DEFAULT_SETTINGS, categories: { ...DEFAULT_SETTINGS.categories } };
  }
}

const state = {
  config: null,
  route: null,
  settings: loadSettings(),
  player: null,
  segments: [],
  blockedChannels: new Set(),
  transferTimer: null,
};

function saveSettings() {
  localStorage.setItem('sourcetube.settings', JSON.stringify(state.settings));
}

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function thumbnailAttrs(video) {
  const fallback = `/api/thumb/${encodeURIComponent(video.id)}/mqdefault.jpg`;
  return `src="${escapeHtml(video.thumbnail)}" data-thumb-fallback="${escapeHtml(fallback)}" alt="" loading="lazy" referrerpolicy="no-referrer"`;
}

function toast(message) {
  toastEl.textContent = message;
  toastEl.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { toastEl.hidden = true; }, 4200);
}

async function api(path, options = {}) {
  const headers = {
    Accept: 'application/json',
    'X-SourceTube-Adblock': state.settings.adblock ? 'on' : 'off',
    ...(options.headers || {}),
  };
  const response = await fetch(path, {
    ...options,
    headers,
    credentials: 'same-origin',
  });
  let payload = {};
  try { payload = await response.json(); } catch { payload = {}; }
  if (!response.ok) throw new Error(payload.error || `Request failed (${response.status})`);
  return payload;
}

function formatViews(number) {
  const n = Number(number || 0);
  if (n >= 1e9) return `${(n / 1e9).toFixed(1).replace(/\.0$/, '')}B views`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(1).replace(/\.0$/, '')}M views`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(1).replace(/\.0$/, '')}K views`;
  return `${n} view${n === 1 ? '' : 's'}`;
}

function formatDuration(iso) {
  if (!iso && iso !== 0) return '';
  if (typeof iso === 'number') {
    const hours = Math.floor(iso / 3600);
    const minutes = Math.floor((iso % 3600) / 60);
    const seconds = Math.floor(iso % 60);
    return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${String(seconds).padStart(2, '0')}` : `${minutes}:${String(seconds).padStart(2, '0')}`;
  }
  const match = /^PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?$/.exec(iso);
  if (!match) return '';
  const [, hours, minutes, seconds] = match.map((part) => Number(part || 0));
  const h = hours ? `${hours}:` : '';
  const m = hours ? String(minutes).padStart(2, '0') : String(minutes);
  return `${h}${m}:${String(seconds).padStart(2, '0')}`;
}

function relativeTime(value) {
  if (typeof value === 'number') value = new Date(value * 1000).toISOString();
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const days = Math.floor((Date.now() - date.getTime()) / 86400000);
  if (days <= 0) return 'today';
  if (days === 1) return '1 day ago';
  if (days < 30) return `${days} days ago`;
  if (days < 365) return `${Math.floor(days / 30)} months ago`;
  return `${Math.floor(days / 365)} years ago`;
}

function initial(name) {
  return escapeHtml(String(name || '?').trim().charAt(0).toUpperCase());
}

function clockTime(seconds) {
  const value = Math.max(0, Math.floor(Number(seconds) || 0));
  const minutes = Math.floor(value / 60);
  return `${minutes}:${String(value % 60).padStart(2, '0')}`;
}

function videoCard(video) {
  const id = escapeHtml(video.id);
  const title = escapeHtml(video.title);
  const channel = escapeHtml(video.channel);
  const duration = formatDuration(video.duration);
  const meta = `${formatViews(video.views)} • ${relativeTime(video.publishedAt)}`;
  return `
    <article class="card">
      <a class="thumb" href="#/watch?v=${encodeURIComponent(video.id)}" aria-label="Watch ${title}">
        <img ${thumbnailAttrs(video)}>
        ${duration ? `<span class="duration">${escapeHtml(duration)}</span>` : ''}
      </a>
      <div class="card-body">
        <div class="avatar" aria-hidden="true">${initial(channel)}</div>
        <div class="card-meta">
          <a class="card-title" href="#/watch?v=${encodeURIComponent(video.id)}">${title}</a>
          <div class="card-sub">${channel}</div>
          <div class="card-sub">${escapeHtml(meta)}</div>
        </div>
      </div>
    </article>`;
}

function visibleVideos(videos) {
  const list = Array.isArray(videos) ? videos : [];
  if (!state.settings.adblock) return list;
  return list.filter((video) => !state.blockedChannels.has(String(video.channelId || video.channel || '')));
}

function videoGrid(videos, emptyText = 'Nothing here yet.') {
  const items = visibleVideos(videos);
  if (!items.length) return `<div class="state"><strong>Nothing found</strong>${escapeHtml(emptyText)}</div>`;
  return `<div class="grid">${items.map(videoCard).join('')}</div>`;
}

function setNav(name) {
  document.querySelectorAll('[data-nav]').forEach((link) => {
    link.classList.toggle('active', link.dataset.nav === name);
  });
}

function setLoading() {
  view.innerHTML = '<div class="state"><strong>Loading…</strong>Pulling metadata through the SourceTube proxy.</div>';
}

function teardownPlayer() {
  if (state.player) {
    try { state.player.reset(); } catch { /* already reset */ }
    state.player = null;
  }
  state.segments = [];
}

function teardownTransfer() {
  if (state.transferTimer) {
    clearInterval(state.transferTimer);
    state.transferTimer = null;
  }
}

async function signOut() {
  await api('/api/auth/logout').catch(() => {});
  state.config = await api('/api/config');
  transferResetNotice();
  renderAccount();
  renderRoute();
  toast('Signed out. Local transfer data stays on this device.');
}

function transferResetNotice() {
  api('/api/transfer/status').catch(() => {});
}

function renderAccount() {
  const config = state.config;
  if (!config) return;
  if (config.signedIn && config.profile) {
    const profile = config.profile;
    account.innerHTML = `
      ${profile.avatar ? `<img src="/api/avatar?url=${encodeURIComponent(profile.avatar)}" alt="" referrerpolicy="no-referrer">` : ''}
      <a class="muted" href="#/transfer">${escapeHtml(profile.name)}</a>
      <button class="button ghost small" id="signout">Sign out</button>`;
    $('#signout')?.addEventListener('click', signOut);
    return;
  }
  account.innerHTML = '<a class="button primary small" href="#/signin">Sign in</a>';
}

function readHistory() {
  try {
    return JSON.parse(localStorage.getItem('sourcetube.history') || '[]');
  } catch {
    return [];
  }
}

function saveHistory(video) {
  const history = readHistory().filter((item) => item.id !== video.id);
  history.unshift({ id: video.id, title: video.title, channel: video.channel, thumbnail: video.thumbnail, at: Date.now() });
  localStorage.setItem('sourcetube.history', JSON.stringify(history.slice(0, 50)));
}

async function renderHome() {
  setNav('home');
  document.title = 'SourceTube';
  setLoading();
  const data = await api('/api/popular');
  view.innerHTML = `
    <section class="hero">
      <p class="eyebrow">release ${VERSION} / same-origin playback</p>
      <h1>YouTube, without the tail.</h1>
      <p>SourceTube fetches public metadata and media through its own Rust server. The watch page uses a bundled DASH player and a local <code>&lt;video&gt;</code> element: no third-party embed, no error 153, no analytics. Everything here is square and monospace.</p>
      <div class="hero-actions">
        <a class="button primary" href="#/subscriptions">Subscriptions</a>
        <a class="button ghost" href="#/library">Local library</a>
        <a class="button ghost" href="#/settings">Player settings</a>
      </div>
    </section>
    <div class="page-head"><div><p class="eyebrow">feed</p><h1>Trending</h1><p>Popular videos, filtered and proxied. No personalized ad profile.</p></div></div>
    ${videoGrid(data.videos, 'The Invidious instance did not return popular videos.')}`;
}

async function renderSearch(route) {
  setNav('home');
  const query = route.params.get('q') || '';
  searchInput.value = query;
  document.title = query ? `${query} — SourceTube` : 'Search — SourceTube';
  if (!query.trim()) {
    view.innerHTML = '<div class="state"><strong>Search YouTube</strong>Type a query above to find videos.</div>';
    return;
  }
  setLoading();
  const data = await api(`/api/search?q=${encodeURIComponent(query)}`);
  view.innerHTML = `
    <div class="page-head"><div><p class="eyebrow">results</p><h1>“${escapeHtml(query)}”</h1><p>${data.videos.length} videos returned through the SourceTube proxy.</p></div></div>
    ${videoGrid(data.videos, 'Try a different query.')}`;
}

function initDash(videoElement, id) {
  const status = $('#player-status');
  if (!window.dashjs) {
    status.textContent = 'Bundled DASH player unavailable';
    status.parentElement.classList.add('error');
    return;
  }
  try {
    const player = dashjs.MediaPlayer().create();
    player.updateSettings({
      streaming: {
        buffer: { bufferTimeAtTopQuality: 12, bufferTimeAtTopQualityLongForm: 20 },
        abr: { autoSwitchBitrate: { video: true } },
      },
    });
    player.initialize(videoElement, `/api/dash/manifest?id=${encodeURIComponent(id)}`, false);
    player.on(dashjs.MediaPlayer.events.MANIFEST_LOADED, () => {
      status.textContent = 'manifest loaded • local proxy';
    });
    player.on(dashjs.MediaPlayer.events.ERROR, (event) => {
      const message = event?.error?.message || 'playback error';
      status.textContent = `playback error: ${message}`;
      status.parentElement.classList.add('error');
      toast('Playback failed. Try the native mpv player.');
    });
    state.player = player;
  } catch (error) {
    status.textContent = `player init failed: ${error.message}`;
    status.parentElement.classList.add('error');
  }
}

function setupSponsorBlock(videoElement, segments) {
  const settings = state.settings;
  state.segments = Array.isArray(segments) ? segments : [];
  const skipRow = $('#skip-row');
  const skipButton = $('#skip-segment');
  const skipLabel = $('#skip-label');
  const timeline = $('#sponsor-timeline');
  const chapterWrap = $('#chapters');
  if (!settings.sponsorblock) {
    skipRow.hidden = true;
    return;
  }

  const duration = () => Number(videoElement.duration) || 0;
  function paintTimeline() {
    if (!timeline) return;
    const total = duration();
    timeline.innerHTML = state.segments.map((segment) => {
      const start = Number(segment.segment?.[0]) || 0;
      const end = Number(segment.segment?.[1]) || 0;
      if (!total || end <= start) return '';
      const highlight = segment.category === 'poi_highlight' ? ' class="highlight"' : '';
      return `<span${highlight} style="left:${(start / total) * 100}%;width:${((end - start) / total) * 100}%"></span>`;
    }).join('');
  }
  if (videoElement.readyState >= 1) paintTimeline();
  else videoElement.addEventListener('loadedmetadata', paintTimeline, { once: true });

  const highlights = state.segments.filter((segment) => segment.category === 'poi_highlight');
  if (chapterWrap && highlights.length) {
    chapterWrap.innerHTML = highlights.map((segment) => {
      const start = Number(segment.segment?.[0]) || 0;
      return `<button class="chapter" data-time="${start}">${clockTime(start)} — ${escapeHtml(segment.description || 'Highlight')}</button>`;
    }).join('');
    chapterWrap.querySelectorAll('[data-time]').forEach((button) => {
      button.addEventListener('click', () => {
        videoElement.currentTime = Number(button.dataset.time) + 0.05;
        videoElement.play().catch(() => {});
      });
    });
  }

  const skipped = new Set();
  videoElement.addEventListener('timeupdate', () => {
    const now = videoElement.currentTime;
    const active = state.segments.find((segment) => {
      const [start, end] = segment.segment || [];
      const enabled = settings.categories[segment.category] !== false;
      return enabled && now >= Number(start) && now < Number(end);
    });
    if (!active) {
      skipRow.hidden = true;
      return;
    }
    const end = Number(active.segment[1]);
    const canSkip = SKIP_CATEGORIES.has(active.category) && settings.categories[active.category] !== false;
    skipLabel.textContent = `${CATEGORIES[active.category] || active.category} ends at ${clockTime(end)}`;
    skipButton.dataset.end = end;
    skipRow.hidden = !canSkip || settings.autoSkip;
    if (canSkip && settings.autoSkip && !skipped.has(`${active.segment[0]}-${active.segment[1]}`)) {
      skipped.add(`${active.segment[0]}-${active.segment[1]}`);
      videoElement.currentTime = end + 0.05;
      if (settings.skipToast) toast(`Skipped ${CATEGORIES[active.category] || active.category.toLowerCase()}.`);
    }
  });
  skipButton?.addEventListener('click', () => {
    videoElement.currentTime = Number(skipButton.dataset.end || 0) + 0.05;
    if (settings.skipToast) toast('Segment skipped.');
  });
}

async function renderWatch(route) {
  setNav('home');
  const id = route.params.get('v') || '';
  setLoading();
  const [detail, related, sponsor] = await Promise.all([
    api(`/api/video?id=${encodeURIComponent(id)}`),
    api(`/api/related/${encodeURIComponent(id)}`).catch(() => ({ videos: [] })),
    api(`/api/sponsorblock?id=${encodeURIComponent(id)}`).catch(() => ({ segments: [] })),
  ]);
  if (state.route?.path !== '/watch') return;
  const video = detail.video;
  document.title = `${video.title} — SourceTube`;
  saveHistory(video);

  const relatedCards = related.videos.map((item) => `
    <article class="card">
      <a class="thumb" href="#/watch?v=${encodeURIComponent(item.id)}" aria-label="Watch ${escapeHtml(item.title)}">
        <img ${thumbnailAttrs(item)}>
        ${formatDuration(item.duration) ? `<span class="duration">${escapeHtml(formatDuration(item.duration))}</span>` : ''}
      </a>
      <div class="card-body">
        <div class="card-meta">
          <a class="card-title" href="#/watch?v=${encodeURIComponent(item.id)}">${escapeHtml(item.title)}</a>
          <div class="card-sub">${escapeHtml(item.channel)}</div>
          <div class="card-sub">${escapeHtml(formatViews(item.views))}</div>
        </div>
      </div>
    </article>`).join('');

  view.innerHTML = `
    <div class="watch">
      <section>
        <p class="eyebrow">now playing / ${VERSION}</p>
        <div class="player-shell">
          <video id="video-player" controls playsinline preload="metadata"></video>
          <div class="player-bar" id="player-bar"><span><span class="live-dot"></span><span id="player-status">connecting to local DASH manifest…</span></span><span>source: sourcetube proxy</span></div>
          <div class="sponsor-timeline" id="sponsor-timeline"></div>
          <div class="skip-row" id="skip-row" hidden>
            <span id="skip-label">Sponsor segment</span>
            <button class="button small primary" id="skip-segment">Skip segment</button>
          </div>
        </div>
        <div class="chapters" id="chapters"></div>
        <h1 class="video-title">${escapeHtml(video.title)}</h1>
        <div class="video-byline">
          <div class="channel">
            <div class="avatar" aria-hidden="true">${initial(video.channel)}</div>
            <div><div class="channel-name">${escapeHtml(video.channel)}</div><div class="channel-sub">${escapeHtml(formatViews(video.views))} • ${escapeHtml(relativeTime(video.publishedAt))}</div></div>
          </div>
          <div class="hero-actions">
            <button class="button primary" id="native-player" data-id="${id}">Native player</button>
            <a class="button ghost" href="#/search?q=${encodeURIComponent(video.channel)}">Search channel</a>
          </div>
        </div>
        <div class="privacy-note">Playback is served from this SourceTube origin: the manifest and every media segment go through the Rust proxy, and decoding uses the bundled dash.js player in a local video element. No third-party player, no tracker, no embed configuration error. Use native player to hand the DASH or HLS stream to mpv/libmpv.</div>
        <div class="description">${escapeHtml(video.description || 'No description available.')}</div>
      </section>
      <aside class="related">
        <h2 class="related-title">Up next</h2>
        ${relatedCards || '<p class="muted">No recommendations returned.</p>'}
      </aside>
    </div>`;

  const videoElement = $('#video-player');
  initDash(videoElement, video.id);
  setupSponsorBlock(videoElement, sponsor.segments);

  $('#native-player')?.addEventListener('click', async (event) => {
    const button = event.currentTarget;
    button.disabled = true;
    button.textContent = 'Launching…';
    try {
      await api(`/api/launch?id=${encodeURIComponent(button.dataset.id)}`);
      toast('Native player launched through mpv/libmpv.');
    } catch (error) {
      toast(error.message);
    } finally {
      button.disabled = false;
      button.textContent = 'Native player';
    }
  });
}

async function renderSubscriptions() {
  setNav('subscriptions');
  document.title = 'Subscriptions — SourceTube';
  if (!state.config?.signedIn) {
    view.innerHTML = `
      <div class="page-head"><div><p class="eyebrow">account</p><h1>Subscriptions</h1><p>Sign in with Google to load your subscribed channels.</p></div></div>
      <div class="state"><strong>Not signed in</strong>Use <a href="#/signin">Sign in</a> to authorize read-only YouTube access, or import Takeout data from the <a href="#/transfer">transfer page</a>.</div>`;
    return;
  }
  setLoading();
  const data = await api('/api/subscriptions');
  view.innerHTML = `
    <div class="page-head"><div><p class="eyebrow">account</p><h1>Subscriptions</h1><p>Recent uploads from channels you follow, fetched server-side with your read-only token.</p></div></div>
    ${videoGrid(data.videos, 'No recent uploads found on your subscriptions.')}`;
}

async function renderLibrary() {
  setNav('library');
  document.title = 'Library — SourceTube';
  const history = readHistory();
  let imported = null;
  try { imported = await api('/api/transfer/result'); } catch { imported = null; }
  const counts = imported?.summary?.counts || {};
  const recent = visibleVideos(imported?.recent?.history || []);
  const summary = Object.entries(counts).map(([kind, count]) => `<span class="summary-chip">${escapeHtml(kind)} <strong>${escapeHtml(count)}</strong></span>`).join('');
  view.innerHTML = `
    <div class="page-head"><div><p class="eyebrow">local only</p><h1>Library</h1><p>Browser history plus metadata imported on this device. Nothing is uploaded.</p></div>
      ${history.length ? '<button class="button ghost small" id="clear-history">Clear history</button>' : ''}
    </div>
    <div class="panel">
      <h2>Imported from Google</h2>
      <p>${imported ? `Total records: <strong>${escapeHtml(imported.summary.total)}</strong>. Sign in and use the transfer wizard to add more.` : 'No transfer data yet.'}</p>
      <div class="summary-list">${summary || '<span class="muted">No imports yet.</span>'}</div>
      ${recent.length ? `<h2 style="margin-top:16px">Watch history</h2>${videoGrid(recent, '')}` : ''}
      <p style="margin-top:12px"><a class="button ghost small" href="#/transfer">Open transfer wizard</a></p>
    </div>
    <div class="panel">
      <h2>Recent in this browser</h2>
      ${videoGrid(history.map((item) => ({ ...item, views: 0, publishedAt: new Date(item.at).toISOString(), duration: '' })), 'Watch a video and it will appear here.')}
    </div>`;
  $('#clear-history')?.addEventListener('click', () => {
    localStorage.removeItem('sourcetube.history');
    renderLibrary();
    toast('Local history cleared.');
  });
}

function renderSettings() {
  setNav('settings');
  document.title = 'Settings — SourceTube';
  const s = state.settings;
  const toggle = (id, label, description, checked) => `
    <div class="setting-row">
      <label for="${id}">${label}<small>${description}</small></label>
      <input type="checkbox" id="${id}" ${checked ? 'checked' : ''}>
    </div>`;
  view.innerHTML = `
    <div class="page-head"><div><p class="eyebrow">preferences</p><h1>Settings</h1><p>Stored locally in this browser. No account, no analytics, no remote config.</p></div></div>
    <div class="panel-grid">
      <div class="panel">
        <h2>Playback</h2>
        <p>Video is decoded locally by the bundled dash.js player. Segments never leave the SourceTube origin.</p>
        ${toggle('set-sponsorblock', 'SponsorBlock', 'Load segments through the SourceTube proxy.', s.sponsorblock)}
        ${toggle('set-autoskip', 'Auto-skip segments', 'Jump to the segment end automatically.', s.autoSkip)}
        ${toggle('set-toast', 'Show skipped segment toast', 'Confirm each automatic skip.', s.skipToast)}
      </div>
      <div class="panel">
        <h2>Privacy</h2>
        ${toggle('set-adblock', 'Adblock and tracker filter', 'Drop promo fields, blocked channels and blocked hosts.', s.adblock)}
        <p>Bundled rules: <code>/adblock-rules.txt</code>. Filter lists are never downloaded at runtime.</p>
        <div class="category-list">
          ${Object.entries(CATEGORIES).map(([key, label]) => `
            <div class="setting-row">
              <label for="cat-${key}">${label}</label>
              <input type="checkbox" id="cat-${key}" data-category="${key}" ${s.categories[key] !== false ? 'checked' : ''}>
            </div>`).join('')}
        </div>
        <p style="margin-top:10px">Highlight chapters are chapter markers, never skipped.</p>
      </div>
    </div>`;
  const bind = (id, key) => $(id)?.addEventListener('change', (event) => {
    state.settings[key] = event.target.checked;
    saveSettings();
    toast('Settings saved locally.');
  });
  bind('#set-sponsorblock', 'sponsorblock');
  bind('#set-autoskip', 'autoSkip');
  bind('#set-toast', 'skipToast');
  bind('#set-adblock', 'adblock');
  document.querySelectorAll('[data-category]').forEach((input) => {
    input.addEventListener('change', () => {
      state.settings.categories[input.dataset.category] = input.checked;
      saveSettings();
      toast('Category settings saved.');
    });
  });
}

async function renderSignin() {
  setNav('signin');
  document.title = 'Sign in — SourceTube';
  const config = state.config || await api('/api/config');
  if (config.signedIn && config.profile) {
    const profile = config.profile;
    view.innerHTML = `
      <div class="page-head"><div><p class="eyebrow">connected</p><h1>Google account</h1><p>Read-only OAuth session, encrypted and scoped to this browser.</p></div></div>
      <div class="panel">
        <h2>${escapeHtml(profile.name)}</h2>
        <p>${escapeHtml(profile.email || 'Email not shared')}</p>
        <div class="summary-list">
          <span class="summary-chip">scope <strong>youtube.readonly</strong></span>
          <span class="summary-chip">tokens <strong>encrypted / session-only</strong></span>
          <span class="summary-chip">account <strong>${escapeHtml(profile.id)}</strong></span>
        </div>
        <div class="hero-actions" style="margin-top:16px">
          <a class="button primary" href="#/transfer">Open transfer</a>
          <button class="button ghost" id="signout-page">Sign out</button>
        </div>
      </div>`;
    $('#signout-page')?.addEventListener('click', signOut);
    return;
  }
  view.innerHTML = `
    <div class="page-head"><div><p class="eyebrow">oauth</p><h1>Continue with Google</h1><p>Optional. Public browsing and playback never need an account.</p></div></div>
    <div class="panel">
      <p>SourceTube requests only <code>openid</code>, <code>email</code>, <code>profile</code> and <code>https://www.googleapis.com/auth/youtube.readonly</code>. That lets the transfer wizard read subscriptions, playlists, likes and channel metadata. It cannot upload, edit or delete anything.</p>
      ${config.hasOAuth
        ? '<button class="button primary" id="google-signin">Continue with Google</button>'
        : '<p class="muted">Google OAuth is not configured. Set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET; see OAUTH.md for the exact redirect URIs.</p>'}
    </div>`;
  $('#google-signin')?.addEventListener('click', async () => {
    try {
      const result = await api('/api/auth/url');
      window.location.assign(result.url);
    } catch (error) {
      toast(error.message);
    }
  });
}

function renderStageCards(data) {
  const stages = data?.stages || [];
  const container = $('#transfer-stages');
  if (!container) return;
  container.innerHTML = stages.map((stage) => `
    <div class="stage ${escapeHtml(stage.status)}">
      <h3>${escapeHtml(CATEGORIES[stage.name] || stage.name.replaceAll('_', ' '))}</h3>
      <div class="count">${escapeHtml(stage.count ?? 0)}</div>
      <div class="state">${escapeHtml(stage.error || stage.status)}</div>
    </div>`).join('');
}

function renderTransferResult(data) {
  const container = $('#transfer-result');
  if (!container) return;
  const counts = data?.summary?.counts || {};
  container.innerHTML = Object.entries(counts).map(([kind, count]) => `<span class="summary-chip">${escapeHtml(kind)} <strong>${escapeHtml(count)}</strong></span>`).join('') || '<span class="muted">No imported records yet.</span>';
}

async function refreshTransferResult() {
  try {
    renderTransferResult(await api('/api/transfer/result'));
  } catch (error) {
    toast(error.message);
  }
}

function pollTransfer() {
  teardownTransfer();
  const update = async () => {
    try {
      const data = await api('/api/transfer/status');
      renderStageCards(data);
      if (data.status === 'running') return;
      teardownTransfer();
      await refreshTransferResult();
    } catch (error) {
      toast(error.message);
      teardownTransfer();
    }
  };
  state.transferTimer = setInterval(update, 1000);
  update();
}

async function startTransfer() {
  try {
    await api('/api/transfer/start', { method: 'POST' });
    toast('Transfer started.');
    pollTransfer();
  } catch (error) {
    toast(error.message);
  }
}

async function renderTransfer() {
  setNav('transfer');
  document.title = 'Transfer — SourceTube';
  if (!state.config?.signedIn) {
    view.innerHTML = `
      <div class="page-head"><div><p class="eyebrow">wizard</p><h1>Transfer</h1><p>Sign in first; imports are stored locally.</p></div></div>
      <div class="state"><strong>Sign-in required</strong><a class="button primary" href="#/signin">Continue with Google</a></div>`;
    return;
  }
  view.innerHTML = `
    <div class="page-head"><div><p class="eyebrow">wizard</p><h1>Transfer to SourceTube</h1><p>Read-only YouTube API import plus local Google Takeout JSON. Cancel, retry and review at any time.</p></div></div>
    <div class="panel">
      <h2>Import stages</h2>
      <div class="stage-grid" id="transfer-stages"></div>
      <div class="hero-actions">
        <button class="button primary" id="transfer-start">Start import</button>
        <button class="button ghost" id="transfer-retry">Retry</button>
        <button class="button ghost" id="transfer-cancel">Cancel</button>
      </div>
    </div>
    <div class="panel">
      <h2>Google Takeout</h2>
      <p>Export YouTube data from Google Takeout, then choose the exported <code>.json</code> files here. Files are parsed by the local SourceTube server and are never sent to a third party.</p>
      <label class="file-drop" for="takeout-files">Choose Takeout JSON files<input type="file" id="takeout-files" accept=".json,application/json" multiple hidden></label>
    </div>
    <div class="panel">
      <h2>Results</h2>
      <div class="summary-list" id="transfer-result"></div>
    </div>`;
  $('#transfer-start')?.addEventListener('click', startTransfer);
  $('#transfer-retry')?.addEventListener('click', startTransfer);
  $('#transfer-cancel')?.addEventListener('click', async () => {
    try {
      const data = await api('/api/transfer/cancel', { method: 'POST' });
      teardownTransfer();
      renderStageCards(data);
      toast('Transfer cancelled.');
    } catch (error) { toast(error.message); }
  });
  $('#takeout-files')?.addEventListener('change', async (event) => {
    const files = Array.from(event.target.files || []);
    if (!files.length) return;
    try {
      const payload = { files: [] };
      for (const file of files) payload.files.push({ name: file.name, content: await file.text() });
      const result = await api('/api/transfer/takeout', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(payload),
      });
      renderTransferResult(result);
      toast(`Imported ${Object.values(result.imported || {}).reduce((a, b) => a + b, 0)} records locally.`);
    } catch (error) {
      toast(error.message);
    }
  });
  pollTransfer();
  refreshTransferResult();
}

function parseRoute() {
  const hash = location.hash.replace(/^#/, '') || '/';
  const [path, query = ''] = hash.split('?');
  return { path, params: new URLSearchParams(query) };
}

async function renderRoute() {
  const route = parseRoute();
  state.route = route;
  if (route.path === '/watch') teardownTransfer();
  else teardownPlayer();
  if (route.path !== '/transfer') teardownTransfer();
  try {
    if (route.path === '/' || route.path === '') return await renderHome();
    if (route.path === '/search') return await renderSearch(route);
    if (route.path === '/watch') return await renderWatch(route);
    if (route.path === '/subscriptions') return await renderSubscriptions();
    if (route.path === '/library') return await renderLibrary();
    if (route.path === '/settings') return renderSettings();
    if (route.path === '/signin') return await renderSignin();
    if (route.path === '/transfer') return await renderTransfer();
    view.innerHTML = '<div class="state"><strong>Unknown page</strong>That route does not exist.</div>';
  } catch (error) {
    view.innerHTML = `<div class="state"><strong>Request failed</strong>${escapeHtml(error.message)}</div>`;
  }
}

function applyDrawer() {
  const collapsed = localStorage.getItem('sourcetube.drawer') === 'collapsed';
  sidebar.classList.toggle('collapsed', collapsed);
  view.classList.toggle('wide', collapsed);
  scrim.hidden = !collapsed || window.innerWidth > 800;
  $('#drawer-toggle').setAttribute('aria-expanded', String(!collapsed));
}

async function init() {
  document.addEventListener('error', (event) => {
    const image = event.target;
    if (!(image instanceof HTMLImageElement) || image.dataset.thumbFallbackApplied) return;
    const fallback = image.dataset.thumbFallback;
    if (!fallback) return;
    image.dataset.thumbFallbackApplied = '1';
    image.src = fallback;
  }, true);
  applyDrawer();
  $('#drawer-toggle').addEventListener('click', () => {
    const collapsed = sidebar.classList.toggle('collapsed');
    localStorage.setItem('sourcetube.drawer', collapsed ? 'collapsed' : 'open');
    applyDrawer();
  });
  scrim.addEventListener('click', () => {
    localStorage.setItem('sourcetube.drawer', 'open');
    applyDrawer();
  });
  window.addEventListener('resize', applyDrawer);
  searchForm.addEventListener('submit', (event) => {
    event.preventDefault();
    const query = searchInput.value.trim();
    if (query) location.hash = `#/search?q=${encodeURIComponent(query)}`;
  });
  window.addEventListener('hashchange', () => {
    if (location.hash.includes('signedin=') || location.hash.includes('signedout=')) return;
    renderRoute();
  });
  fetch('/adblock-rules.txt')
    .then((response) => response.text())
    .then((text) => {
      state.blockedChannels = new Set(
        text.split('\n')
          .map((line) => line.trim())
          .filter((line) => line.startsWith('channel:'))
          .map((line) => line.slice(8).trim()),
      );
    })
    .catch(() => {});

  const flag = location.hash.match(/(?:^#|&)signed(in|out)=1/);
  state.config = await api('/api/config').catch(() => ({
    signedIn: false, profile: null, hasOAuth: false, hasYouTubeKey: false,
    adblock: true, version: VERSION,
  }));
  if (flag) {
    history.replaceState(null, '', location.pathname + location.search + '#/signin');
    toast(flag[1] === 'in' ? 'Signed in. Transfer is ready.' : 'Signed out.');
  }
  renderAccount();
  renderRoute();
}

init();
