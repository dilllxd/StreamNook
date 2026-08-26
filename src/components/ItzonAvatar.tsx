import { useState, type CSSProperties } from 'react';
import { itzonProfileColor } from '../services/itzon';

interface ItzonAvatarProps {
  src?: string | null;
  name: string;
  alt?: string;
  className?: string;
  fallbackClassName?: string;
  style?: CSSProperties;
  loading?: 'eager' | 'lazy';
  draggable?: boolean;
}

export function ItzonAvatar({
  src,
  name,
  alt = '',
  className = '',
  fallbackClassName = '',
  style,
  loading = 'lazy',
  draggable,
}: ItzonAvatarProps) {
  const [failedSrc, setFailedSrc] = useState<string | null>(null);

  if (!src || failedSrc === src) {
    return (
      <span
        aria-hidden="true"
        className={`inline-flex select-none items-center justify-center font-semibold uppercase text-black/75 ${className} ${fallbackClassName}`}
        style={{ ...style, backgroundColor: itzonProfileColor(name) }}
      >
        {name.trim().slice(0, 1).toUpperCase() || '?'}
      </span>
    );
  }

  return (
    <img
      src={src}
      alt={alt}
      className={className}
      style={style}
      loading={loading}
      draggable={draggable}
      onError={() => setFailedSrc(src)}
    />
  );
}
