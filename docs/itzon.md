# Itzon support

This fork adds Itzon through StreamNook's provider interfaces while retaining
the upstream Twitch, Kick, YouTube, and TikTok integrations.

## Included

- Itzon OAuth 2.0 Authorization Code login with PKCE
- Following, directory, categories, search, live status, and thumbnails
- HLS playback, quality selection, viewer heartbeat, and MultiNook tiles
- Live chat, sending, replies, roles, avatars, and Itzon/7TV emotes
- MultiChat, blended chat, OBS overlay sources, and provider-aware popouts
- The official Itzon logo and the site's deterministic initial avatar fallback

## Authentication

Desktop builds need an Itzon public client ID and no client secret:

```powershell
$env:ITZON_OAUTH_CLIENT_ID='<public client id>'
```

The current preview temporarily uses the public client ID issued to the local
Chatterino fork. Replace it with a StreamNook-specific registration before a
stable release. Tokens are stored in the operating-system credential store.

The site-session flow remains as a compatibility fallback for endpoints that
do not yet accept OAuth bearer tokens.

## Build configuration

The upstream Twitch and Kick integrations also need their application
credentials at compile time. See `.github/workflows/itzon-preview.yml` for the
exact variable names. Do not commit client secrets.

```powershell
npm ci
npm run build
$tests = Get-ChildItem src -Recurse -Filter '*.test.ts' | Select-Object -ExpandProperty FullName
npx --yes tsx@4.23.12 --test $tests
cargo test --manifest-path src-tauri/Cargo.toml
npm run tauri build -- --no-bundle
```

Tagged builds publish `StreamNook.7z`, a portable ZIP, checksums, and a fork-owned
update manifest. The app only checks this fork's release channel, so an upstream
StreamNook update cannot silently replace the Itzon build.

## Known gaps

- Itzon does not expose equivalents for every Twitch feature. Raids, clips,
  whispers, channel points, and Twitch-specific moderation surfaces remain
  platform-specific.
- OAuth-backed following currently falls back to the validated site session if
  the website endpoint rejects a bearer token.
- Long-running playback, reconnects, and authenticated chat still need release
  soak testing against live Itzon channels.

## License

This branch is based on upstream StreamNook v8.5.1 and remains under the
PolyForm Noncommercial License 1.0.0 plus the additional permissions in the
root `LICENSE`. Keep that file with every source or binary distribution.
