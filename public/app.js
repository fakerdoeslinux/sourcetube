const $ = (selector) => document.querySelector(selector);
const view = $('#view');
const account = $('#account');
const toastEl = $('#toast');
const scrim = $('#scrim');
const sidebar = $('#sidebar');
const searchForm = $('#search-form');
const searchInput = $('#search-input');

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
      <p>SourceTube loads public metadata through its own server, keeps Google scripts and trackers out of the page, and plays video from YouTube's privacy-enhanced embed domain. Dark since 2018.</p>
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

  const embed = `https://www.youtube-nocookie.com/embed/${encodeURIComponent(video.id)}?rel=0&modestbranding=1&playsinline=1`;
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
        <iframe class="player" src="${escapeHtml(embed)}" title="${escapeHtml(video.title)}" referrerpolicy="no-referrer" allowfullscreen></iframe>
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
        <div class="privacy-note">Playback uses YouTube's privacy-enhanced embed. SourceTube injects no analytics, no ad scripts, and no third-party trackers. Your IP still reaches YouTube while the video itself streams.</div>
        <div class="description">${escapeHtml(video.description || 'No description available.')}</div>
      </section>
      <aside class="related">
        <h2 class="related-title">Up next</h2>
        ${relatedCards || '<p class="muted">No recommendations returned.</p>'}
      </aside>
    </div>`;

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
