# SourceTube

Privacy-first, open-source YouTube client with a 2018-style dark interface, built in Rust.

SourceTube keeps Google scripts, analytics, and third-party trackers out of the page. Public metadata comes from a FreeTube-compatible Invidious instance through the SourceTube server, so your browser does not talk to Google for listings. Google sign-in uses OAuth with a read-only YouTube scope for subscriptions and personalized recommendations.

## Features

- 2018 YouTube-style layout, dark theme by default
- Rust server and Rust native-player launcher; small resident memory
- Google sign-in for subscriptions and personalized recommendations
- Search, trending, watch pages, related videos
- Native low-RAM playback through mpv/libmpv, with privacy-enhanced embed fallback
- Local watch history stored only in your browser
- No analytics, no ad scripts, no third-party trackers
- Metadata and thumbnails proxied through SourceTube
- Strict CSP and locked-down security headers
- No YouTube Data API key needed for public metadata

## What Google still sees

SourceTube removes trackers from the client, but YouTube is still YouTube:

- Sign-in goes to Google's OAuth service.
- Video playback streams from YouTube while the video is playing.
- Signed-in subscription requests go from the SourceTube server to the YouTube Data API with your read-only token.
- Public metadata comes from the configured Invidious instance, not from your browser.
- Watch history is not uploaded by SourceTube.

## Get your Google credentials

Google credentials are only needed for sign-in and subscriptions. Public search and trending work without them. Creating the OAuth client is free; no billing account or credit card is needed for personal use.

See [OAUTH.md](OAUTH.md) for the full Google Cloud setup, `.env` values, where to put the file for each install method, the port-fallback gotcha, and verification steps.

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

4. The app opens the browser automatically on the port it selected. If `8787` was busy, it uses the next free port and still opens the right URL.

Set `SOURCETUBE_NO_BROWSER=1` to suppress the browser launch. Server output is written to `~/.local/share/sourcetube/server.log`.

If your distro lacks FUSE, run:

```bash
./SourceTube-x86_64.AppImage --appimage-extract-and-run
```

Put your `.env` file in `~/.config/sourcetube/.env` or next to the AppImage before starting it. The AppImage ships the Rust server, the Rust native-player launcher, and the original SourceTube icon. Native playback needs `mpv` or `libmpv.so.2` installed on the host; without it, use the embedded player.

### From source

Requires Rust 1.85 or newer.

```bash
git clone https://github.com/fakerdoeslinux/sourcetube.git
cd sourcetube
cp .env.example .env
cargo run --release
```

Open `http://localhost:8787`.

## Install on Windows

Windows uses the portable ZIP; there is no Windows AppImage.

1. Download `SourceTube-Windows.zip` from the Releases page.
2. Extract it somewhere you can write to, for example `Documents\SourceTube`.
3. Copy `app\.env.example` to `app\.env` and fill in your credentials.
4. Double-click `sourcetube.cmd`.
5. The browser opens automatically on the selected port. If `8787` was busy, it uses the next free port and opens there. Server output is written to `%LOCALAPPDATA%\SourceTube\server.log`.

The ZIP contains native `sourcetube.exe` and `sourcetube-player.exe`. No Node.js install is needed. For low-RAM native playback, install mpv for Windows and keep `mpv.exe` in `PATH`.

## macOS

There is no native macOS build. SourceTube works on macOS through the web client: run SourceTube on a Linux machine, server, VPS, or container, then open it in Safari or Chrome on the Mac.

### Web client with Docker

```bash
git clone https://github.com/fakerdoeslinux/sourcetube.git
cd sourcetube
cp .env.example .env
docker compose up -d
```

Open `http://localhost:8787` on the Mac if Docker runs locally, or your server's HTTPS URL if hosted remotely.

Set `APP_ORIGIN` in `.env` to the public URL. For remote use, put SourceTube behind HTTPS with a reverse proxy.

## Keyboard and navigation

- Search from the top bar.
- Use the left drawer for Home, Subscriptions, and Local library.
- Videos open in the privacy-enhanced embed.
- Use **Native player** on a watch page to hand the stream to mpv/libmpv. On web-client deployments, the player starts on the machine running the SourceTube server.

## Build from source

```bash
./scripts/build-release.sh
```

Outputs land in `dist/`:

- `SourceTube-x86_64.AppImage`
- `SourceTube-Windows.zip`

Individual builds:

```bash
./scripts/build-appimage.sh
./scripts/build-windows.sh
```

## Privacy design

- No external JavaScript loaded by the frontend.
- No analytics, telemetry, pixel, or ad network.
- Thumbnails and avatars are reverse-proxied.
- Strict CSP: `default-src 'self'`, scripts and styles only from SourceTube.
- `Referrer-Policy: no-referrer`.
- `Cross-Origin-Opener-Policy: same-origin`.
- Session cookies are encrypted, HttpOnly, SameSite=Lax, and Secure on HTTPS.
- Public metadata uses a FreeTube-compatible Invidious instance configurable in `.env`.

## License

AGPL-3.0-only. See `LICENSE`.
