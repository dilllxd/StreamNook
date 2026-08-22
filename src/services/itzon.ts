import { invoke } from '@tauri-apps/api/core';
import type { TwitchStream } from '../types';

export interface ItzonStream {
  username: string;
  viewers: number;
  thumbnail?: string;
  hlsBase?: string;
  mediaBase?: string;
  wssBase?: string;
  language?: string;
  title?: string;
  category?: string;
  categoryId?: number;
}

export interface ItzonCategory {
  id: number;
  name: string;
  liveStreamCount: number;
  viewerCount: number;
  imageUrl?: string;
}

export interface ItzonExplore {
  streams: ItzonStream[];
  categories: ItzonCategory[];
  hlsBase?: string;
  mediaBase?: string;
  wssBase?: string;
}

export interface ItzonChannel {
  username: string;
  live: boolean;
  locked: boolean;
  partner: boolean;
  title?: string;
  category?: string;
  categoryId?: number;
  language?: string;
  pointsName?: string;
  emoteTwitchId?: string;
  thumbnail?: string;
  hlsBase?: string;
  mediaBase?: string;
  wssBase?: string;
}

export const itzonAvatarUrl = (username: string): string =>
  `https://itzon.tv/api/live/profile/${encodeURIComponent(username.toLowerCase())}/avatar`;

function trustedItzonUrl(value: string, base = 'https://itzon.tv'): string | null {
  try {
    const url = new URL(value, base);
    if (
      url.protocol === 'https:' &&
      (url.hostname === 'itzon.tv' || url.hostname.endsWith('.itzon.tv'))
    ) {
      return url.toString();
    }
  } catch {
    // Invalid platform metadata falls through to the avatar placeholder.
  }
  return null;
}

/** Match itzon Browse's live-preview URL, including its one-minute cache key. */
export function itzonLiveThumbnailUrl(
  username: string,
  mediaBase?: string,
  customThumbnail?: string,
): string {
  if (customThumbnail) {
    const custom = trustedItzonUrl(customThumbnail);
    if (custom) return custom;
  }

  if (mediaBase) {
    const base = trustedItzonUrl(mediaBase);
    if (base) {
      const minute = Math.floor(Date.now() / 60_000);
      return `${base.replace(/\/+$/, '')}/thumb/${encodeURIComponent(username.toLowerCase())}.jpg?t=${minute}`;
    }
  }

  return itzonAvatarUrl(username);
}

export const getItzonExplore = (): Promise<ItzonExplore> => invoke('get_itzon_explore');

export const getItzonChannel = (username: string): Promise<ItzonChannel> =>
  invoke('get_itzon_channel', { username });

export const getItzonFollowing = (): Promise<string[]> => invoke('get_itzon_following');

/** Adapt itzon's public live-directory shape to the legacy stream-card model. */
export function itzonStreamToTwitchStream(stream: ItzonStream): TwitchStream {
  const username = stream.username.trim().toLowerCase();
  const avatar = itzonAvatarUrl(username);
  return {
    provider: 'itzon',
    id: `itzon:${username}`,
    user_id: `itzon:${username}`,
    user_login: username,
    user_name: stream.username,
    title: stream.title?.trim() || `${stream.username} live on itzon`,
    viewer_count: stream.viewers ?? 0,
    game_id: stream.categoryId != null ? String(stream.categoryId) : '',
    game_name: stream.category?.trim() || 'Live on itzon',
    thumbnail_url: itzonLiveThumbnailUrl(username, stream.mediaBase, stream.thumbnail),
    profile_image_url: avatar,
    started_at: '',
    is_live: true,
    tags: stream.language ? [stream.language] : [],
  };
}

export function partitionItzonStreams(
  streams: TwitchStream[],
  following: Iterable<string>,
): { followed: TwitchStream[]; recommended: TwitchStream[] } {
  const followedNames = new Set(
    Array.from(following, username => username.trim().toLowerCase()).filter(Boolean),
  );
  return {
    followed: streams.filter(stream => followedNames.has(stream.user_login.toLowerCase())),
    recommended: streams.filter(stream => !followedNames.has(stream.user_login.toLowerCase())),
  };
}
