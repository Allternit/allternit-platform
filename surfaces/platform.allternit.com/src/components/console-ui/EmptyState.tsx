import React from 'react';
import { cn } from '@/lib/utils';
import { EmptyArt, type EmptyArtKind } from './empty-art';

export type { EmptyArtKind } from './empty-art';

/** Art sizes per treatment (empty-art-v2 CONTRACT.md): full 96, panel 64, inline 36. */
const ART_SIZE = { full: 96, panel: 64, inline: 36 } as const;

interface EmptyStateProps {
  /** Legacy icon. Ignored when `art` is set (the art replaces it). */
  icon?: React.ReactNode;
  /** Empty-state drawing. Decorative (aria-hidden). */
  art?: EmptyArtKind;
  /** `full` = a whole pane or view (art 96). `panel` = a block in a list, card, tab or dropdown (art 64). */
  size?: 'full' | 'panel';
  /** Fill for the art's solid parts when the block sits on a filled card (sets --ea-bg). */
  paper?: string;
  title?: string;
  caption?: string;
  ctaLabel?: string;
  onCtaClick?: () => void;
  className?: string;
  primaryCta?: boolean;
}

export function EmptyState({
  icon,
  art,
  size = 'full',
  paper,
  title,
  caption,
  ctaLabel,
  onCtaClick,
  className,
  primaryCta = false,
}: EmptyStateProps): React.ReactNode {
  const panel = size === 'panel';
  return (
    <div
      className={cn(
        'flex flex-col items-center justify-center text-center',
        panel ? 'py-6 px-4' : 'py-12 px-6',
        className
      )}
    >
      {art ? (
        <div className={cn('text-[var(--text-primary)]', panel ? 'mb-3' : 'mb-4')}>
          <EmptyArt kind={art} size={ART_SIZE[size]} paper={paper} />
        </div>
      ) : icon ? (
        <div className="text-[var(--text-tertiary)] mb-4">{icon}</div>
      ) : null}
      {title && (
        <h3
          className={cn(
            'font-semibold text-[var(--text-primary)] m-0 mb-1',
            panel ? 'text-[14px]' : 'text-[16px]'
          )}
        >
          {title}
        </h3>
      )}
      {caption && (
        <p className="text-[13px] text-[var(--text-secondary)] m-0 max-w-xs leading-relaxed">
          {caption}
        </p>
      )}
      {ctaLabel && onCtaClick && (
        <button
          type="button"
          onClick={onCtaClick}
          className={cn(
            'mt-5 px-4 py-2 rounded-lg text-[13px] font-semibold cursor-pointer transition-colors',
            primaryCta
              ? 'bg-[var(--accent-primary)] text-[var(--ui-text-inverse)] hover:brightness-110'
              : 'border border-solid border-[var(--border-subtle)] bg-transparent text-[var(--text-primary)] hover:bg-[var(--bg-secondary)]'
          )}
        >
          {ctaLabel}
        </button>
      )}
    </div>
  );
}

interface EmptyInlineProps {
  /** Drawing beside the line, 36 px (scenes 60 x 36). Decorative (aria-hidden). */
  art: EmptyArtKind;
  /** The line, kept exactly as written at the call site. */
  children: React.ReactNode;
  /** Fill for the art's solid parts on a filled card (sets --ea-bg). */
  paper?: string;
  className?: string;
}

/** Inline treatment: one muted line with 36 px art in front of it. */
export function EmptyInline({ art, children, paper, className }: EmptyInlineProps): React.ReactNode {
  return (
    <div className={cn('flex items-center gap-2.5 text-[13px] text-[var(--text-secondary)]', className)}>
      <EmptyArt kind={art} size={ART_SIZE.inline} paper={paper} className="shrink-0" />
      <span className="min-w-0">{children}</span>
    </div>
  );
}
