# SourceTube

Privacy-first, open-source YouTube client with a 2018-style dark interface.

SourceTube keeps Google scripts, analytics, and third-party trackers out of the page. Public video metadata and thumbnails are fetched by the SourceTube server, so your browser does not talk to Google for listings. Google sign-in uses OAuth with a read-only YouTube scope for recommendations and subscriptions.

## Features

- 2018 YouTube-style layout, dark theme by default
- Google sign-in for subscriptions and personalized recommendations
- Search, trending, watch pages, related videos
- Local watch history stored only in your browser
- No analytics, no ad scripts, no third-party trackers
- Metadata and thumbnails proxied through SourceTube
- Privacy-enhanced YouTube embeds via `youtube-nocookie.com`
- Strict CSP and locked-down security headers

## What Google still sees

SourceTube removes trackers from the client, but YouTube is still YouTube:

- Sign-in goes to Google's OAuth service.
- Video playback streams from YouTube's CDN while the video is playing.
- The YouTube Data API sees requests from the SourceTube server, not directly from your browser.
- Watch history is not uploaded by SourceTube.

## Get your Google credentials

1. Open Google Cloud Console and create a project.
2. Enable **YouTube Data API v3**.
3. Create an OAuth client ID of type **Web application**.
4. Add the exact redirect URI: `http://localhost:8787/api/auth/callback`
5. Copy `.env.example` to `.env`.
6. Fill in `YOUTUBE_API_KEY`, `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, and a long random `SESSION_SECRET`.

Generate a session secret with:

```bash
openssl rand -base64 48
```

## Install on Linux

### AppImage, recommended

1. Download `SourceTube-x86_64.AppImage` from the Releases page.
2. Make it executable:

```bash
chmod +x SourceTube-x86_64.AppImage
```

3. Run it:

```bash
./SourceTube-x86_64.AppImage
```

4. Open `http://localhost:8787`.

If your distro lacks FUSE, run:

```bash
./SourceTube-x86_64.AppImage --appimage-extract-and-run
```

Put your `.env` file in `~/.config/sourcetube/.env` or next to the AppImage before starting it. The AppImage writes no app data outside your normal home/runtime directories.

### From source

Requires Node.js 20 or newer.

```bash
git clone https://github.com/fakerdoeslinux/source-tube.git
cd source-tube
cp .env.example .env
npm start
```

Open `http://localhost:8787`.

## Install on Windows

Windows uses the portable ZIP; there is no Windows AppImage.

1. Download `SourceTube-Windows.zip` from the Releases page.
2. Extract it somewhere you can write to, for example `Documents\SourceTube`.
3. Install Node.js 20 or newer from `https://nodejs.org`.
4. Copy `.env.example` to `.env` and fill in your credentials.
5. Double-click `sourcetube.cmd`.
6. Open `http://localhost:8787`.

Or from a terminal:

```powershell
cd SourceTube
npm start
```

## macOS

There is no native macOS build. SourceTube works on macOS through the web client: run SourceTube on a Linux machine, server, VPS, or container, then open it in Safari or Chrome on the Mac.

### Web client with Docker

```bash
git clone https://github.com/fakerdoeslinux/source-tube.git
cd source-tube
cp .env.example .env
docker compose up -d
```

Open `http://localhost:8787` on the Mac if Docker runs locally, or your server's HTTPS URL if hosted remotely.

Set `APP_ORIGIN` in `.env` to the public URL. For remote use, put SourceTube behind HTTPS with a reverse proxy. Do not expose the raw HTTP port to the internet.

## Keyboard and navigation

- `Super + Space` style launcher is unrelated; SourceTube runs as a web app or local server.
- Search from the top bar.
- Use the left drawer for Home, Subscriptions, and Local library.
- Videos open in the privacy-enhanced embed.

## Build from source

```bash
./scripts/build-appimage.sh
./scripts/build-windows.sh
```

Outputs land in `dist/`.

## Privacy design

- No external JavaScript loaded by the frontend.
- No analytics, telemetry, pixel, or ad network.
- Thumbnails and avatars are reverse-proxied.
- Strict CSP: `default-src 'self'`, scripts and styles only from SourceTube.
- `Referrer-Policy: no-referrer`.
- `Cross-Origin-Opener-Policy: same-origin`.
- Session cookies are encrypted, HttpOnly, SameSite=Lax, and Secure on HTTPS.

## License

AGPL-3.0-only. See `LICENSE`.
