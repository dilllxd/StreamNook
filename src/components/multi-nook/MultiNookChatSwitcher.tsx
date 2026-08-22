import React from 'react';
import { usemultiNookStore } from '../../stores/multiNookStore';
import { DEFAULT_PROVIDER, PROVIDERS } from '../../types/providers';
import { makeKey } from '../../utils/providerKey';
import { Tooltip } from '../ui/Tooltip';

const MultiNookChatSwitcher: React.FC = () => {
  const { slots, activeChatChannelId, setActiveChatChannelId } = usemultiNookStore();

  if (slots.length <= 1) return null;

  return (
    <div className="flex-shrink-0 flex items-center gap-2 p-2 px-3 overflow-x-auto scrollbar-thin border-b border-borderSubtle bg-glass/30 backdrop-blur-sm shadow-sm" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
      <div className="flex items-center gap-1.5 min-w-max">
        {slots.map((slot) => {
          const provider = slot.provider ?? DEFAULT_PROVIDER;
          const chatKey = provider === 'twitch'
            ? slot.channelId ?? slot.channelLogin
            : makeKey(provider, slot.channelLogin);
          const isActive = activeChatChannelId === chatKey;

          return (
            <Tooltip key={slot.id} content={`Switch to ${PROVIDERS[provider].label} chat for ${slot.channelName || slot.channelLogin}`} side="bottom">
              <button
                onClick={() => setActiveChatChannelId(chatKey)}
                aria-pressed={isActive}
                className={`
                  px-3 py-1.5 text-xs font-bold tracking-wide transition-all duration-200 flex items-center gap-1.5
                  ${isActive 
                    ? 'glass-input text-emerald-400 font-extrabold' 
                    : 'glass-button text-textSecondary hover:text-white'}
                `}
                style={{ borderRadius: '8px' }}
              >
                <span
                  aria-hidden="true"
                  className="h-1.5 w-1.5 flex-shrink-0 rounded-full"
                  style={{ backgroundColor: PROVIDERS[provider].color }}
                />
                {slot.channelName || slot.channelLogin}
              </button>
            </Tooltip>
          );
        })}
      </div>
    </div>
  );
};

export default MultiNookChatSwitcher;

