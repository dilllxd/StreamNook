import type { ProviderId } from '../types/providers';

export type ProviderSendCapability = 'read_only' | 'sendable' | 'needs_login';

interface ProviderSendState {
  twitchUserId?: string | null;
  kickConnected: boolean;
  youtubeConnected: boolean;
  itzonCapability?: ProviderSendCapability;
}

export function canSendToProvider(provider: ProviderId, state: ProviderSendState): boolean {
  if (provider === 'twitch') return !!state.twitchUserId;
  if (provider === 'kick') return state.kickConnected;
  if (provider === 'youtube') return state.youtubeConnected;
  if (provider === 'itzon') return state.itzonCapability === 'sendable';
  return false;
}
