/**
 * Slides chrome inside an Allternit artifact session (the host has its own
 * chat beside the editor): no ribbon or status bar — a floating text toolbar
 * over the text being edited, a Theme/Custom colour popover fed by the
 * session's design system, and a small view pill (download, present,
 * slide list, zoom). The full ribbon stays one click away (⋯).
 *
 * Every control acts on mousedown with preventDefault, so the text being
 * edited keeps its selection (same rule as the ribbon's colour swatches).
 */
import React, { useEffect, useRef, useState } from 'react'
import type { OfficeDesignSystem } from '@allternit/office-suite/bridge'
import { FONT_FAMILIES, FONT_SIZES, TEXT_COLORS } from './ribbon-shared'

const keep = (fn: () => void) => (e: React.MouseEvent) => {
  e.preventDefault()
  e.stopPropagation()
  fn()
}

const ICON = {
  width: 15,
  height: 15,
  viewBox: '0 0 16 16',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.5,
  strokeLinecap: 'round' as const,
  strokeLinejoin: 'round' as const,
}

function Menu({
  label,
  title,
  width,
  children,
}: {
  label: React.ReactNode
  title: string
  width?: number
  children: (close: () => void) => React.ReactNode
}) {
  const [open, setOpen] = useState(false)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) setOpen(false)
    }
    window.addEventListener('mousedown', onDown)
    return () => window.removeEventListener('mousedown', onDown)
  }, [open])
  return (
    <div className="sc-menu" ref={ref}>
      <button
        type="button"
        className={`sc-btn sc-menu-trigger${open ? ' on' : ''}`}
        title={title}
        aria-label={title}
        aria-expanded={open}
        onMouseDown={keep(() => setOpen((v) => !v))}
      >
        {label}
      </button>
      {open && (
        <div className="sc-pop" style={width ? { width } : undefined} role="menu">
          {children(() => setOpen(false))}
        </div>
      )}
    </div>
  )
}

const Caret = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden>
    <path d="M2.5 4 5 6.5 7.5 4" />
  </svg>
)

/** The Theme/Custom colour popover body. */
export function ColorPopover({
  designSystem,
  current,
  onPick,
  onApplyTheme,
}: {
  designSystem?: OfficeDesignSystem
  current?: string | null
  onPick: (hex: string) => void
  /** Make the design system the deck's theme (colours + fonts). */
  onApplyTheme?: () => void
}) {
  const hasTheme = !!designSystem && designSystem.colors.length > 0
  const [tab, setTab] = useState<'theme' | 'custom'>(hasTheme ? 'theme' : 'custom')
  const [hex, setHex] = useState(current ?? '#000000')
  const valid = /^#[0-9a-f]{6}$/i.test(hex)
  return (
    <div className="sc-color">
      <div className="sc-tabs" role="tablist">
        {(['theme', 'custom'] as const).map((k) => (
          <button
            key={k}
            type="button"
            role="tab"
            aria-selected={tab === k}
            className={`sc-tab${tab === k ? ' on' : ''}`}
            onMouseDown={keep(() => setTab(k))}
          >
            {k === 'theme' ? 'Theme' : 'Custom'}
          </button>
        ))}
      </div>
      {tab === 'theme' ? (
        hasTheme ? (
          <>
            <div className="sc-color-title">{designSystem!.name}</div>
            <div className="sc-color-sub">Palette</div>
            <div className="sc-swatch-list">
              {designSystem!.colors.map((c) => (
                <button
                  key={`${c.name}${c.value}`}
                  type="button"
                  className={`sc-swatch-row${current?.toLowerCase() === c.value.toLowerCase() ? ' on' : ''}`}
                  onMouseDown={keep(() => onPick(c.value))}
                  title={c.value}
                >
                  <span className="sc-swatch" style={{ background: c.value }} />
                  <span className="sc-swatch-name">{c.name}</span>
                </button>
              ))}
            </div>
            {onApplyTheme && (
              <button type="button" className="sc-apply sc-apply-theme" onMouseDown={keep(onApplyTheme)}>
                Apply as deck theme
              </button>
            )}
          </>
        ) : (
          <div className="sc-color-empty">No design system is chosen for this deck.</div>
        )
      ) : (
        <>
          <div className="sc-swatch-grid">
            {TEXT_COLORS.map((c) => (
              <button
                key={c}
                type="button"
                className="sc-swatch sc-swatch-btn"
                style={{ background: c }}
                title={c}
                aria-label={c}
                onMouseDown={keep(() => onPick(c))}
              />
            ))}
          </div>
          <div className="sc-hex">
            <input
              type="color"
              aria-label="Pick a colour"
              value={valid ? hex : '#000000'}
              onChange={(e) => setHex(e.target.value)}
            />
            <input
              className="sc-hex-input"
              aria-label="Hex colour"
              value={hex}
              onChange={(e) => setHex(e.target.value.trim())}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && valid) onPick(hex)
              }}
            />
            <button type="button" className="sc-apply" disabled={!valid} onMouseDown={keep(() => valid && onPick(hex))}>
              Apply
            </button>
          </div>
        </>
      )}
    </div>
  )
}

export interface TextToolbarProps {
  /** Where the text box is, in slide px (the toolbar sits above it, or below near the top edge). */
  box: { x: number; y: number; w: number; h: number }
  zoom: number
  designSystem?: OfficeDesignSystem
  fontFamily: string | null
  fontSizePt: number | null
  onFontFamily: (family: string) => void
  onFontSize: (pt: number) => void
  onColor: (hex: string) => void
  onToggle: (kind: 'bold' | 'italic' | 'underline') => void
  onLink: () => void
  onBullets: () => void
  onAlign: (align: 'left' | 'center' | 'right') => void
  onComment?: () => void
  onMore: () => void
  onApplyTheme?: () => void
}

/** Floats above the text box being edited, at a constant on-screen size. */
export function FloatingTextToolbar(p: TextToolbarProps) {
  const [color, setColor] = useState<string | null>(null)
  const [align, setAlign] = useState<'left' | 'center' | 'right'>('left')
  const below = p.box.y * p.zoom < 56
  const families = [
    ...(p.designSystem?.fonts ?? []),
    ...FONT_FAMILIES.filter((f) => !p.designSystem?.fonts.includes(f)),
  ]
  if (p.fontFamily && !families.includes(p.fontFamily)) families.unshift(p.fontFamily)
  const nextAlign = { left: 'center', center: 'right', right: 'left' } as const
  return (
    <div
      className="sc-toolbar"
      role="toolbar"
      aria-label="Text formatting"
      style={{
        left: p.box.x,
        top: below ? p.box.y + p.box.h : p.box.y,
        transform: `scale(${1 / p.zoom}) translateY(${below ? 8 : 'calc(-100% - 8px)'})`,
        transformOrigin: 'top left',
      }}
      onMouseDown={(e) => e.preventDefault()}
    >
      <Menu
        title="Font"
        width={220}
        label={
          <>
            <span className="sc-aa">Aa</span>
            <span className="sc-font-label">
              {p.fontFamily ?? 'Font'}
              {p.fontSizePt ? ` · ${p.fontSizePt}` : ''}
            </span>
            <Caret />
          </>
        }
      >
        {(close) => (
          <div className="sc-font-menu">
            <div className="sc-font-list">
              {families.map((f, i) => (
                <button
                  key={f}
                  type="button"
                  className={`sc-item${f === p.fontFamily ? ' on' : ''}`}
                  style={{ fontFamily: f }}
                  onMouseDown={keep(() => {
                    p.onFontFamily(f)
                    close()
                  })}
                >
                  {f}
                  {p.designSystem && i < p.designSystem.fonts.length ? <span className="sc-ds-tag">Design system</span> : null}
                </button>
              ))}
            </div>
            <div className="sc-size-list">
              {FONT_SIZES.map((s) => (
                <button
                  key={s}
                  type="button"
                  className={`sc-item${s === p.fontSizePt ? ' on' : ''}`}
                  onMouseDown={keep(() => {
                    p.onFontSize(s)
                    close()
                  })}
                >
                  {s}
                </button>
              ))}
            </div>
          </div>
        )}
      </Menu>
      <span className="sc-sep" />
      <Menu
        title="Text colour"
        width={232}
        label={<span className="sc-swatch sc-swatch-sm" style={{ background: color ?? 'currentColor' }} />}
      >
        {(close) => (
          <ColorPopover
            designSystem={p.designSystem}
            current={color}
            onPick={(hex) => {
              setColor(hex)
              p.onColor(hex)
              close()
            }}
            onApplyTheme={
              p.onApplyTheme
                ? () => {
                    p.onApplyTheme!()
                    close()
                  }
                : undefined
            }
          />
        )}
      </Menu>
      <button type="button" className="sc-btn" title="Bold" aria-label="Bold" onMouseDown={keep(() => p.onToggle('bold'))}>
        <b>B</b>
      </button>
      <button type="button" className="sc-btn" title="Italic" aria-label="Italic" onMouseDown={keep(() => p.onToggle('italic'))}>
        <i style={{ fontFamily: 'Georgia, serif' }}>I</i>
      </button>
      <button type="button" className="sc-btn" title="Underline" aria-label="Underline" onMouseDown={keep(() => p.onToggle('underline'))}>
        <u>U</u>
      </button>
      <button type="button" className="sc-btn" title="Link" aria-label="Link" onMouseDown={keep(p.onLink)}>
        <svg {...ICON}>
          <path d="M6.5 9.5a3 3 0 0 0 4.2 0l2-2a3 3 0 0 0-4.2-4.2l-.7.7M9.5 6.5a3 3 0 0 0-4.2 0l-2 2a3 3 0 0 0 4.2 4.2l.7-.7" />
        </svg>
      </button>
      <span className="sc-sep" />
      <button type="button" className="sc-btn" title="Bullets" aria-label="Bullets" onMouseDown={keep(p.onBullets)}>
        <svg {...ICON}>
          <path d="M6 4h8M6 8h8M6 12h8" />
          <circle cx="2.5" cy="4" r=".6" fill="currentColor" />
          <circle cx="2.5" cy="8" r=".6" fill="currentColor" />
          <circle cx="2.5" cy="12" r=".6" fill="currentColor" />
        </svg>
      </button>
      <button
        type="button"
        className="sc-btn"
        title={`Align ${align}`}
        aria-label={`Align (now ${align})`}
        onMouseDown={keep(() => {
          const next = nextAlign[align]
          setAlign(next)
          p.onAlign(next)
        })}
      >
        <svg {...ICON}>
          {align === 'left' && <path d="M2 4h12M2 8h8M2 12h10" />}
          {align === 'center' && <path d="M2 4h12M4 8h8M3 12h10" />}
          {align === 'right' && <path d="M2 4h12M6 8h8M4 12h10" />}
        </svg>
      </button>
      {p.onComment && (
        <>
          <span className="sc-sep" />
          <button type="button" className="sc-btn" title="Comment" aria-label="Comment" onMouseDown={keep(p.onComment)}>
            <svg {...ICON}>
              <path d="M3 3.5h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H7l-3 2.5v-2.5H3a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1Z" />
            </svg>
          </button>
        </>
      )}
      <button type="button" className="sc-btn" title="More formatting (ribbon)" aria-label="More formatting" onMouseDown={keep(p.onMore)}>
        <svg {...ICON}>
          <circle cx="8" cy="3.5" r=".8" fill="currentColor" />
          <circle cx="8" cy="8" r=".8" fill="currentColor" />
          <circle cx="8" cy="12.5" r=".8" fill="currentColor" />
        </svg>
      </button>
    </div>
  )
}

const ZOOM_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2]

/** Download, present, slide list, zoom — top right of the workspace. */
export function ViewPill(p: {
  zoom: number
  showThumbs: boolean
  ribbonShown: boolean
  onDownload: () => void
  onPresent: () => void
  onToggleThumbs: () => void
  onZoom: (z: number) => void
  onFit: () => void
  onToggleRibbon: () => void
}) {
  return (
    <div className="sc-pill" role="toolbar" aria-label="View">
      <button type="button" className="sc-btn" title="Download .pptx" aria-label="Download .pptx" onMouseDown={keep(p.onDownload)}>
        <svg {...ICON}>
          <path d="M8 2v8M4.5 6.5 8 10l3.5-3.5M2.5 13.5h11" />
        </svg>
      </button>
      <button type="button" className="sc-btn" title="Present" aria-label="Present" onMouseDown={keep(p.onPresent)}>
        <svg {...ICON}>
          <path d="M4.5 2.8v10.4L13 8 4.5 2.8Z" />
        </svg>
      </button>
      <span className="sc-sep" />
      <button
        type="button"
        className={`sc-btn${p.showThumbs ? ' on' : ''}`}
        title={p.showThumbs ? 'Hide slides' : 'Show slides'}
        aria-label={p.showThumbs ? 'Hide slides' : 'Show slides'}
        aria-pressed={p.showThumbs}
        onMouseDown={keep(p.onToggleThumbs)}
      >
        <svg {...ICON}>
          <rect x="2" y="2.5" width="12" height="11" rx="1.5" />
          <path d="M6 2.5v11" />
        </svg>
      </button>
      <span className="sc-sep" />
      <Menu
        title="Zoom"
        width={120}
        label={
          <>
            <span className="sc-zoom">{Math.round(p.zoom * 100)}%</span>
            <Caret />
          </>
        }
      >
        {(close) => (
          <>
            <button type="button" className="sc-item" onMouseDown={keep(() => { p.onFit(); close() })}>
              Fit
            </button>
            {ZOOM_STEPS.map((z) => (
              <button
                key={z}
                type="button"
                className={`sc-item${Math.abs(z - p.zoom) < 0.005 ? ' on' : ''}`}
                onMouseDown={keep(() => { p.onZoom(z); close() })}
              >
                {Math.round(z * 100)}%
              </button>
            ))}
          </>
        )}
      </Menu>
      <button
        type="button"
        className={`sc-btn${p.ribbonShown ? ' on' : ''}`}
        title={p.ribbonShown ? 'Hide the ribbon' : 'Show the ribbon'}
        aria-label={p.ribbonShown ? 'Hide the ribbon' : 'Show the ribbon'}
        aria-pressed={p.ribbonShown}
        onMouseDown={keep(p.onToggleRibbon)}
      >
        <svg {...ICON}>
          <circle cx="3.5" cy="8" r=".8" fill="currentColor" />
          <circle cx="8" cy="8" r=".8" fill="currentColor" />
          <circle cx="12.5" cy="8" r=".8" fill="currentColor" />
        </svg>
      </button>
    </div>
  )
}
