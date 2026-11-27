# Google OAuth setup for SourceTube

Google sign-in is server-side OAuth with PKCE. Public search and trending do not need a Google API key.

## Do I need to pay Google?

No. Creating a Google Cloud project, enabling YouTube Data API v3, and creating an OAuth client ID/secret are free. The default YouTube Data API quota is 10,000 units per day, and SourceTube's read-only subscription calls stay far below that for personal use. No billing account or credit card is needed unless you deliberately request a higher quota.

Google sign-in cannot be done without registering an OAuth client with Google. Google is the identity provider, so the client ID and secret must come from Google Cloud Console. Any app that signs you into a Google account has to go through that console.

If you skip OAuth entirely, SourceTube still works anonymously through the configured Invidious instance. Search, trending, video details, and related videos work. Sign-in and subscriptions stay unavailable.

## 1. Google Cloud Console

1. Create or select a project.
2. Enable **YouTube Data API v3**.
3. Configure the **OAuth consent screen**:
   - User type: **External**
   - Add yourself as a **Test user** while the app is in testing
   - Add scopes: `openid`, `email`, `profile`, `https://www.googleapis.com/auth/youtube.readonly`
4. Go to **Credentials → Create credentials → OAuth client ID**.
5. Application type: **Web application**.
6. Under **Authorized redirect URIs**, add the exact URI:

   ```text
   http://localhost:8787/api/auth/callback
   ```

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
```

Generate the session secret with:

```bash
openssl rand -base64 48
```

`SESSION_SECRET` must stay the same across restarts or signed-in cookies break.

`YOUTUBE_API_KEY` is optional. It is only a fallback for signed-in subscription metadata, not needed for public metadata or sign-in.

## 3. Where the `.env` goes

- **AppImage:** put it at `~/.config/sourcetube/.env`, or beside the AppImage.
- **Windows ZIP:** copy `app\.env.example` to `app\.env`.
- **Source/Docker:** put it at the repo root as `.env`.

## 4. Start and verify

Launch SourceTube, then check:

```bash
curl -s http://localhost:8787/api/config
```

OAuth is wired when you see:

```json
"hasOAuth": true
```

Then click **Sign in with Google** in the app. After callback, `/api/me` returns the signed-in profile and subscriptions become available.

## Port fallback gotcha

OAuth is bound to the redirect URI port. If `8787` is busy, the app falls back to `8788`, `8789`, etc., but Google will reject the sign-in unless that port is also authorized.

Pick one:

- Keep `8787` free, or
- Set `STRICT_PORT=1` so the app fails loudly instead of switching ports, or
- Add `http://localhost:8787/api/auth/callback` through `http://localhost:8806/api/auth/callback` as authorized redirect URIs in Google Cloud.

For a remote HTTPS deployment, set `APP_ORIGIN=https://your.domain` and `GOOGLE_REDIRECT_URI=https://your.domain/api/auth/callback`, and add that exact HTTPS URI to Google Cloud.
