const $ = (selector) => document.querySelector(selector);
const view = $('#view');
const account = $('#account');
const toastEl = $('#toast');
const scrim = $('#scrim');
const sidebar = $('#sidebar');
const searchForm = $('#search-form');
const searchInput = $('#search-input');
const SPONSOR_CATEGORIES = ['sponsor', 'intro', 'outro', 'selfpromo', 'interaction', 'music_offtopic', 'poi_highlight'];

const state = {
  config: null,
  route: null,
  loading: false,
};

function escapeHtml(value) {
  return String(value ?? '')
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function toast(message) {
  toastEl.textContent = message;
  toastEl.hidden = false;
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { toastEl.hidden = true; }, 4200);
}

function sponsorSettings() {
  const defaults = {
    enabled: true,
    autoSkip: false,
    showToast: true,
    categories: Object.fromEntries(SPONSOR_CATEGORIES.map((category) => [category, true])),
  };
  try {
    const saved = JSON.parse(localStorage.getItem('sourcetube.sponsorblock') || '{}');
    return {
      ...defaults,
      ...saved,
      categories: { ...defaults.categories, ...(saved.categories || {}) },
    };
  } catch {
    return defaults;
  }
}

function saveSponsorSettings(settings) {
  localStorage.setItem('sourcetube.sponsorblock', JSON.stringify(settings));
}

async function api(path) {
  const response = await fetch(path, { headers: { Accept: 'application/json' }, credentials: 'same-origin' });
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

function videoCard(video) {
  const id = escapeHtml(video.id);
  const title = escapeHtml(video.title);
  const channel = escapeHtml(video.channel);
  const duration = formatDuration(video.duration);
  const meta = `${formatViews(video.views)} • ${relativeTime(video.publishedAt)}`;
  return `
    <article class="card">
      <a class="thumb" href="#/watch?v=${encodeURIComponent(video.id)}" aria-label="Watch ${title}">
        <img src="${escapeHtml(video.thumbnail)}" alt="" loading="lazy" referrerpolicy="no-referrer">
        ${duration ? `<span class="duration">${escapeHtml(duration)}</span>` : ''}
      </a>
      <div class="card-body">
        <div class="avatar" aria-hidden="true">${initial(channel)}</div>
        <div class="card-meta">
          <a class="card-title" href="#/watch?v=${encodeURIComponent(video.id)}">${title}</a>
          <div class="card-sub">${escapeHtml(channel)}</div>
          <div class="card-sub">${escapeHtml(meta)}</div>
        </div>
      </div>
    </article>`;
}

function videoGrid(videos, emptyText = 'Nothing here yet.') {
  if (!videos.length) return `<div class="state"><strong>Nothing found</strong>${escapeHtml(emptyText)}</div>`;
  return `<div class="grid">${videos.map(videoCard).join('')}</div>`;
}

function setNav(name) {
  document.querySelectorAll('[data-nav]').forEach((link) => {
    link.classList.toggle('active', link.dataset.nav === name);
  });
}

function setLoading() {
  view.innerHTML = '<div class="state"><strong>Loading…</strong>Pulling metadata through SourceTube.</div>';
}

function renderAccount() {
  const config = state.config;
  if (!config) return;
  if (config.signedIn && config.profile) {
    const profile = config.profile;
    account.innerHTML = `
      ${profile.avatar ? `<img src="/api/avatar?url=${encodeURIComponent(profile.avatar)}" alt="" referrerpolicy="no-referrer">` : ''}
      <span class="muted">${escapeHtml(profile.name)}</span>
      <button class="button ghost" id="signout">Sign out</button>`;
    $('#signout')?.addEventListener('click', async () => {
      await api('/api/auth/logout').catch(() => {});
      state.config = await api('/api/config');
      renderAccount();
      renderRoute();
      toast('Signed out locally.');
    });
    return;
  }

  account.innerHTML = config.hasOAuth
    ? '<button class="button primary" id="signin">Sign in</button>'
    : '<span class="muted">OAuth not configured</span>';
  $('#signin')?.addEventListener('click', async () => {
    try {
      const result = await api('/api/auth/url');
      window.location.assign(result.url);
    } catch (error) {
      toast(error.message);
    }
  });
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
      <h1>YouTube, without the tail.</h1>
      <p>SourceTube loads public metadata through its own server, keeps Google scripts and trackers out of the page, and proxies video playback through the local server. Dark since 2018.</p>
      <div class="hero-actions">
        <a class="button primary" href="#/subscriptions">Subscriptions</a>
        <a class="button ghost" href="#/library">Local library</a>
      </div>
    </section>
    <div class="page-head"><div><h1>Trending</h1><p>Popular videos, proxied metadata, no personalized ad profile.</p></div></div>
    ${videoGrid(data.videos, 'The YouTube API did not return popular videos.')}`;
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
    <div class="page-head"><div><h1>Results for “${escapeHtml(query)}”</h1><p>${data.videos.length} videos returned through the SourceTube proxy.</p></div></div>
    ${videoGrid(data.videos, 'Try a different query.')}`;
}

async function renderWatch(route) {
  setNav('home');
  const id = route.params.get('v') || '';
  setLoading();
  const [detail, related] = await Promise.all([
    api(`/api/video?id=${encodeURIComponent(id)}`),
    api(`/api/related/${encodeURIComponent(id)}`).catch(() => ({ videos: [] })),
  ]);
  const video = detail.video;
  document.title = `${video.title} — SourceTube`;
  saveHistory(video);

  const relatedCards = related.videos.map((item) => `
    <article class="card">
      <a class="thumb" href="#/watch?v=${encodeURIComponent(item.id)}" aria-label="Watch ${escapeHtml(item.title)}">
        <img src="${escapeHtml(item.thumbnail)}" alt="" loading="lazy" referrerpolicy="no-referrer">
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
        <video class="player" id="video-player" controls playsinline preload="metadata" aria-label="${escapeHtml(video.title)}"></video>
        <div class="player-tools"><button class="button primary" id="skip-segment" hidden>Skip segment</button></div>
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
        <div class="privacy-note">Video and media segments are requested through the SourceTube server. The bundled DASH player keeps playback requests on this origin.</div>
        <div class="description">${escapeHtml(video.description || 'No description available.')}</div>
      </section>
      <aside class="related">
        <h2 class="related-title">Up next</h2>
        ${relatedCards || '<p class="muted">No recommendations returned.</p>'}
      </aside>
    </div>`;

  const player = $('#video-player');
  if (window.dashjs?.MediaPlayer) {
    const dash = window.dashjs.MediaPlayer().create();
    dash.initialize(player, `/api/dash/manifest?id=${encodeURIComponent(video.id)}`, false);
    player.addEventListener('error', () => toast('Browser playback failed to load the DASH stream.'));
    loadSponsorSegments(player, video.id);
  } else {
    player.replaceWith(Object.assign(document.createElement('p'), {
      className: 'state',
      textContent: 'The bundled DASH player did not load.',
    }));
  }

  async function loadSponsorSegments(player, id) {
    const settings = sponsorSettings();
    const skipButton = $('#skip-segment');
    if (!settings.enabled) {
      skipButton.hidden = true;
      return;
    }
    try {
      const { segments } = await api(`/api/sponsorblock?id=${encodeURIComponent(id)}`);
      const usable = segments.filter((item) =>
        item && Array.isArray(item.segment) && item.segment.length === 2
        && item.segment.every(Number.isFinite) && item.segment[1] > item.segment[0]
        && SPONSOR_CATEGORIES.includes(item.category)
        && settings.categories[item.category]);
      const skipped = new Set();
      const poi = usable.filter((item) => item.category === 'poi_highlight');
      if (poi.length) {
        const cues = poi.map((item) => {
          const start = Math.max(0, item.segment[0]);
          const end = Math.max(start + 0.1, item.segment[1]);
          const label = String(item.actionType || 'Point of interest').replace(/[\r\n]/g, ' ');
          return `${toVttTime(start)} --> ${toVttTime(end)}\n${label}`;
        });
        const track = document.createElement('track');
        track.kind = 'chapters';
        track.label = 'SponsorBlock markers';
        track.srclang = 'en';
        track.src = URL.createObjectURL(new Blob([`WEBVTT\n\n${cues.join('\n\n')}`], { type: 'text/vtt' }));
        player.append(track);
      }

      const skippable = usable.filter((item) => item.category !== 'poi_highlight');
      const activeSegment = () => skippable.find((item) => {
        const [start, end] = item.segment;
        return player.currentTime >= start && player.currentTime < end && !skipped.has(item);
      });
      const skip = (segment) => {
        if (!segment) return;
        player.currentTime = segment.segment[1];
        skipped.add(segment);
        skipButton.hidden = true;
        if (settings.showToast) toast(`Skipped ${segment.category} segment.`);
      };
      player.addEventListener('timeupdate', () => {
        const segment = activeSegment();
        if (settings.autoSkip) skip(segment);
        else {
          skipButton.hidden = !segment;
          if (segment) skipButton.textContent = `Skip ${segment.category} segment`;
        }
      });
      skipButton.addEventListener('click', () => skip(activeSegment()));
    } catch (error) {
      skipButton.hidden = true;
      toast(`SponsorBlock unavailable: ${error.message}`);
    }
  }

  function toVttTime(seconds) {
    const millis = Math.floor(seconds * 1000);
    const hours = Math.floor(millis / 3600000);
    const minutes = Math.floor((millis % 3600000) / 60000);
    const wholeSeconds = Math.floor((millis % 60000) / 1000);
    const fraction = millis % 1000;
    return `${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}:${String(wholeSeconds).padStart(2, '0')}.${String(fraction).padStart(3, '0')}`;
  }

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

async function renderSettings() {
  setNav('settings');
  document.title = 'Settings — SourceTube';
  const settings = sponsorSettings();
  const serverSettings = await api('/api/settings');
  const checked = (value) => value ? 'checked' : '';
  view.innerHTML = `
    <div class="page-head"><div><h1>Settings</h1><p>Playback preferences are stored in this browser.</p></div></div>
    <section class="settings-panel">
      <h2>SponsorBlock</h2>
      <label class="setting-row"><span>Enable SponsorBlock segments</span><input type="checkbox" data-setting="enabled" ${checked(settings.enabled)}></label>
      <label class="setting-row"><span>Automatically skip segments</span><input type="checkbox" data-setting="autoSkip" ${checked(settings.autoSkip)}></label>
      <label class="setting-row"><span>Show a toast when a segment is skipped</span><input type="checkbox" data-setting="showToast" ${checked(settings.showToast)}></label>
      <h3>Categories</h3>
      ${SPONSOR_CATEGORIES.map((category) => `<label class="setting-row"><span>${escapeHtml(category.replaceAll('_', ' '))}${category === 'poi_highlight' ? ' (chapter marker)' : ''}</span><input type="checkbox" data-category="${category}" ${checked(settings.categories[category])}></label>`).join('')}
      <h2 class="settings-section">Privacy</h2>
      <label class="setting-row"><span>Block known ad and tracker hosts</span><input type="checkbox" data-server-setting="adblockEnabled" ${checked(serverSettings.adblockEnabled)}></label>
    </section>`;
  view.querySelectorAll('[data-setting]').forEach((input) => {
    input.addEventListener('change', () => {
      const next = sponsorSettings();
      next[input.dataset.setting] = input.checked;
      saveSponsorSettings(next);
    });
  });
  view.querySelectorAll('[data-category]').forEach((input) => {
    input.addEventListener('change', () => {
      const next = sponsorSettings();
      next.categories[input.dataset.category] = input.checked;
      saveSponsorSettings(next);
    });
  });
  view.querySelectorAll('[data-server-setting]').forEach((input) => {
    input.addEventListener('change', async () => {
      input.disabled = true;
      try {
        const response = await fetch('/api/settings', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify({ [input.dataset.serverSetting]: input.checked }),
        });
        const payload = await response.json();
        if (!response.ok) throw new Error(payload.error || `Request failed (${response.status})`);
      } catch (error) {
        input.checked = !input.checked;
        toast(error.message);
      } finally {
        input.disabled = false;
      }
    });
  });
}

async function renderSignIn() {
  setNav('');
  document.title = 'Sign in — SourceTube';
  const profile = state.config?.profile;
  if (state.config?.signedIn && profile) {
    view.innerHTML = `
      <div class="page-head"><div><h1>Google account connected</h1><p>${escapeHtml(profile.name)}${profile.email ? ` · ${escapeHtml(profile.email)}` : ''}</p></div></div>
      <section class="settings-panel">
        <p><strong>Granted scope</strong><br><code>https://www.googleapis.com/auth/youtube.readonly</code></p>
        <p>Access tokens stay encrypted in the SourceTube session cookie. Sign out to clear this browser's session.</p>
        <div class="hero-actions"><a class="button primary" href="/transfer">Transfer YouTube data</a><button class="button ghost" id="signin-signout">Sign out</button></div>
      </section>`;
    $('#signin-signout').addEventListener('click', () => { location.assign('/api/auth/logout'); });
    return;
  }
  view.innerHTML = `
    <div class="page-head"><div><h1>Sign in</h1><p>Connect an account to import your YouTube data.</p></div></div>
    <section class="settings-panel">
      <p>SourceTube requests read-only access to YouTube account data. Public browsing does not require Google sign-in.</p>
      <p><strong>Requested scope</strong><br><code>https://www.googleapis.com/auth/youtube.readonly</code></p>
      ${state.config?.hasOAuth ? '<button class="button primary" id="google-signin">Continue with Google</button>' : '<p class="muted">Google OAuth is not configured. See OAUTH.md.</p>'}
    </section>`;
  $('#google-signin')?.addEventListener('click', async () => {
    try {
      const { url } = await api('/api/auth/url');
      location.assign(url);
    } catch (error) {
      toast(error.message);
    }
  });
}

async function renderTransfer() {
  setNav('transfer');
  document.title = 'Transfer — SourceTube';
  if (state.transferPoll) clearInterval(state.transferPoll);
  const status = await api('/api/transfer/status');
  let result = { counts: {}, records: {} };
  let resultError = '';
  if (state.config?.signedIn) {
    try {
      result = await api('/api/transfer/result');
    } catch (error) {
      resultError = error.message;
    }
  }
  const stages = status.stages || [];
  const counts = result.counts || {};
  const recordItems = result.records?.watch_history || [];
  const importedHistory = recordItems.map((item) => {
    const rawUrl = item.titleUrl || '';
    const id = item.videoId || (() => { try { return new URL(rawUrl).searchParams.get('v'); } catch { return ''; } })();
    if (!id) return null;
    const title = String(item.title || 'Imported video').replace(/^Watched\s+/, '');
    const channel = item.subtitles?.[0]?.name || item.channel || 'YouTube';
    return { id, title, channel, thumbnail: `/api/thumb/${encodeURIComponent(id)}/hqdefault.jpg`, views: 0, publishedAt: '', duration: 0 };
  }).filter(Boolean);
  view.innerHTML = `
    <div class="page-head"><div><h1>Transfer YouTube data</h1><p>Imports stay in SourceTube's local SQLite database. Takeout files are sent only to this server.</p></div></div>
    ${state.config?.signedIn ? `<div class="transfer-actions">
      <button class="button primary" id="transfer-start" ${status.running ? 'hidden' : ''}>${status.stage === 'failed' ? 'Retry import' : 'Start Google import'}</button>
      <button class="button ghost" id="transfer-cancel" ${status.running ? '' : 'hidden'}>Cancel</button>
      <label class="button ghost upload-label" for="takeout-files">Import Takeout JSON</label>
      <input id="takeout-files" type="file" accept=".json,application/json" multiple hidden>
    </div>` : '<div class="state"><strong>Sign in required</strong><a href="/signin">Connect Google</a> to enable account imports and Takeout transfer.</div>'}
    <section class="transfer-stages">${stages.map((stage) => `
      <article class="transfer-card ${escapeHtml(stage.status)}">
        <strong>${escapeHtml(stage.name)}</strong>
        <span>${escapeHtml(stage.status)}</span>
        ${stage.detail ? `<p>${escapeHtml(stage.detail)}</p>` : ''}
      </article>`).join('')}</section>
    ${status.error ? `<p class="transfer-error">${escapeHtml(status.error)}</p>` : ''}
    ${resultError ? `<p class="transfer-error">Could not load imported data: ${escapeHtml(resultError)}</p>` : ''}
    <section class="settings-panel">
      <h2>Imported records</h2>
      <div class="transfer-counts">${Object.entries(counts).map(([kind, count]) => `<span>${escapeHtml(kind.replaceAll('_', ' '))}: ${Number(count)}</span>`).join('') || '<span>No records imported yet.</span>'}</div>
      ${importedHistory.length ? videoGrid(importedHistory, 'No watch history imported.') : ''}
    </section>`;

  $('#transfer-start')?.addEventListener('click', async () => {
    try {
      await api('/api/transfer/start');
      await renderTransfer();
    } catch (error) {
      toast(error.message);
    }
  });
  $('#transfer-cancel')?.addEventListener('click', async () => {
    await api('/api/transfer/cancel').catch((error) => toast(error.message));
    await renderTransfer();
  });
  $('#takeout-files')?.addEventListener('change', async (event) => {
    const files = [...event.target.files];
    for (const file of files) {
      try {
        if (file.size > 16 * 1024 * 1024) throw new Error('File exceeds the 16 MiB upload limit.');
        const data = JSON.parse(await file.text());
        const response = await fetch('/api/transfer/takeout', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          credentials: 'same-origin',
          body: JSON.stringify({ filename: file.name, data }),
        });
        const payload = await response.json();
        if (!response.ok) throw new Error(payload.error || `Request failed (${response.status})`);
        toast(`${file.name}: imported ${payload.imported} records.`);
      } catch (error) {
        toast(`${file.name}: ${error.message}`);
      }
    }
    await renderTransfer();
  });
  if (status.running) {
    state.transferPoll = setInterval(() => {
      if (state.route?.path === '/transfer') renderTransfer();
      else clearInterval(state.transferPoll);
    }, 1400);
  }
}

async function renderSubscriptions() {
  setNav('subscriptions');
  document.title = 'Subscriptions — SourceTube';
  if (!state.config?.signedIn) {
    view.innerHTML = `
      <div class="page-head"><div><h1>Subscriptions</h1><p>Sign in with Google to load your subscribed channels.</p></div></div>
      <div class="state"><strong>Not signed in</strong>Use the button in the top-right to authorize read-only YouTube access.</div>`;
    return;
  }
  setLoading();
  const data = await api('/api/subscriptions');
  view.innerHTML = `
    <div class="page-head"><div><h1>Subscriptions</h1><p>Recent uploads from channels you follow, fetched server-side with your read-only token.</p></div></div>
    ${videoGrid(data.videos, 'No recent uploads found on your subscriptions.')}`;
}

function renderLibrary() {
  setNav('library');
  document.title = 'Library — SourceTube';
  const history = readHistory();
  view.innerHTML = `
    <div class="page-head"><div><h1>Local library</h1><p>Last 50 videos you opened. Stored only in this browser.</p></div>
      ${history.length ? '<button class="button ghost" id="clear-history">Clear history</button>' : ''}
    </div>
    ${videoGrid(history.map((item) => ({ ...item, views: 0, publishedAt: new Date(item.at).toISOString(), duration: '' })), 'Watch a video and it will appear here.')}`;
  $('#clear-history')?.addEventListener('click', () => {
    localStorage.removeItem('sourcetube.history');
    renderLibrary();
    toast('Local history cleared.');
  });
}

function parseRoute() {
  if (location.pathname === '/signin' || location.pathname === '/transfer') {
    return { path: location.pathname, params: new URLSearchParams(location.search) };
  }
  const hash = location.hash.replace(/^#/, '') || '/';
  const [path, query = ''] = hash.split('?');
  return { path, params: new URLSearchParams(query) };
}

async function renderRoute() {
  const route = parseRoute();
  state.route = route;
  try {
    if (route.path === '/' || route.path === '') return await renderHome();
    if (route.path === '/search') return await renderSearch(route);
    if (route.path === '/watch') return await renderWatch(route);
    if (route.path === '/subscriptions') return await renderSubscriptions();
    if (route.path === '/library') return renderLibrary();
    if (route.path === '/settings') return renderSettings();
    if (route.path === '/signin') return await renderSignIn();
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

  const flag = location.hash.match(/(?:^#|&)signed(in|out)=1/);
  state.config = await api('/api/config').catch(() => ({
    signedIn: false, profile: null, hasOAuth: false, hasYouTubeKey: false,
  }));
  if (flag) {
    history.replaceState(null, '', location.pathname + location.search + '#/');
    toast(flag[1] === 'in' ? 'Signed in. Recommendations unlocked.' : 'Signed out.');
  }
  renderAccount();
  renderRoute();
}

init();
