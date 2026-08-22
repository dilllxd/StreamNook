# Local Twitch + itzon prototype

This working tree is a local evaluation of StreamNook with itzon added as a
first-class playback and chat provider alongside Twitch. It is based on upstream
commit `e947641b9cc8b3703b68a4421c1f0e9c3074a705`, fetched on August 19, 2026,
when the upstream README said MIT but the repository had no root `LICENSE` file.

On August 21, 2026, the maintainer explicitly clarified in writing that this
pre-change copy keeps the terms represented as MIT at the time it was obtained.
They also said that new releases use PolyForm Noncommercial 1.0.0 plus additional
permissions: a fork adding another platform while retaining Twitch may be
published, the software itself may not be monetized or paywalled, donations are
allowed, and the upstream `LICENSE` must be retained.

That promised root license file is not yet visible on GitHub `main` as of the
latest checked commit, `f6046ed68242dd5cfeb7b2efaba06c03bc9fb948`; the README
still says `MIT. See LICENSE`, and GitHub's license API returns no detected
license. The maintainer's direct permission covers this fork, but no public
release should be cut under the new terms until the exact license text and its
additional permissions can be archived and reviewed.

## Local guardrails

- Work on the `feat/itzon-provider` branch.
- Preserve the upstream Git history and attribution.
- The user changed the local-only scope on August 22, 2026 and authorized a fork
  plus commits. A source feature branch may be published under the maintainer's
  direct permission; do not publish binaries or a release until the promised
  license text is available and the fork's product identity and update paths are
  separated from upstream.
- The `upstream` fetch remote points at the original StreamNook repository. Its
  push URL is intentionally set to an invalid host.

## Running locally

Install dependencies with `npm ci`, then provide the Twitch compile-time values
expected by the existing Rust application in the current shell:

```powershell
$env:TWITCH_APP_CLIENT_ID='<local value>'
$env:TWITCH_APP_CLIENT_SECRET='<local value>'
$env:TWITCH_WEB_CLIENT_ID='<local value>'
$env:TWITCH_ANDROID_CLIENT_ID='<local value>'
# Optional until itzon registers this fork's public OAuth application:
$env:ITZON_OAUTH_CLIENT_ID='<32-hex public client id>'
npm run tauri dev
```

Public itzon playback, discovery, chat reading, and 7TV emotes do not require a
login. In MultiNook, open Add Stream, select the itzon tab, and choose a live
public channel. Sending is available only after the user explicitly connects an
itzon account in Settings > Connections.

## Implemented in this prototype

- Provider-aware MultiNook slots and presets, with legacy slots defaulting to
  Twitch.
- Twitch and itzon discovery in one compact Add Stream picker.
- Official ITZON branding is bundled from the site's published organization
  assets (`/static/img/icon.png` and `/static/img/brand-o.png`) and is used by
  shared provider UI instead of an improvised letter mark. Discover filters and
  cards, login and connection surfaces, player/chat headers, MultiNook,
  MultiChat, and hosted-overlay provider tags all resolve through that artwork.
- Dynamic itzon channel and HLS-edge resolution.
- A local HLS relay that supports relative standard LL-HLS init, part, and media
  resources in addition to the existing Twitch path.
- Solo playback mirrors itzon's desktop RTT tiers instead of forcing every
  connection into low-latency mode: nearby LL origins use a 2.5/8-second live
  window, midrange connections use 5/12, and connections above 80 ms use 10/24.
  This keeps distant viewers from repeatedly draining an undersized buffer.
- The solo player header and sidebar share the same itzon avatar renderer, so a
  missing or failed avatar uses the site's deterministic color and username
  initial in both places.
- Per-stream itzon viewer heartbeats, stopped when a tile or the grid closes.
- Provider-aware metadata refresh, duplicate handling, persistence, and tile
  source markers.
- Provider-aware MultiNook chat selection. Twitch keeps its existing native IRC
  path; an itzon slot uses a separate `itzon:<channel>` identity, so equal names
  on different platforms cannot cross-wire their chat buffers.
- Dynamic itzon chat-edge discovery through `/api/live/ws-config`, restricted to
  trusted TLS endpoints under `itzon.tv`.
- Anonymous itzon IRC-over-WebSocket reading with capability negotiation, PING /
  PONG handling, bounded reconnect backoff, join backlog, stable message IDs,
  server timestamps, colors, avatars, subscription/partner metadata, replies,
  `/me` actions, and message redactions.
- Itzon chat identity matches the first-party viewer: uploaded avatars render
  from validated IRC metadata, avatar-less users get an initial fallback, and
  NAMES/META role state resolves to itzon's official owner, staff, bot,
  moderator, VIP, partner, subscriber, and unverified SVG badges.
- Incoming itzon messages and moderation redactions use StreamNook's shared
  provider chat bus and existing rich chat renderer.
- Global and channel 7TV sets are resolved using itzon's public channel metadata
  and 7TV's current Twitch-user lookup. Sets are cached per itzon channel for ten
  minutes, channel emotes override same-named globals, and the last good set is
  retained across a transient refresh failure.
- 7TV emotes render in backlog and live messages, including reply previews, and
  are available in StreamNook's autocomplete and emote picker. Each MultiNook
  channel keeps an independent set.
- A user-visible, site-owned WebView2 login flow is available from Connections.
  StreamNook never receives the user's password. The website's browser profile
  retains the site login; the Rust process keeps only the currently required
  cookie maps in memory and validates them against itzon's session endpoint.
- Authenticated IRC-over-WebSocket sending supports ordinary messages and reply
  tags, strips line breaks, enforces itzon's 400-character limit, and reports
  success only after the server echoes the matching message. NOTICE/404,
  disconnect, expiry, and timeout paths return a truthful failure.
- MultiChat's blended composer asks the native adapter for each itzon channel's
  current send capability instead of treating the provider as permanently
  read-only. A send-ready arcade source can be selected while another itzon
  source remains read-only, and provider sends reject an unconfirmed native
  outcome instead of silently reporting success.
- MultiChat live-following cards reuse the Twitch broadcaster id already present
  on the selected card, avoiding a second Helix lookup. If an authenticated IRC
  session is already established, a card can still join through that session
  after metadata OAuth expires. A cold or expired Twitch session remains visibly
  unavailable and requires login; arbitrary typed Twitch names still require
  Helix validation so misspellings do not become dead tabs.
- MultiChat serializes concurrent frontend bridge startup through a shared
  attempt gate. Every caller waits for the same attempt, and if the first
  provider fails, a later provider becomes the recovery owner instead of being
  left send-ready with no incoming events.
- Twitch composers now require a current OAuth identity and clearly switch to a
  login-required state after expiry instead of appearing to accept a no-op send.
- The solo provider composer no longer requires StreamNook's Twitch `currentUser`
  before submitting an authenticated itzon/Kick/YouTube message. Provider sends
  use their own capability and echo path; Twitch-only slash-command routing,
  duplicate-message bypass, secondary-account selection, and profile statistics
  remain scoped to Twitch.
- Channel-level sending remains disabled until the socket confirms the signed-in
  account's own JOIN. A guest nick, missing edge cookie, reconnect, unverified
  email, or channel ban keeps the composer read-only. Verification is learned
  from itzon's NAMES-prefix snapshot and subsequent `VERIFIED` events; readiness
  is never assumed merely because JOIN succeeded. Reply IDs are length- and
  character-checked before they can enter an IRCv3 tag.
- Connecting or disconnecting an account increments an auth revision so active
  itzon chat sockets reconnect with the new session. Expired sessions fall back
  to readable chat with the composer visibly disabled.

## Authentication and local security model

- A build containing `ITZON_OAUTH_CLIENT_ID` uses the supported public-client
  authorization-code flow with PKCE S256 and an ephemeral `127.0.0.1` callback.
  The authorization request contains no client secret and asks only for
  `identity chat api:read`.
- OAuth access and rotating refresh tokens are stored in the operating-system
  credential store. Refreshes are serialized, the newly rotated pair is saved
  before it replaces the in-memory pair, and active chat sockets reconnect when
  the credential changes. An `invalid_grant` or expired refresh grant removes
  the unusable local credential.
- OAuth IRC authentication sends the access token as the first `PASS` command,
  then registers with the authorized username. The implementation adapts the
  already-tested approach in the MIT-licensed local Itzon Chatterino fork.
- Until this fork receives its own registered client ID, Connect retains the
  site-owned WebView compatibility flow. StreamNook never sees the password;
  copied cookies remain process-memory only, while the dedicated WebView2
  profile may retain the website's own browser storage.
- Compatibility sessions are revision-guarded, periodically revalidated, and
  cleared on expiry. Disconnect removes both OAuth credentials and the dedicated
  compatibility profile.

## Remaining limitations

- Itzon staff must still register this fork as an OAuth public client. The PKCE,
  loopback callback, token exchange, encrypted persistence, rotation, restore,
  and IRC `PASS` paths are implemented; only the fork's 32-hex client ID is
  missing.
- The public API exposes a channel's followers but does not expose the signed-in
  viewer's native followed-channel list. The current sidebar and Following page
  use the internal `/api/main/v1/follows/mine` website endpoint. Production must
  either receive a supported `/me/following` equivalent from Itzon or explicitly
  retain this unstable compatibility dependency.
- Global discovery and playable media also depend on Itzon's internal live API.
  The public API has channel lookup and categories but no complete live catalog.
  Dynamic `hlsBase`, `wssBase`, and media URLs must continue to be discovered at
  runtime rather than hard-coded.
- Provider-specific system events beyond chat messages and redactions remain:
  pins, raids, whispers, membership changes, PART/QUIT/KICK/MODE state, and
  moderation/role command translation. Close codes 4400-4403, the eight-socket
  account limit, and the chat send-rate queue also need explicit UI behavior.
- Categories, category browsing, channel search beyond the live catalog,
  provider-specific profile cards, follow/unfollow/notification actions, live
  notifications, public clip browsing, private-stream locked states, and longer
  playback/reconnect soak tests remain viewer-facing gaps.
- Before release, the fork needs a distinct bundle identifier and deep-link
  scheme. Executable self-update, release, changelog, issue, announcement,
  component, and support URLs still point at upstream StreamNook. Upstream data
  services may be retained only where that dependency is intentional; the fork
  must never install an upstream executable over itself.
- Release packaging is not enabled in the current Tauri config. A distributable
  build still needs bundle configuration, CI ownership, checksums, code-signing
  decisions, and a clean-install/upgrade test matrix.
- Deterministic ordinary-message and reply sends are complete. Both targeted
  only the non-personal 24/7 `arcade` channel and were submitted under separate,
  explicit action-time approvals.
- The current persisted Twitch session can still populate cached discovery, but
  an attempted MultiChat add correctly reported that its token had expired and
  refresh failed. Twitch reauthentication is therefore required before another
  live mixed-source add/send validation; it was not initiated automatically.
- The MultiChat bridge recovery race has deterministic unit coverage, but the
  exact first-source-Twitch-fails/second-source-itzon-recovers sequence still
  needs a fresh runtime replay after reauthentication or in a disposable window.

## Validation performed

- Existing provider tests cover trusted URL construction, IRCv3 tag escaping,
  captured live-message normalization, reply enrichment from the join backlog,
  redaction mapping, and bounded reconnect backoff. New focused tests cover the
  current 7TV response shape, plain-text fallback, two-channel cache isolation,
  authenticated cookie rotation and deletion, stable cookie headers, echo
  matching, reply-tag injection rejection, and account/verification/ban send
  readiness.
- The live-network 7TV smoke test passes against steqian and tokenizes its
  current channel set.
- `npm run build` passes (TypeScript plus the production Vite build), and focused
  frontend lint reports zero errors.
- The branding build emits both bundled ITZON image variants and the rebuilt
  Windows app was visually checked at compact tab/card/header sizes and the
  larger login size.
- A clean non-incremental Rust standalone build passes on Windows.
- The focused itzon Rust run reports 15 passing and one optional live test
  ignored. The full Rust suite reports 174 passing, zero failures, and three
  ignored tests. This includes the single-token itzon IRC normalization
  regression, the repaired unknown-emoji-shortcode fallback, and a session
  expiry guard proving a stale validation cannot clear a replacement login.
- The full discovered frontend test surface passes: 46 tests, zero failures.
  Six specifically cover this milestone. Two cover the shared-attempt gate
  (concurrent callers share one attempt; rejection clears for recovery), and
  three cover provider send independence (Twitch requires its OAuth identity,
  while Itzon, Kick, and YouTube use their own authenticated state). One locks
  itzon's near, midrange, and far desktop HLS latency tiers.
- The current frontend production build passes. Targeted ESLint reports no
  errors in the provider/chat changes. Full-project ESLint was also run and
  remains nonzero with 26 errors in 13 pre-existing unrelated files; the one
  error introduced by the new Itzon avatar fallback was fixed.
- The rebuilt Tauri app was exercised against public live itzon channels.
  steqian's video and chat backlog rendered together; two simultaneous itzon
  sources held two live chat sockets; and the switcher moved between independent
  buffers and 7TV sets. At validation time steqian exposed 236 picker entries
  (193 standard) while Sulvane exposed 46 (43 standard), and switching back
  restored steqian's set. The disconnected composer correctly remained disabled
  with a Connect-account explanation. The rebuilt hardened app was exercised a
  second time and retained that safe disconnected state while playback and
  anonymous chat continued.
- Solo discovery rendered the current itzon catalog and thumbnails, and live
  itzon playback was visually confirmed. A mixed MultiNook grid recovered three
  initially stale tiles through the built-in resynchronization action and then
  played two itzon sources beside Twitch while keeping provider-specific chat
  buffers isolated.
- A read-only observer attached to StreamNook's local chat bridge and confirmed
  live `itzon:arcade`, `itzon:grandpachang`, and Twitch channel metadata on the
  same bus. A fresh MultiChat window then rendered arcade both in tabbed and
  blended layouts; after the blended-capability fix, arcade was send-ready while
  the offline Sulvane source remained read-only. No chat input was staged or sent
  during these checks.
- The latest rebuilt executable again restored the site-owned itzon session,
  played the 24/7 arcade stream, loaded its live Race #333 chat, and moved the
  solo composer from handshake read-only to send-ready after the authenticated
  JOIN/NAMES state completed. Its restored blended MultiChat then received the
  same live arcade event and showed `Send to 1 connected chat` while Sulvane
  remained read-only. No chat input was staged or sent.
- On the final build, StreamNook visibly reported the expired Twitch session and
  left Twitch logged out while independently restoring Itzon. Arcade playback
  and live Race #336 chat loaded, and the solo composer reached `Send a message`
  without a Twitch `currentUser`, directly exercising the provider-independent
  submit fix. The empty composer was not focused and no message was submitted.
- With explicit action-time approval, the final build submitted exactly one
  ordinary message to `arcade`: `StreamNook local echo test
  2026-08-20T14-14-08Z`. The composer cleared, the native send call completed,
  and the exact text appeared in the live feed under the authenticated account,
  proving echo-backed success rather than optimistic-only rendering. No retry or
  reply was sent.
- With a second, separate action-time approval, the same build submitted exactly
  one reply to that message in `arcade`: `StreamNook local reply test`. The
  native feed rendered it under `dylan` with a parent preview containing the
  exact original marker, while the reply banner closed and the composer cleared.
  A passive follow-up observation confirmed both reply and parent remained
  present. This proves reply-target propagation and echo-backed confirmation;
  no duplicate or additional message was sent.
- A fresh close/reopen cycle then exercised cold restoration without sending.
  Twitch surfaced its expected expired-session warning while Itzon discovery
  independently repopulated four live channels and their thumbnails. Opening
  `arcade` restored playing video, chat backlog, advancing live messages, the
  prior echoed message, avatars and official role badges, and a send-ready Itzon
  composer after the authenticated handshake. The in-app reload control also
  recovered playback/chat and advanced the feed from Race #339 to Race #340.
- The final local validation rerun passed all 44 discovered frontend tests, the
  production TypeScript/Vite build, all 182 discovered Rust tests (179 passed
  and three intentionally ignored), and `cargo fmt --check`. Targeted ESLint on
  the changed frontend surface reported zero errors and 96 warnings. The
  optional live Itzon + 7TV smoke test was run explicitly and passed 1/1.
- The apparently blank Sulvane thumbnail was compared with its opened player;
  the upstream stream itself was serving a matching white frame, so this is not
  a failed thumbnail element or relay decode error.
