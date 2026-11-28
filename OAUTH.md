# Google OAuth and transfer setup for SourceTube

Google sign-in is server-side OAuth with PKCE. Public search, trending and playback do not need a Google API key or an account.

## Do I need to pay Google?

No. Creating a Google Cloud project, enabling YouTube Data API v3, and creating an OAuth client ID/secret are free. The default YouTube Data API quota is 10,000 units per day, and SourceTube's read-only calls stay far below that for personal use. No billing account or credit card is needed unless you deliberately request a higher quota.

Google sign-in cannot be done without registering an OAuth client with Google. Google is the identity provider, so the client ID and secret must come from Google Cloud Console.

If you skip OAuth entirely, SourceTube still works anonymously through the configured Invidious instance. Search, trending, video details, related videos and bundled DASH playback all work. Sign-in and transfers stay unavailable.

## 1. Create the free Google OAuth client

1. Create or select a project at <https://console.cloud.google.com/>.
2. Enable **YouTube Data API v3**.
3. Configure the **OAuth consent screen**:
   - User type: **External**
   - Add yourself as a **Test user** while the app is in testing
   - Add scopes: `openid`, `email`, `profile`, `https://www.googleapis.com/auth/youtube.readonly`
4. Go to **Credentials → Create credentials → OAuth client ID**.
5. Application type: **Web application**.
6. Under **Authorized redirect URIs**, add every port SourceTube may select (8787 through 8806):

   ```text
   http://localhost:8787/api/auth/callback
   http://localhost:8788/api/auth/callback
   http://localhost:8789/api/auth/callback
   http://localhost:8790/api/auth/callback
   http://localhost:8791/api/auth/callback
   http://localhost:8792/api/auth/callback
   http://localhost:8793/api/auth/callback
   http://localhost:8794/api/auth/callback
   http://localhost:8795/api/auth/callback
   http://localhost:8796/api/auth/callback
   http://localhost:8797/api/auth/callback
   http://localhost:8798/api/auth/callback
   http://localhost:8799/api/auth/callback
   http://localhost:8800/api/auth/callback
   http://localhost:8801/api/auth/callback
   http://localhost:8802/api/auth/callback
   http://localhost:8803/api/auth/callback
   http://localhost:8804/api/auth/callback
   http://localhost:8805/api/auth/callback
   http://localhost:8806/api/auth/callback
   ```

   SourceTube tries 20 consecutive ports starting at `PORT` (default 8787), and writes the chosen port to the runtime port file. If you only add 8787, set `STRICT_PORT=1` so the app fails loudly instead of falling back.

7. Copy the **Client ID** and **Client secret**.

## 2. Configure SourceTube

For source or Docker, copy `.env.example` to `.env` in the repo root and fill it:

```dotenv
PORT=8787
APP_ORIGIN=http://localhost:8787

GOOGLE_CLIENT_ID=your-client-id.apps.googleusercontent.com
GOOGLE_CLIENT_SECRET=your-client-secret
GOOGLE_REDIRECT_URI=http://localhost:8787/api/auth/callback

SESSION_SECRET=paste-a-long-random-string-here
SOURCETUBE_DATA=/path/to/private/data/directory
```

Generate the session secret with:

```bash
openssl rand -base64 48
```

`SESSION_SECRET` must stay the same across restarts or signed-in cookies break. OAuth tokens are AES-256-GCM sealed into HttpOnly session cookies and are never written to the SQLite store.

`YOUTUBE_API_KEY` is optional. It is only a fallback for signed-in subscription metadata, not needed for public metadata, sign-in or playback.

## 3. Where the `.env` goes

- **AppImage:** put it at `~/.config/sourcetube/.env`, or beside the AppImage.
- **Windows ZIP:** copy `app\.env.example` to `app\.env`.
- **Source/Docker:** put it at the repo root as `.env`.

## 4. Sign in and verify

Launch SourceTube, then check:

```bash
curl -s http://localhost:8787/api/config
```

OAuth is wired when you see:

```json
"hasOAuth": true
```

Then open **Sign in**, click **Continue with Google**, and approve. `/api/me` returns the connected profile with the read-only scope. `/api/transfer/status` reports the wizard state. Signing out clears the session cookie and the in-memory transfer task; imported metadata remains local.

## 5. Transfer scope and imports

The requested scope is `https://www.googleapis.com/auth/youtube.readonly`. It allows read-only access to:

- subscriptions via `subscriptions.list` and channel metadata via `channels.list`
- playlists via `playlists.list`
- playlist items via `playlistItems.list` (including Watch Later where available)
- liked videos via `videos.list` with `myRating=like`

It cannot upload, edit, delete or comment. Public browsing never uses this token. Imports are deduplicated by video/channel/playlist ID and stored under `SOURCETUBE_DATA/sourcetube.sqlite3` with bounded size.

## 6. Google Takeout import

1. Visit <https://takeout.google.com/>.
2. Select only **YouTube and YouTube Music** (or just the YouTube data you want).
3. Choose JSON format, all available YouTube history, likes, comments, playlists and subscriptions.
4. Download and extract the archive. You do not need to upload it anywhere else.
5. In SourceTube open **Transfer** and choose the exported `.json` files. The local server parses watch history, comments, likes, playlists and subscriptions when present, merges them with API imports, and deduplicates by ID.

Takeout data never leaves your machine: the browser posts it only to `/api/transfer/takeout` on the SourceTube origin, and SourceTube does not forward it to Google or any third party.

## Port fallback gotcha

OAuth is bound to the redirect URI port. If `8787` is busy, the app falls back to `8788`, `8789`, etc., and Google will reject sign-in unless that port is also authorized. Add the full 8787..8806 list above, keep 8787 free, or set `STRICT_PORT=1`.

For a remote HTTPS deployment, set `APP_ORIGIN=https://your.domain` and `GOOGLE_REDIRECT_URI=https://your.domain/api/auth/callback`, and add that exact HTTPS URI to Google Cloud.
