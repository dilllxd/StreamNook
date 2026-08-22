import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { TwitchStream } from '../../types';
import { useAppStore } from '../../stores/AppStore';
import { Logger } from '../../utils/logger';
import type { ProviderId } from '../../types/providers';

/** Raw shape returned by the `search_channels` Tauri command. The endpoint is
 *  loosely typed (different Twitch surfaces fill different fields), so every key
 *  is optional and normalized into a ChannelItem before rendering. */
export interface ChannelSearchResult {
  id?: string;
  user_id?: string;
  user_login?: string;
  broadcaster_login?: string;
  user_name?: string;
  display_name?: string;
  thumbnail_url?: string;
  is_live?: boolean;
  game_name?: string;
  profile_image_url?: string;
}

/** Normalized shape so live-follows and Twitch search results render through one row. */
export interface ChannelItem {
  id: string;
  login: string;
  displayName: string;
  avatarUrl?: string;
  isLive: boolean;
  gameName?: string;
  source: 'following' | 'search';
  provider?: ProviderId;
  viewerCount?: number;
}

export const DEFAULT_AVATAR =
  'https://static-cdn.jtvnw.net/user-default-pictures-uv/75305d54-c7cc-40d1-bb9c-91c46bf27829-profile_image-70x70.png';

/** Followed-streams thumbnails are stream previews carrying {width}x{height} placeholders that
 *  won't load as-is, so prefer a real profile image and only fall back to a sized preview. */
export function resolveAvatar(profileImageUrl?: string, thumbnailUrl?: string): string | undefined {
  if (profileImageUrl) return profileImageUrl;
  if (thumbnailUrl) return thumbnailUrl.replace('{width}', '150').replace('{height}', '150');
  return undefined;
}

export function streamToItem(s: TwitchStream): ChannelItem {
  return {
    id: s.user_id,
    login: s.user_login,
    displayName: s.user_name || s.user_login,
    avatarUrl: resolveAvatar(s.profile_image_url, s.thumbnail_url),
    isLive: true, // followed-streams endpoint only returns live channels
    gameName: s.game_name,
    source: 'following',
    provider: 'twitch',
  };
}

export function resultToItem(r: ChannelSearchResult): ChannelItem {
  const login = r.user_login || r.broadcaster_login || '';
  return {
    id: r.user_id || r.id || login,
    login,
    displayName: r.user_name || r.display_name || login,
    avatarUrl: resolveAvatar(r.profile_image_url, r.thumbnail_url),
    isLive: !!r.is_live,
    gameName: r.game_name,
    source: 'search',
    provider: 'twitch',
  };
}

/**
 * Shared channel-finder logic backing both the toolbar's "Add Stream" search and
 * the preset editor's channel picker. Owns: the query, the debounced Twitch
 * search, the merged "live following + all channels" lists (minus any logins the
 * caller excludes), and the keyboard-highlight index with scroll-into-view.
 *
 * The caller owns panel open/close, focus, and what happens on select. This
 * hook is purely the data + navigation layer so the two surfaces stay identical.
 */
export function useChannelSearch(excludeLogins: Set<string>, enabled = true) {
  const followedStreams = useAppStore((s) => s.followedStreams);
  const loadFollowedStreams = useAppStore((s) => s.loadFollowedStreams);

  const [searchInput, setSearchInput] = useState('');
  const [isSearching, setIsSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<ChannelSearchResult[]>([]);
  const [highlightIndex, setHighlightIndex] = useState(0);

  const listRef = useRef<HTMLDivElement>(null);
  const searchTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const query = searchInput.trim().toLowerCase();

  // Online following, instantly filtered against the typed query (no network round-trip).
  const followingItems = useMemo(() => {
    if (!enabled) return [];
    const items = followedStreams
      .map(streamToItem)
      .filter((it) => !excludeLogins.has(it.login.toLowerCase()));
    if (!query) return items;
    return items.filter(
      (it) =>
        it.login.toLowerCase().includes(query) ||
        it.displayName.toLowerCase().includes(query) ||
        (it.gameName || '').toLowerCase().includes(query),
    );
  }, [enabled, followedStreams, excludeLogins, query]);

  // Twitch search results, minus anything excluded or already shown as a live follow.
  const searchItems = useMemo(() => {
    if (!enabled) return [];
    const followingLogins = new Set(followingItems.map((it) => it.login.toLowerCase()));
    return searchResults
      .map(resultToItem)
      .filter(
        (it) =>
          it.login &&
          !excludeLogins.has(it.login.toLowerCase()) &&
          !followingLogins.has(it.login.toLowerCase()),
      );
  }, [enabled, searchResults, followingItems, excludeLogins]);

  // Flat list backing keyboard navigation (following first, then search).
  const visibleItems = useMemo(() => [...followingItems, ...searchItems], [followingItems, searchItems]);

  // Reset the highlight whenever the result set changes shape.
  useEffect(() => {
    setHighlightIndex(0);
  }, [query, followingItems.length, searchItems.length]);

  // Keep the highlighted row scrolled into view during keyboard navigation.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-idx="${highlightIndex}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  }, [highlightIndex]);

  // Debounced Twitch search, only fires while there's a query; the live list above stays instant.
  useEffect(() => {
    if (!enabled) {
      setSearchResults([]);
      setIsSearching(false);
      if (searchTimeoutRef.current) clearTimeout(searchTimeoutRef.current);
      return;
    }
    if (!query) {
      setSearchResults([]);
      setIsSearching(false);
      if (searchTimeoutRef.current) clearTimeout(searchTimeoutRef.current);
      return;
    }

    if (searchTimeoutRef.current) clearTimeout(searchTimeoutRef.current);

    setIsSearching(true);
    searchTimeoutRef.current = setTimeout(async () => {
      try {
        const results = (await invoke('search_channels', { query: searchInput.trim() })) as ChannelSearchResult[];
        setSearchResults(results.slice(0, 8));
      } catch (err) {
        Logger.error('channel search failed:', err);
        setSearchResults([]);
      } finally {
        setIsSearching(false);
      }
    }, 300);

    return () => {
      if (searchTimeoutRef.current) clearTimeout(searchTimeoutRef.current);
    };
  }, [enabled, searchInput, query]);

  const reset = useCallback(() => {
    setSearchInput('');
    setSearchResults([]);
    setIsSearching(false);
    setHighlightIndex(0);
  }, []);

  return {
    // query state
    searchInput,
    setSearchInput,
    query,
    isSearching,
    // results
    followingItems,
    searchItems,
    visibleItems,
    followedCount: followedStreams.length,
    // keyboard navigation
    highlightIndex,
    setHighlightIndex,
    listRef,
    // actions
    refreshFollowing: loadFollowedStreams,
    reset,
  };
}
