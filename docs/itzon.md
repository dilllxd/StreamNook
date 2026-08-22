# itzon support

This fork adds itzon as a first-class provider without removing Twitch. Public
streams support discovery, thumbnails, playback, anonymous chat, avatars, role
badges, and 7TV emotes. Connected accounts add following and chat sending.

## Authentication

itzon uses OAuth 2.0 Authorization Code with PKCE. Desktop builds need a public
client ID but no client secret:

```powershell
$env:ITZON_OAUTH_CLIENT_ID='<32-hex public client id>'
```

Until this fork receives its own registration, CI uses the public client ID
issued to the local Chatterino fork. Replace it before a stable release.

The existing site-session compatibility flow remains available. OAuth tokens
are stored in the operating-system credential store; compatibility cookies are
kept in memory and validated against itzon before use.

## Build

```powershell
npm ci
npm run build
npx tsx --test "src/**/*.test.ts"
npm run tauri build -- --no-bundle
```

The Rust build also expects a Twitch client ID. Twitch authentication uses the
public-client Device Code Flow, including token refresh, linked accounts, and
moderator-room consent, so no client secret is embedded in the application. The
fork workflow reads `TWITCH_CLIENT_ID` from a GitHub Actions repository
variable. It produces an unsigned portable Windows executable, ZIP and 7z
archives, manifests, and SHA-256 checksums. Pushing an `itzon-v*` tag creates a
GitHub prerelease after verifying that the tag matches the application version
and the Twitch client ID exists.

## Current gaps

- itzon does not expose every Twitch feature. Pins, raids, whispers, role and
  moderation commands, clips, and provider-specific notifications remain.
- Following and global discovery still depend on website endpoints because the
  public API does not yet expose equivalent endpoints.
- Windows builds are unsigned and may trigger SmartScreen.
- Release soak testing should cover long-running playback, reconnects, mixed
  Twitch/itzon grids, and authenticated chat before a stable release.

## License provenance

This fork is based on upstream commit
`e947641b9cc8b3703b68a4421c1f0e9c3074a705`, obtained while the upstream README
identified StreamNook as MIT-licensed. The maintainer later confirmed in writing
that the licensing change would not apply retroactively to copies obtained while
the README stated MIT. See the root `LICENSE` file.
