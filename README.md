# SourceTube

Privacy-first, open-source YouTube client with a sharp-cornered, monospace 2018-style dark interface, built in Rust.

SourceTube keeps Google scripts, analytics, and third-party trackers out of the page. Public metadata comes from a FreeTube-compatible Invidious instance through the SourceTube server, so your browser does not talk to Google for listings. Google sign-in uses optional OAuth with a read-only YouTube scope for subscriptions and transfers. Version 1.1.3 plays video through a bundled DASH player and a server-side media proxy: the watch page has no third-party player embed and cannot hit YouTube embed error 153.

## Features

- Sharp-corner UI using a locally bundled Iosevka Nerd Font Mono; no web fonts from CDNs
- Rust server and Rust native-player launcher; small resident memory
- Same-origin DASH playback: bundled `dash.js`, local `<video controls playsinline>`, proxied MPD and segments
- Native low-RAM playback through mpv/libmpv, accepting progressive, DASH and HLS URLs
- SponsorBlock with auto-skip, manual skip, category toggles and highlight chapters, fetched through SourceTube
- Bundled ad/tracker rules applied to outbound fetches and to promoted metadata
- Google sign-in and a transfer wizard for subscriptions, playlists, playlist items, likes and Takeout JSON
- Local SQLite storage for imported data under `SOURCETUBE_DATA`
- Local watch history stored only in your browser
- No analytics, no ad scripts, no third-party trackers
- Strict CSP, encrypted HttpOnly session cookies, no external runtime assets

## How playback works

The watch page requests `/api/dash/manifest?id=<video-id>`. The Rust server asks the configured Invidious instance for the video metadata, fetches its MPD server-side, and rewrites every `BaseURL` to `/api/dash/segment?id=<video-id>&u=<encoded upstream URL>`. Segment requests are validated against an HTTPS-only, host-allowlisted URL guard, require a sane `Range`, are capped at 32 MiB, and are passed back as `206 Partial Content` with `Content-Range` and `Accept-Ranges`. `dash.js` is vendored in `public/vendor/` and loaded from the same origin, so DevTools shows manifest and media traffic only to SourceTube. The AppImage, Windows ZIP and Docker image all ship the player, rules, font and icons locally.

## What Google still sees

- Sign-in and the optional transfer go to Google's OAuth and YouTube Data APIs.
- Video manifests and segments are fetched by the SourceTube server, not by your browser.
- Public metadata comes from the configured Invidious instance, never a YouTube Data API key.
- Watch history and Takeout files are never uploaded by SourceTube.
- The bundled adblock layer blocks tracker hosts and promoted metadata before they reach the page.

## Get your Google credentials

Google credentials are only needed for sign-in and transfers. Public search, trending and playback work without them. Creating the OAuth client is free; no billing account or credit card is needed for personal use.

See [OAUTH.md](OAUTH.md) for the full Google Cloud setup, `.env` values, where to put the file for each install method, the port-fallback redirect URIs, transfer scopes, Takeout steps and verification.

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

Set `SOURCETUBE_NO_BROWSER=1` to suppress the browser launch. Server output is written to `~/.local/share/sourcetube/server.log`. Imported metadata is stored in `~/.local/share/sourcetube/sourcetube.sqlite3`.

If your distro lacks FUSE, run:

```bash
./SourceTube-x86_64.AppImage --appimage-extract-and-run
```

Put your `.env` file in `~/.config/sourcetube/.env` or next to the AppImage before starting it. The AppImage ships the Rust server, the Rust native-player launcher, the new icon, the bundled font and the local DASH player. Native playback needs `mpv` or `libmpv.so.2` installed on the host; browser playback does not.

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

The ZIP contains native `sourcetube.exe` and `sourcetube-player.exe`, the icon, the font and the bundled DASH player. No Node.js install is needed. For low-RAM native playback, install mpv for Windows and keep `mpv.exe` in `PATH`.

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
- Use the left drawer for Home, Subscriptions, Library, Settings, Sign in and Transfer.
- Videos open in the bundled DASH player with native `<video>` controls and seeking.
- Use **Native player** on a watch page to hand the stream to mpv/libmpv. On web-client deployments, the player starts on the machine running the SourceTube server.
- Sponsor segments can be auto-skipped, manually skipped, or shown as highlight chapters in Settings.

## Build and test from source

```bash
cargo fmt --all
cargo test
./scripts/build-release.sh
```

Outputs land in `dist/`:

- `SourceTube-x86_64.AppImage`
- `SourceTube-Windows.zip`
- `SHA256SUMS`

Individual builds:

```bash
./scripts/build-appimage.sh
./scripts/build-windows.sh
```

## Privacy design

- No external JavaScript, font, filter list or media request at runtime.
- No analytics, telemetry, pixel, or ad network.
- Thumbnails, avatars, DASH manifests/segments and SponsorBlock are reverse-proxied behind a URL guard that denies HTTP, private hosts, trackers and unknown hosts. Thumbnail URLs come from the metadata API (FreeTube-style) and prefer the 16:9 medium image; browser requests go to `/api/image` and fall back to `/api/thumb/<id>/mqdefault.jpg`.
- Promo fields and bundled-channel rules are stripped from metadata; the adblock toggle is sent as a request header and stored locally.
- Strict CSP: `default-src 'self'`, `media-src 'self' blob:`, `connect-src 'self' blob:`, scripts and styles only from SourceTube.
- `Referrer-Policy: no-referrer`, `Cross-Origin-Opener-Policy: same-origin`.
- Session cookies are encrypted, HttpOnly, SameSite=Lax, and Secure on HTTPS. OAuth tokens are never written to SQLite.

## License

AGPL-3.0-only. See `LICENSE`. Vendored dash.js is BSD-licensed (`public/vendor/dash.all.min.js.LICENSE.txt`); the bundled Iosevka Nerd Font subset is OFL-licensed (`public/fonts/`).
