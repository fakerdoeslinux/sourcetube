import http from 'node:http';
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import {
  createCipheriv,
  createDecipheriv,
  createHash,
  randomBytes,
} from 'node:crypto';

const ROOT = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC_DIR = path.join(ROOT, 'public');

const PORT = Number(process.env.PORT || 8787);
const APP_ORIGIN = process.env.APP_ORIGIN || `http://localhost:${PORT}`;
const GOOGLE_CLIENT_ID = process.env.GOOGLE_CLIENT_ID || '';
const GOOGLE_CLIENT_SECRET = process.env.GOOGLE_CLIENT_SECRET || '';
const YOUTUBE_API_KEY = process.env.YOUTUBE_API_KEY || '';
const SESSION_SECRET =
  process.env.SESSION_SECRET || randomBytes(32).toString('base64url');
const REDIRECT_URI =
  process.env.GOOGLE_REDIRECT_URI || `${APP_ORIGIN}/api/auth/callback`;
const KEY = createHash('sha256').update(SESSION_SECRET).digest();
const SECURE_COOKIES = APP_ORIGIN.startsWith('https://');

const OAUTH_SCOPES = [
  'openid',
  'email',
  'profile',
  'https://www.googleapis.com/auth/youtube.readonly',
].join(' ');

const CSP = [
  "default-src 'self'",
  "script-src 'self'",
  "style-src 'self'",
  "img-src 'self' data:",
  "media-src 'self' https://www.youtube-nocookie.com",
  "frame-src https://www.youtube-nocookie.com",
  "connect-src 'self'",
  "font-src 'self' data:",
  "object-src 'none'",
  "base-uri 'none'",
  "form-action 'self'",
  "frame-ancestors 'none'",
].join('; ');

const SECURITY_HEADERS = {
  'Content-Security-Policy': CSP,
  'Referrer-Policy': 'no-referrer',
  'X-Content-Type-Options': 'nosniff',
  'X-Frame-Options': 'DENY',
  'Cross-Origin-Opener-Policy': 'same-origin',
  'Permissions-Policy':
    'camera=(), microphone=(), geolocation=(), payment=(), interest-cohort=()',
};

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

function seal(value) {
  const iv = randomBytes(12);
  const cipher = createCipheriv('aes-256-gcm', KEY, iv);
  const ciphertext = Buffer.concat([
    cipher.update(JSON.stringify(value), 'utf8'),
    cipher.final(),
  ]);
  return Buffer.concat([iv, cipher.getAuthTag(), ciphertext]).toString('base64url');
}

function unseal(token) {
  if (typeof token !== 'string' || !token) return null;
  try {
    const raw = Buffer.from(token, 'base64url');
    if (raw.length < 29) return null;
    const iv = raw.subarray(0, 12);
    const tag = raw.subarray(12, 28);
    const ciphertext = raw.subarray(28);
    const decipher = createDecipheriv('aes-256-gcm', KEY, iv);
    decipher.setAuthTag(tag);
    const plain = Buffer.concat([
      decipher.update(ciphertext),
      decipher.final(),
    ]);
    return JSON.parse(plain.toString('utf8'));
  } catch {
    return null;
  }
}

function parseCookies(req) {
  const out = {};
  const header = req.headers.cookie || '';
  for (const part of header.split(';')) {
    const index = part.indexOf('=');
    if (index < 0) continue;
    const name = part.slice(0, index).trim();
    const value = part.slice(index + 1).trim();
    if (name) out[name] = decodeURIComponent(value);
  }
  return out;
}

function addCookie(res, name, value, maxAge) {
  const flags = [
    `${name}=${encodeURIComponent(value)}`,
    'Path=/',
    'HttpOnly',
    'SameSite=Lax',
    `Max-Age=${maxAge}`,
  ];
  if (SECURE_COOKIES) flags.push('Secure');
  const existing = res.getHeader('Set-Cookie');
  const list = Array.isArray(existing) ? existing : existing ? [existing] : [];
  list.push(flags.join('; '));
  res.setHeader('Set-Cookie', list);
}

function send(res, status, body, type = 'application/json; charset=utf-8', extra = {}) {
  res.writeHead(status, {
    ...SECURITY_HEADERS,
    'Content-Type': type,
    'Cache-Control': 'no-store',
    ...extra,
  });
  res.end(body);
}

function sendJson(res, status, value) {
  send(res, status, JSON.stringify(value));
}

function redirect(res, location) {
  res.writeHead(302, { ...SECURITY_HEADERS, Location: location, 'Cache-Control': 'no-store' });
  res.end();
}

async function postForm(url, params) {
  const response = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/x-www-form-urlencoded', Accept: 'application/json' },
    body: new URLSearchParams(params).toString(),
    redirect: 'manual',
  });
  const text = await response.text();
  let payload = {};
  try {
    payload = text ? JSON.parse(text) : {};
  } catch {
    payload = {};
  }
  if (!response.ok) {
    throw new ApiError(502, payload.error_description || payload.error?.message || `OAuth request failed (${response.status})`);
  }
  return payload;
}

async function profileFromIdToken(idToken) {
  if (!idToken) return null;
  const url = new URL('https://oauth2.googleapis.com/tokeninfo');
  url.searchParams.set('id_token', idToken);
  const response = await fetch(url, { headers: { Accept: 'application/json' }, redirect: 'manual' });
  if (!response.ok) return null;
  const data = await response.json();
  return {
    id: data.sub || '',
    name: data.name || data.email || 'Signed in',
    email: data.email || '',
    avatar: data.picture || '',
  };
}

async function refreshSession(session) {
  if (!session?.refresh_token) return null;
  const tokens = await postForm('https://oauth2.googleapis.com/token', {
    client_id: GOOGLE_CLIENT_ID,
    client_secret: GOOGLE_CLIENT_SECRET,
    grant_type: 'refresh_token',
    refresh_token: session.refresh_token,
  });
  return {
    ...session,
    access_token: tokens.access_token || session.access_token,
    refresh_token: tokens.refresh_token || session.refresh_token,
    expiry: Math.floor(Date.now() / 1000) + Number(tokens.expires_in || 3600),
  };
}

async function loadSession(req, res) {
  const cookies = parseCookies(req);
  const session = unseal(cookies.st_session);
  if (!session) return null;
  const expiresAt = Number(session.expiry || 0);
  if (expiresAt > Date.now() / 1000 + 30) return session;
  const refreshed = await refreshSession(session);
  if (!refreshed) return null;
  addCookie(res, 'st_session', seal(refreshed), 60 * 60 * 24 * 30);
  return refreshed;
}

function requireCreds() {
  if (!GOOGLE_CLIENT_ID || !GOOGLE_CLIENT_SECRET) {
    throw new ApiError(503, 'Set GOOGLE_CLIENT_ID and GOOGLE_CLIENT_SECRET to enable Google sign-in.');
  }
}

function validateVideoId(id) {
  if (!/^[A-Za-z0-9_-]{11}$/.test(id || '')) {
    throw new ApiError(400, 'Invalid video ID.');
  }
  return id;
}

async function youtube(pathname, params, session) {
  const url = new URL(`https://www.googleapis.com/youtube/v3/${pathname}`);
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null && value !== '') {
      url.searchParams.set(key, String(value));
    }
  }
  const headers = {
    Accept: 'application/json',
    'User-Agent': 'SourceTube/1.0 (+privacy-first YouTube client)',
  };
  if (session?.access_token) {
    headers.Authorization = `Bearer ${session.access_token}`;
  } else if (YOUTUBE_API_KEY) {
    url.searchParams.set('key', YOUTUBE_API_KEY);
  } else {
    throw new ApiError(503, 'Set YOUTUBE_API_KEY to load public YouTube data.');
  }

  const response = await fetch(url, { headers, redirect: 'manual' });
  const text = await response.text();
  let payload = {};
  try {
    payload = text ? JSON.parse(text) : {};
  } catch {
    payload = {};
  }
  if (!response.ok) {
    const message = payload.error?.message || payload.error?.errors?.[0]?.reason || `YouTube API error (${response.status})`;
    throw new ApiError(502, message);
  }
  return payload;
}

function mapVideo(item) {
  const id = typeof item.id === 'string' ? item.id : item.id?.videoId || '';
  const snippet = item.snippet || {};
  const statistics = item.statistics || {};
  return {
    id,
    title: snippet.title || 'Untitled video',
    channel: snippet.channelTitle || 'Unknown channel',
    channelId: snippet.channelId || '',
    description: snippet.description || '',
    publishedAt: snippet.publishedAt || '',
    thumbnail: id ? `/api/thumb/${id}/hqdefault.jpg` : '',
    views: Number(statistics.viewCount || 0),
    likes: Number(statistics.likeCount || 0),
    duration: item.contentDetails?.duration || '',
  };
}

function uniqueVideoIds(items) {
  return [...new Set(items.map((item) => item.id?.videoId || (typeof item.id === 'string' ? item.id : '')).filter(Boolean))];
}

async function videoDetails(ids, session) {
  if (!ids.length) return [];
  const result = await youtube(
    'videos',
    { part: 'snippet,statistics,contentDetails', id: ids.slice(0, 50).join(',') },
    session,
  );
  return (result.items || []).map(mapVideo);
}

async function handleAuthUrl(req, res) {
  requireCreds();
  const verifier = randomBytes(32).toString('base64url');
  const challenge = createHash('sha256').update(verifier).digest('base64url');
  const state = randomBytes(16).toString('base64url');
  addCookie(res, 'st_pkce', seal({ verifier, state }), 600);

  const url = new URL('https://accounts.google.com/o/oauth2/v2/auth');
  url.searchParams.set('client_id', GOOGLE_CLIENT_ID);
  url.searchParams.set('redirect_uri', REDIRECT_URI);
  url.searchParams.set('response_type', 'code');
  url.searchParams.set('scope', OAUTH_SCOPES);
  url.searchParams.set('access_type', 'offline');
  url.searchParams.set('prompt', 'consent');
  url.searchParams.set('code_challenge', challenge);
  url.searchParams.set('code_challenge_method', 'S256');
  url.searchParams.set('state', state);
  redirect(res, url.toString());
}

async function handleAuthCallback(req, res, url) {
  requireCreds();
  const code = url.searchParams.get('code');
  const state = url.searchParams.get('state');
  const cookies = parseCookies(req);
  const pkce = unseal(cookies.st_pkce);
  if (!code || !pkce || state !== pkce.state) {
    throw new ApiError(400, 'OAuth callback was missing a valid state or code.');
  }

  const tokens = await postForm('https://oauth2.googleapis.com/token', {
    code,
    client_id: GOOGLE_CLIENT_ID,
    client_secret: GOOGLE_CLIENT_SECRET,
    redirect_uri: REDIRECT_URI,
    grant_type: 'authorization_code',
    code_verifier: pkce.verifier,
  });

  const profile = await profileFromIdToken(tokens.id_token);
  const session = {
    access_token: tokens.access_token || '',
    refresh_token: tokens.refresh_token || '',
    expiry: Math.floor(Date.now() / 1000) + Number(tokens.expires_in || 3600),
    profile,
  };
  addCookie(res, 'st_session', seal(session), 60 * 60 * 24 * 30);
  addCookie(res, 'st_pkce', '', 0);
  redirect(res, `${APP_ORIGIN}/#signedin=1`);
}

async function handleLogout(req, res) {
  addCookie(res, 'st_session', '', 0);
  addCookie(res, 'st_pkce', '', 0);
  redirect(res, `${APP_ORIGIN}/#signedout=1`);
}

async function handlePopular(req, res, url, session) {
  const region = (url.searchParams.get('region') || 'US').slice(0, 2).toUpperCase();
  const result = await youtube(
    'videos',
    { part: 'snippet,statistics', chart: 'mostPopular', maxResults: 24, regionCode: region },
    session,
  );
  sendJson(res, 200, { videos: (result.items || []).map(mapVideo) });
}

async function handleSearch(req, res, url, session) {
  const query = (url.searchParams.get('q') || '').trim().slice(0, 120);
  if (!query) return sendJson(res, 200, { videos: [] });
  const search = await youtube(
    'search',
    { part: 'snippet', type: 'video', maxResults: 24, q: query },
    session,
  );
  const videos = await videoDetails(uniqueVideoIds(search.items || []), session);
  const byId = new Map(videos.map((video) => [video.id, video]));
  const ordered = (search.items || [])
    .map((item) => byId.get(item.id?.videoId))
    .filter(Boolean);
  sendJson(res, 200, { videos: ordered });
}

async function handleVideo(req, res, url, session) {
  const id = validateVideoId(url.searchParams.get('id'));
  const details = await videoDetails([id], session);
  if (!details.length) throw new ApiError(404, 'Video not found.');
  sendJson(res, 200, { video: details[0] });
}

async function handleRelated(req, res, session, id) {
  validateVideoId(id);
  const search = await youtube(
    'search',
    { part: 'snippet', type: 'video', maxResults: 16, relatedToVideoId: id },
    session,
  );
  const videos = await videoDetails(uniqueVideoIds(search.items || []), session);
  const byId = new Map(videos.map((video) => [video.id, video]));
  const ordered = (search.items || [])
    .map((item) => byId.get(item.id?.videoId))
    .filter(Boolean);
  sendJson(res, 200, { videos: ordered });
}

async function handleSubscriptions(req, res, session) {
  if (!session) throw new ApiError(401, 'Sign in to see subscriptions.');
  const subs = await youtube(
    'subscriptions',
    { part: 'snippet', mine: true, maxResults: 48 },
    session,
  );
  const channelIds = (subs.items || [])
    .map((item) => item.snippet?.channelId)
    .filter(Boolean)
    .slice(0, 12);
  if (!channelIds.length) return sendJson(res, 200, { videos: [] });

  const channels = await youtube(
    'channels',
    { part: 'contentDetails', id: channelIds.join(','), maxResults: 50 },
    session,
  );
  const playlists = (channels.items || [])
    .map((item) => item.contentDetails?.relatedPlaylists?.uploads)
    .filter(Boolean)
    .slice(0, 12);

  const batches = await Promise.all(
    playlists.map((playlistId) =>
      youtube(
        'playlistItems',
        { part: 'snippet', playlistId, maxResults: 4 },
        session,
      ).catch(() => ({ items: [] })),
    ),
  );

  const ids = uniqueVideoIds(batches.flatMap((batch) => batch.items || []));
  const videos = await videoDetails(ids, session);
  videos.sort((a, b) => String(b.publishedAt).localeCompare(String(a.publishedAt)));
  sendJson(res, 200, { videos });
}

async function handleThumbnail(res, id, name) {
  validateVideoId(id);
  const allowed = new Set(['default', 'mqdefault', 'hqdefault', 'sddefault', 'maxresdefault']);
  if (!allowed.has(name)) throw new ApiError(400, 'Invalid thumbnail size.');
  const upstream = await fetch(`https://i.ytimg.com/vi/${id}/${name}.jpg`, {
    headers: { 'User-Agent': 'SourceTube/1.0 (+privacy-first YouTube client)' },
    redirect: 'manual',
  });
  if (!upstream.ok) throw new ApiError(404, 'Thumbnail not found.');
  const buffer = Buffer.from(await upstream.arrayBuffer());
  send(res, 200, buffer, 'image/jpeg', {
    'Cache-Control': 'public, max-age=86400, immutable',
  });
}

async function handleAvatar(res, rawUrl) {
  let upstreamUrl;
  try {
    upstreamUrl = new URL(rawUrl || '');
  } catch {
    throw new ApiError(400, 'Invalid avatar URL.');
  }
  const host = upstreamUrl.hostname.toLowerCase();
  if (upstreamUrl.protocol !== 'https:' || !(host === 'googleusercontent.com' || host.endsWith('.googleusercontent.com'))) {
    throw new ApiError(400, 'Avatar host is not allowed.');
  }
  const upstream = await fetch(upstreamUrl, {
    headers: { 'User-Agent': 'SourceTube/1.0 (+privacy-first YouTube client)' },
    redirect: 'manual',
  });
  if (!upstream.ok) throw new ApiError(404, 'Avatar not found.');
  const buffer = Buffer.from(await upstream.arrayBuffer());
  send(res, 200, buffer, upstream.headers.get('content-type') || 'image/jpeg', {
    'Cache-Control': 'public, max-age=86400',
  });
}

async function handleStatic(req, res, pathname) {
  const relative = pathname === '/' ? 'index.html' : decodeURIComponent(pathname).replace(/^\/+/, '');
  const filePath = path.resolve(PUBLIC_DIR, relative);
  if (!filePath.startsWith(PUBLIC_DIR + path.sep) && filePath !== path.join(PUBLIC_DIR, 'index.html')) {
    throw new ApiError(403, 'Forbidden.');
  }
  const data = await fs.readFile(filePath).catch(() => null);
  if (!data) throw new ApiError(404, 'Not found.');
  const type = MIME[path.extname(filePath).toLowerCase()] || 'application/octet-stream';
  send(res, 200, data, type, { 'Cache-Control': 'public, max-age=300' });
}

async function route(req, res) {
  const url = new URL(req.url || '/', APP_ORIGIN);
  const pathname = url.pathname;

  if (pathname.startsWith('/api/')) {
    const session = await loadSession(req, res);

    if (pathname === '/api/config') {
      return sendJson(res, 200, {
        signedIn: Boolean(session),
        profile: session?.profile || null,
        hasOAuth: Boolean(GOOGLE_CLIENT_ID && GOOGLE_CLIENT_SECRET),
        hasYouTubeKey: Boolean(YOUTUBE_API_KEY),
        privacy: {
          analytics: false,
          trackers: false,
          clientIpToGoogleForMetadata: false,
        },
      });
    }
    if (pathname === '/api/me') return sendJson(res, 200, { profile: session?.profile || null });
    if (pathname === '/api/auth/url') return handleAuthUrl(req, res);
    if (pathname === '/api/auth/callback') return handleAuthCallback(req, res, url);
    if (pathname === '/api/auth/logout') return handleLogout(req, res);
    if (pathname === '/api/popular') return handlePopular(req, res, url, session);
    if (pathname === '/api/search') return handleSearch(req, res, url, session);
    if (pathname === '/api/video') return handleVideo(req, res, url, session);
    if (pathname === '/api/subscriptions') return handleSubscriptions(req, res, session);

    const relatedMatch = pathname.match(/^\/api\/related\/([^/]+)$/);
    if (relatedMatch) return handleRelated(req, res, session, relatedMatch[1]);

    const thumbMatch = pathname.match(/^\/api\/thumb\/([^/]+)\/([^/]+)$/);
    if (thumbMatch) return handleThumbnail(res, thumbMatch[1], thumbMatch[2]);

    if (pathname === '/api/avatar') return handleAvatar(res, url.searchParams.get('url'));

    throw new ApiError(404, 'Unknown API endpoint.');
  }

  return handleStatic(req, res, pathname);
}

const server = http.createServer((req, res) => {
  route(req, res).catch((error) => {
    const status = error instanceof ApiError ? error.status : 500;
    const message = error instanceof ApiError ? error.message : 'Internal server error.';
    if (!res.headersSent) sendJson(res, status, { error: message });
    else res.end();
  });
});

server.listen(PORT, () => {
  console.log(`SourceTube listening on ${APP_ORIGIN}`);
});
