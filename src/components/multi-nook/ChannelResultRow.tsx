import React from 'react';
import { Plus, Users } from 'lucide-react';
import { ChannelItem, DEFAULT_AVATAR } from './channelSearch';
import { ProviderLogo } from '../ProviderLogo';

/** One row in a channel smart-list. Renders both live follows and Twitch search
 *  hits identically, with an optional trailing slot for a non-add affordance
 *  (e.g. a check when the channel is already in the preset). */
export const ChannelResultRow: React.FC<{
  item: ChannelItem;
  index: number;
  highlighted: boolean;
  disabled?: boolean;
  /** Override the trailing affordance (defaults to a + add indicator). */
  trailing?: React.ReactNode;
  onSelect: (item: ChannelItem) => void;
  onHover: (index: number) => void;
}> = ({ item, index, highlighted, disabled = false, trailing, onSelect, onHover }) => {
  const viewerLabel =
    item.viewerCount === undefined
      ? null
      : item.viewerCount >= 1000
        ? `${(item.viewerCount / 1000).toFixed(item.viewerCount >= 10000 ? 0 : 1)}K`
        : String(item.viewerCount);

  return (
    <button
      data-idx={index}
      onClick={() => onSelect(item)}
      onMouseEnter={() => onHover(index)}
      disabled={disabled}
      className={`w-full px-2.5 py-2 text-left rounded-lg transition-all duration-150 flex items-center gap-3 group disabled:opacity-40 ${
        highlighted ? 'bg-white/[0.06]' : 'hover:bg-white/[0.06]'
      }`}
    >
      {/* Avatar with accent ring when active */}
      <div className="relative shrink-0">
        {item.avatarUrl ? (
          <img
            src={item.avatarUrl}
            alt={item.displayName}
            onError={(e) => {
              const img = e.currentTarget as HTMLImageElement;
              if (img.src !== DEFAULT_AVATAR) img.src = DEFAULT_AVATAR;
            }}
            className={`w-8 h-8 rounded-full object-cover ring-2 transition-all duration-200 shadow-sm ${
              highlighted ? 'ring-accent/30' : 'ring-transparent group-hover:ring-accent/30'
            }`}
          />
        ) : (
          <div
            className={`w-8 h-8 rounded-full bg-white/[0.04] ring-2 flex items-center justify-center transition-all duration-200 ${
              highlighted ? 'ring-accent/30' : 'ring-transparent group-hover:ring-accent/30'
            }`}
          >
            <Users size={13} className="text-textSecondary" />
          </div>
        )}
        {/* Live dot on avatar */}
        {item.isLive && (
          <span className="absolute -bottom-0.5 -right-0.5 w-2.5 h-2.5 rounded-full bg-red-500 border-2 border-surface/80"></span>
        )}
      </div>

      {/* Info */}
      <div className="flex-1 min-w-0">
        <span className="flex items-center gap-1.5 min-w-0">
          <ProviderLogo provider={item.provider ?? 'twitch'} size={10} />
          <span
            className={`block text-[13px] font-semibold truncate leading-tight transition-colors ${
              highlighted ? 'text-textPrimary' : 'text-textPrimary group-hover:text-white'
            }`}
          >
            {item.displayName}
          </span>
        </span>
        <span className="block text-[11px] text-textMuted truncate mt-0.5 leading-tight">
          {item.isLive && item.gameName ? item.gameName : item.isLive ? 'Live' : item.login}
        </span>
      </div>

      {/* Trailing affordance, defaults to an add indicator */}
      {trailing !== undefined ? (
        trailing
      ) : (
        <div className="flex items-center gap-2 shrink-0">
          {viewerLabel && <span className="text-[10px] tabular-nums text-textMuted">{viewerLabel}</span>}
          <div
            className={`w-6 h-6 rounded-md flex items-center justify-center transition-colors duration-150 ${
              highlighted ? 'bg-white/[0.07]' : 'bg-transparent group-hover:bg-white/[0.07]'
            }`}
          >
            <Plus size={13} className="text-textMuted group-hover:text-textPrimary transition-colors" />
          </div>
        </div>
      )}
    </button>
  );
};
