import Foundation

/// The skill directory Customize → Skills → Discover offers (allternit-ai
/// src/plugins/catalog/native-plugins.ts, entries tagged `skill`, in the
/// web's order). Generated from that file; regenerate when it changes.
/// Adding one installs it on the user's computer through
/// `POST /api/v1/skills/catalog/install` (SkillCatalogClient).
struct SkillCatalogEntry: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let description: String
    let category: String
    let author: String
}

enum SkillCatalog {
    static let entries: [SkillCatalogEntry] = [
        SkillCatalogEntry(id: "claude-algorithmic-art", name: "Algorithmic Art", description: "Creating algorithmic art using p5.js with seeded randomness and interactive parameter exploration. Use this when users request creating art using code,…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-brand-guidelines", name: "Brand Guidelines", description: "Applies Anthropic's official brand colors and typography to any sort of artifact that may benefit from having Anthropic's look-and-feel. Use it when brand…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-canvas-design", name: "Canvas Design", description: "Create beautiful visual art in .png and .pdf documents using design philosophy. You should use this skill when the user asks to create a poster, piece of art,…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-claude-api", name: "Claude API", description: "Build, debug, and optimize Claude API / Anthropic SDK apps. Apps built with this skill should include prompt caching. Also handles migrating existing Claude…", category: "build", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-doc-coauthoring", name: "Doc Co-Authoring", description: "Guide users through a structured workflow for co-authoring documentation. Use when user wants to write documentation, proposals, technical specs, decision…", category: "productivity", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-docx", name: "DOCX", description: "Use this skill whenever the user wants to create, read, edit, or manipulate Word documents (.docx files). Triggers include: any mention of 'Word doc', 'word…", category: "productivity", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-frontend-design", name: "Frontend Design", description: "Create distinctive, production-grade frontend interfaces with high design quality. Use this skill when the user asks to build web components, pages, artifacts,…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-internal-comms", name: "Internal Comms", description: "A set of resources to help me write all kinds of internal communications, using the formats that my company likes to use. Claude should use this skill whenever…", category: "productivity", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-mcp-builder", name: "MCP Builder", description: "Guide for creating high-quality MCP (Model Context Protocol) servers that enable LLMs to interact with external services through well-designed tools. Use when…", category: "build", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-pdf", name: "PDF", description: "Use this skill whenever the user wants to do anything with PDF files. This includes reading or extracting text/tables from PDFs, combining or merging multiple…", category: "productivity", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-pptx", name: "PPTX", description: "Use this skill any time a .pptx file is involved in any way — as input, output, or both. This includes: creating slide decks, pitch decks, or presentations;…", category: "productivity", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-skill-creator", name: "Skill Creator", description: "Create new skills, modify and improve existing skills, and measure skill performance. Use when users want to create a skill from scratch, edit, or optimize an…", category: "build", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-slack-gif-creator", name: "Slack GIF Creator", description: "Knowledge and utilities for creating animated GIFs optimized for Slack. Provides constraints, validation tools, and animation concepts. Use when users request…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-theme-factory", name: "Theme Factory", description: "Toolkit for styling artifacts with a theme. These artifacts can be slides, docs, reportings, HTML landing pages, etc. There are 10 pre-set themes with…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-web-artifacts-builder", name: "Web Artifacts Builder", description: "Suite of tools for creating elaborate, multi-component claude.ai HTML artifacts using modern frontend web technologies (React, Tailwind CSS, shadcn/ui). Use…", category: "create", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-webapp-testing", name: "Web App Testing", description: "Toolkit for interacting with and testing local web applications using Playwright. Supports verifying frontend functionality, debugging UI behavior, capturing…", category: "build", author: "Anthropic"),
        SkillCatalogEntry(id: "claude-xlsx", name: "XLSX", description: "Use this skill any time a spreadsheet file is the primary input or output. This means any task where the user wants to: open, read, edit, or fix an existing…", category: "productivity", author: "Anthropic"),
        SkillCatalogEntry(id: "allternit-alabs-course-pipeline", name: "ALabs Course Pipeline", description: "End-to-end A://Labs course creation pipeline. Creates Canvas courses, generates interactive HTML modules from Allternit packages, syncs quizzes, tracks…", category: "productivity", author: "Allternit"),
        SkillCatalogEntry(id: "allternit-allternit-codebase-to-course", name: "Allternit Codebase To Course", description: "Turn any Allternit package or codebase into a beautiful, interactive single-page HTML course module for the A://Labs curriculum. Use this skill whenever…", category: "productivity", author: "Allternit"),
        SkillCatalogEntry(id: "codex-clone-website", name: "Clone Website", description: "Reverse-engineers any public website and rebuilds it as a pixel-accurate React page using the Allternit design system. Extracts computed styles, design tokens,…", category: "build", author: "Codex"),
        SkillCatalogEntry(id: "codex-mobile-app-design", name: "Mobile App Design", description: "Mobile app UI/UX design skill. Generates frame structures, React Native / Expo component code, asset manifests, and navigation flows. Outputs Figma-compatible…", category: "create", author: "Codex"),
    ]
}
