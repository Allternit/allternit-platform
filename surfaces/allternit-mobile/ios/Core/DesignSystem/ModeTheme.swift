import SwiftUI

/// Per-mode visual theme, ported from the web's mode config
/// (surfaces/ai.allternit.com/src/shell/ModeSwitcher.tsx:24-70) and the
/// per-surface agent-mode palette
/// (surfaces/ai.allternit.com/src/views/chat/agentModeSurfaceTheme.tsx:16-56):
/// `soft` is accent at 14%, `glow` is accent at ~28%, matching the web's
/// rgba treatments of each `--accent-*`.
struct ModeTheme {
    let mode: AppMode

    /// Dark-theme accent (theme.css `--accent-chat/cowork/code/browser`).
    var accent: Color {
        switch mode {
        case .chat: return Theme.accentChat
        case .cowork: return Theme.accentCowork
        case .code: return Theme.accentCode
        case .browser: return Theme.accentBrowser
        }
    }

    /// Soft background tint (web `soft`: accent at 14%).
    var accentSoft: Color { accent.opacity(0.14) }

    /// Glow border/shadow color (web `glow`: accent at ~28%).
    var accentGlow: Color { accent.opacity(0.28) }

    /// SF Symbol standing in for the web's Phosphor mode icon
    /// (ChatText / UsersThree / TerminalWindow / Globe).
    var icon: String {
        switch mode {
        case .chat: return "message"
        case .cowork: return "person.3"
        case .code: return "terminal"
        case .browser: return "globe"
        }
    }
}

/// Agent-mode tiles shown in the composer's bottom deck when agent mode is
/// on. Mirrors `MODE_TABS` + `SURFACE_MODES`
/// (surfaces/ai.allternit.com/src/views/chat/components/ModeDock.tsx:30-48),
/// including the per-tile colors (dark status hues for research/data/slides).
enum AgentModeTile: String, CaseIterable, Sendable {
    case swarms
    case research
    case website
    case docs
    case data
    case slides
    case image
    case video
    case code

    var label: String {
        switch self {
        case .swarms: return "Agent Swarm"
        case .research: return "Deep Research"
        case .website: return "Websites"
        case .docs: return "Docs"
        case .data: return "Sheets"
        case .slides: return "Slides"
        case .image: return "Image"
        case .video: return "Video"
        case .code: return "Code"
        }
    }

    var color: Color {
        switch self {
        case .swarms: return Color(hex: "#14B8A6")
        case .research: return Theme.statusInfo
        case .website: return Color(hex: "#6366F1")
        case .docs: return Color(hex: "#3B82F6")
        case .data: return Theme.statusSuccess
        case .slides: return Theme.statusWarning
        case .image: return Color(hex: "#8B5CF6")
        case .video: return Color(hex: "#EC4899")
        case .code: return Theme.statusWarning
        }
    }

    /// SF Symbols standing in for the web's Phosphor icons (UsersThree,
    /// BookOpen, Globe, FileText, Database, PresentationChart, Image,
    /// VideoCamera, Code).
    var icon: String {
        switch self {
        case .swarms: return "person.3"
        case .research: return "book"
        case .website: return "globe"
        case .docs: return "doc.text"
        case .data: return "externaldrive"
        case .slides: return "chart.bar.doc.horizontal"
        case .image: return "photo"
        case .video: return "video"
        case .code: return "chevron.left.forwardslash.chevron.right"
        }
    }

    /// The Allternit icon family tile for this mode (web ModeDock
    /// AgentModeIcon → ArtifactKindIcon, Assets.xcassets/DeckIcons). Code
    /// has none on the web and keeps its SF Symbol.
    var deckIcon: String? {
        switch self {
        case .swarms: return "deck-swarms"
        case .research: return "deck-research"
        case .website: return "deck-build"
        case .docs: return "deck-docs"
        case .data: return "deck-sheets"
        case .slides: return "deck-slides"
        case .image: return "deck-image"
        case .video: return "deck-video"
        case .code: return nil
        }
    }

    /// Default prompt that populates the composer when the tile is selected
    /// (the web fills a starter task for each mode).
    var taskPrompt: String {
        switch self {
        case .swarms: return "Coordinate a swarm of agents to tackle this:"
        case .research: return "Research this topic in depth and summarize the findings:"
        case .website: return "Explore this website and summarize what you find: https://"
        case .docs: return "Create or edit a document about:"
        case .data: return "Build a spreadsheet or analyze data for:"
        case .slides: return "Create a presentation about:"
        case .image: return "Generate or describe an image for:"
        case .video: return "Plan or describe a video about:"
        case .code: return "Write or review code for:"
        }
    }

    /// Short explainer shown in the agent-mode top deck when the tile is
    /// selected, giving the user context about what the mode will do.
    var contextDescription: String {
        switch self {
        case .swarms:
            return "Multiple agents collaborate on your task in parallel."
        case .research:
            return "Deep research with sources and structured summaries."
        case .website:
            return "Fetch, read, and act on content from any website."
        case .docs:
            return "Draft, edit, and refine documents with the agent."
        case .data:
            return "Build sheets, run analysis, and extract insights."
        case .slides:
            return "Generate slide decks and talking points."
        case .image:
            return "Create, edit, or analyze images."
        case .video:
            return "Plan scripts, storyboards, and video content."
        case .code:
            return "Write, review, and debug code in any language."
        }
    }

    /// Starter prompt rows shown in the collapsed agent-mode bottom deck
    /// (scrollable — the deck caps at ~3 rows and the rest scroll).
    /// Tapping a row fills the composer with that prompt, giving the user a
    /// quick way to reset or vary the starter task for the selected mode.
    var templates: [String] {
        switch self {
        case .swarms:
            return [
                "Coordinate a research swarm",
                "Coordinate a coding swarm",
                "Split this project across parallel agents:",
                "Run a competitive analysis swarm on",
            ]
        case .research:
            return [
                "Research the latest news on",
                "Research academic sources for",
                "Compare the top options for",
                "Fact-check this claim and cite sources:",
            ]
        case .website:
            return [
                "Summarize this website: https://",
                "Extract links from: https://",
                "Monitor this page for changes: https://",
                "Turn this article into key points: https://",
            ]
        case .docs:
            return [
                "Write a memo about",
                "Edit this document about",
                "Draft a proposal for",
                "Proofread and tighten this draft:",
            ]
        case .data:
            return [
                "Analyze this dataset:",
                "Build a spreadsheet for",
                "Create a budget tracker for",
                "Find trends in this data:",
            ]
        case .slides:
            return [
                "Create a pitch deck about",
                "Create a status update deck for",
                "Build a lesson deck on",
                "Turn these notes into slides:",
            ]
        case .image:
            return [
                "Generate an illustration of",
                "Describe this image idea:",
                "Create a logo concept for",
                "Generate a hero image for",
            ]
        case .video:
            return [
                "Write a script for",
                "Create a storyboard for",
                "Outline a short-form video about",
                "Plan a product demo video for",
            ]
        case .code:
            return [
                "Write a function that",
                "Review and debug this code:",
                "Refactor this for clarity:",
                "Add tests for this code:",
            ]
        }
    }

    /// `SURFACE_MODES` from ModeDock.tsx:42-48 — which tiles a surface's
    /// bottom deck offers. Chat and cowork both get the full set.
    static func visibleTiles(for surface: AppMode) -> [AgentModeTile] {
        switch surface {
        case .chat, .cowork:
            return [.swarms, .research, .website, .docs, .data, .slides, .image, .video, .code]
        case .code:
            return [.swarms, .website, .docs]
        case .browser:
            return [.research, .website, .docs, .data]
        }
    }
}

/// Terminal treatment for code-mode threads — the PLATFORM palette with a
/// terminal layout: monospace type, `❯` prompt lines, boot-style header,
/// blinking block cursor. Not a dark-theme takeover — it matches the rest
/// of the app, just behaves like a terminal session.
enum TerminalTheme {
    /// Session background — the platform feed background.
    static let bg = Color("BgSecondary")
    /// Raised surfaces (composer card, bottom deck).
    static let panel = Color("BgPanel")
    /// Primary text.
    static let text = Color("TextPrimary")
    /// Dimmed text (prompts, placeholders, inactive icons).
    static let dim = Color("TextSecondary")
    /// Prompt glyph / accents — the code mode accent green.
    static let accent = Theme.accentCode
}
