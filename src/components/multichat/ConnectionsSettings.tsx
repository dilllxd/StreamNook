// Connections — the shared account manager used by the main app and MultiChat.
//
// Lists every provider with its connection status and a connect/disconnect
// action. Twitch is the app's native account (managed in the main app), Kick is
// wired to the OAuth flow, and the rest show as "coming soon" until their adapters
// ship. This scales as platforms light up — no more per-composer connect chips
// being the only way in.

import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { ProviderLogo } from '../ProviderLogo';
import { PROVIDERS, PROVIDER_IDS, type ProviderId } from '../../types/providers';
import { useAppStore } from '../../stores/AppStore';
import { Logger } from '../../utils/logger';

type Status = 'native' | 'connected' | 'disconnected' | 'soon';
type ItzonAuthMethod = 'oauth' | 'website-session' | 'oauth-ready' | 'registration-pending';

const DOT: Record<Status, string> = {
  native: '#53fc18',
  connected: '#53fc18',
  disconnected: 'rgba(255,255,255,0.25)',
  soon: 'rgba(245,158,11,0.7)',
};

const LABEL: Record<Status, string> = {
  native: 'Connected · managed in the main app',
  connected: 'Connected',
  disconnected: 'Not connected',
  soon: 'Coming soon',
};

export default function ConnectionsSettings() {
  const currentUser = useAppStore((s) => s.currentUser);
  const loginToTwitch = useAppStore((s) => s.loginToTwitch);
  const twitchBusy = useAppStore((s) => s.isLoading);
  const [kickConnected, setKickConnected] = useState(false);
  const [kickName, setKickName] = useState<string | null>(null);
  const [kickBusy, setKickBusy] = useState(false);
  const [youtubeConnected, setYoutubeConnected] = useState(false);
  const [youtubeName, setYoutubeName] = useState<string | null>(null);
  const [youtubeBusy, setYoutubeBusy] = useState(false);
  const [itzonConnected, setItzonConnected] = useState(false);
  const [itzonName, setItzonName] = useState<string | null>(null);
  const [itzonBusy, setItzonBusy] = useState(false);
  const [itzonAuthMethod, setItzonAuthMethod] = useState<ItzonAuthMethod>('registration-pending');

  useEffect(() => {
    let active = true;
    const check = async () => {
      try {
        const connected = await invoke<boolean>('itzon_restore_session');
        if (!active) return;
        setItzonConnected(connected);
        setItzonName(connected ? await invoke<string | null>('itzon_account_name') : null);
        setItzonAuthMethod(await invoke<ItzonAuthMethod>('itzon_auth_method'));
      } catch {
        /* ignore */
      }
      try {
        const c = await invoke<boolean>('kick_is_connected');
        if (!active) return;
        setKickConnected(c);
        if (c) {
          const n = await invoke<string | null>('kick_account_name');
          if (active) setKickName(n);
        } else {
          setKickName(null);
        }
      } catch {
        /* ignore */
      }
      try {
        const y = await invoke<boolean>('youtube_is_connected');
        if (!active) return;
        setYoutubeConnected(y);
        if (y) {
          const n = await invoke<string | null>('youtube_account_name');
          if (active) setYoutubeName(n);
        } else {
          setYoutubeName(null);
        }
      } catch {
        /* ignore */
      }
    };
    void check();
    const t = setInterval(() => void check(), 5000);
    return () => {
      active = false;
      clearInterval(t);
    };
  }, []);

  const connectKick = useCallback(() => {
    setKickBusy(true);
    void invoke('kick_connect')
      .then(() => setKickConnected(true))
      .catch((e) => Logger.warn('[Kick] connect failed:', e))
      .finally(() => setKickBusy(false));
  }, []);

  const connectItzon = useCallback(() => {
    setItzonBusy(true);
    void invoke('itzon_connect')
      .then(async () => {
        setItzonConnected(true);
        setItzonName(await invoke<string | null>('itzon_account_name'));
        setItzonAuthMethod(await invoke<ItzonAuthMethod>('itzon_auth_method'));
        await emit('itzon-connection-changed');
      })
      .catch((error) => Logger.warn('[itzon] connect failed:', error))
      .finally(() => setItzonBusy(false));
  }, []);

  const disconnectItzon = useCallback(() => {
    void invoke('itzon_disconnect')
      .then(async () => {
        setItzonConnected(false);
        setItzonName(null);
        setItzonAuthMethod(await invoke<ItzonAuthMethod>('itzon_auth_method'));
        await emit('itzon-connection-changed');
      })
      .catch(() => {});
  }, []);

  const disconnectKick = useCallback(() => {
    void invoke('kick_disconnect')
      .then(() => setKickConnected(false))
      .catch(() => {});
  }, []);

  const connectYoutube = useCallback(() => {
    setYoutubeBusy(true);
    void invoke('youtube_connect')
      .then(() => setYoutubeConnected(true))
      .catch((e) => Logger.warn('[YouTube] connect failed:', e))
      .finally(() => setYoutubeBusy(false));
  }, []);

  const disconnectYoutube = useCallback(() => {
    void invoke('youtube_disconnect')
      .then(() => setYoutubeConnected(false))
      .catch(() => {});
  }, []);

  const statusFor = (p: ProviderId): Status => {
    if (p === 'twitch') return currentUser ? 'native' : 'disconnected';
    if (p === 'itzon') return itzonConnected ? 'connected' : 'disconnected';
    if (p === 'kick') return kickConnected ? 'connected' : 'disconnected';
    if (p === 'youtube') return youtubeConnected ? 'connected' : 'disconnected';
    return PROVIDERS[p].enabled ? 'disconnected' : 'soon';
  };

  // Subtitle, naming the connected account where we know it.
  const subtitleFor = (p: ProviderId, status: Status): string => {
    if (p === 'twitch') {
      if (currentUser?.display_name) return `Connected as ${currentUser.display_name}`;
      return status === 'native' ? LABEL.native : LABEL.disconnected;
    }
    if (p === 'kick' && status === 'connected') {
      return kickName ? `Connected as ${kickName}` : 'Connected';
    }
    if (p === 'itzon' && status === 'connected') {
      const method = itzonAuthMethod === 'oauth' ? 'OAuth' : 'compatibility session';
      return `${itzonName ? `Connected as ${itzonName}` : 'Connected'} · ${method}`;
    }
    if (p === 'itzon' && itzonAuthMethod === 'oauth-ready') {
      return 'OAuth ready · not connected';
    }
    if (p === 'itzon' && itzonAuthMethod === 'registration-pending') {
      return 'Compatibility login · OAuth registration pending';
    }
    if (p === 'youtube' && status === 'connected') {
      return youtubeName ? `Connected as ${youtubeName}` : 'Connected';
    }
    return LABEL[status];
  };

  return (
    <div className="space-y-4">
      <p className="text-xs text-textSecondary">
        Connect the accounts StreamNook uses for followed channels, playback, and chat. The same connections are
        available in MultiChat.
      </p>

      <div className="hairline-y overflow-hidden rounded-lg border border-borderSubtle">
        {PROVIDER_IDS.map((p) => {
          const meta = PROVIDERS[p];
          const status = statusFor(p);
          return (
            <div
              key={p}
              className="flex items-center gap-3 px-3 py-3"
              style={{ opacity: status === 'soon' ? 0.6 : 1 }}
            >
              <ProviderLogo provider={p} size={22} />
              <div className="min-w-0 flex-1">
                <div className="text-sm font-medium text-textPrimary">{meta.label}</div>
                <div className="mt-0.5 flex items-center gap-1.5 text-xs text-textSecondary">
                  <span
                    className="inline-block h-1.5 w-1.5 rounded-full"
                    style={{ backgroundColor: DOT[status] }}
                  />
                  {subtitleFor(p, status)}
                </div>
              </div>

              {/* Account-backed providers connect through their own consent flow. */}
              {p === 'twitch' && !currentUser && (
                <button
                  type="button"
                  onClick={() => void loginToTwitch()}
                  disabled={twitchBusy}
                  className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-semibold text-[#a970ff] transition-colors disabled:opacity-60"
                >
                  {twitchBusy ? 'Waiting for sign-in…' : 'Connect'}
                </button>
              )}
              {p === 'itzon' &&
                (itzonConnected ? (
                  <button
                    type="button"
                    onClick={disconnectItzon}
                    className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-medium text-textSecondary transition-colors hover:text-red-400"
                  >
                    Disconnect
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={connectItzon}
                    disabled={itzonBusy}
                    className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-semibold transition-colors disabled:opacity-60"
                    style={{ color: PROVIDERS.itzon.color }}
                  >
                    {itzonBusy ? 'Waiting for sign-in…' : 'Connect'}
                  </button>
                ))}
              {p === 'kick' &&
                (kickConnected ? (
                  <button
                    type="button"
                    onClick={disconnectKick}
                    className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-medium text-textSecondary transition-colors hover:text-red-400"
                  >
                    Disconnect
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={connectKick}
                    disabled={kickBusy}
                    className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-semibold transition-colors disabled:opacity-60"
                    style={{ color: '#53fc18' }}
                  >
                    {kickBusy ? 'Connecting…' : 'Connect'}
                  </button>
                ))}
              {p === 'youtube' &&
                (youtubeConnected ? (
                  <button
                    type="button"
                    onClick={disconnectYoutube}
                    className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-medium text-textSecondary transition-colors hover:text-red-400"
                  >
                    Disconnect
                  </button>
                ) : (
                  <button
                    type="button"
                    onClick={connectYoutube}
                    disabled={youtubeBusy}
                    className="glass-button-secondary shrink-0 px-3 py-1 text-xs font-semibold transition-colors disabled:opacity-60"
                    style={{ color: '#ff4d4d' }}
                  >
                    {youtubeBusy ? 'Connecting…' : 'Connect'}
                  </button>
                ))}

              {status === 'soon' && (
                <span className="shrink-0 rounded px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-amber-500/80">
                  Soon
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
