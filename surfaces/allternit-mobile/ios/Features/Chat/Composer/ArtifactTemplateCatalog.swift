import Foundation

// Generated from allternit-ai src/lib/templates/catalog.json (featured
// Docs / Slides / Sites / Sheets templates) by templatePrompt():
// each prompt already carries its style paragraph (describeStyle).
// Regenerate instead of editing by hand when the web catalog changes.

extension ArtifactTemplate {
    static let catalog: [ArtifactTemplate] = [
        ArtifactTemplate(
            id: "docs-project-proposal",
            mode: .docs,
            title: "Project Proposal",
            description: "Scope, timeline, budget and sign-off in one document",
            prompt: "Draft a professional project proposal. Ask me for the client, goals, scope, timeline, deliverables and budget. Structure: cover block (project name, client, date, version); executive summary (one paragraph); objectives as a numbered list; scope with in-scope / out-of-scope tables; approach in phases; timeline table (phase, dates, milestone); deliverables table; budget table with subtotal, contingency and total; assumptions and risks; acceptance and sign-off block.\n\nVisual style \"Navy & Brass\": use exactly this palette (background #FFFFFF, surface #F4F6FA, text #14213D, muted #5C677D, accent #1F3A68, secondary accent #B8893B); headings in Source Serif 4, body in Inter. Layout: Letter/A4, 1in margins, serif headings in navy, tables with navy header row and light zebra rows, brass rule under the title, totals row bold. Art direction: Trustworthy and formal; brass only for the title rule and totals. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/docs-project-proposal.webp",
            styleName: "Navy & Brass",
            swatches: ["#FFFFFF", "#14213D", "#1F3A68", "#B8893B"],
            placeholder: ["#FFFFFF", "#F4F6FA"],
            accent: "#1F3A68"
        ),
        ArtifactTemplate(
            id: "docs-prd",
            mode: .docs,
            title: "Product Requirements",
            description: "PRD with goals, user stories and acceptance criteria",
            prompt: "Draft a complete product requirements document. Ask me for the feature, the problem, target users and constraints. Structure: header table (owner, status, last updated, reviewers); TL;DR; problem and evidence; goals and non-goals; success metrics table (metric, baseline, target); user stories as 'As a… I want… so that…'; functional requirements numbered R1, R2… with priority (P0-P2); edge cases; acceptance criteria as checklists; open questions; rollout plan.\n\nVisual style \"Engineering Clean\": use exactly this palette (background #FFFFFF, surface #F6F8FA, text #1F2328, muted #59636E, accent #0969DA, secondary accent #8250DF); headings in Inter, body in Inter, code/labels in JetBrains Mono. Layout: A4, compact 11pt body, H1/H2/H3 hierarchy, status pills in the header table, requirement IDs in mono, callout boxes on the light grey surface for decisions. Art direction: Scannable; every requirement testable. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/docs-prd.webp",
            styleName: "Engineering Clean",
            swatches: ["#FFFFFF", "#1F2328", "#0969DA", "#8250DF"],
            placeholder: ["#FFFFFF", "#F6F8FA"],
            accent: "#0969DA"
        ),
        ArtifactTemplate(
            id: "docs-research-report",
            mode: .docs,
            title: "Research Report",
            description: "Long-form report with numbered sections and references",
            prompt: "Create a structured research report. Ask me for the question, scope and any sources (or use attached files and web research). Structure: title page block with author and date; abstract; executive summary with three key findings; 1 Introduction; 2 Methodology; 3 Findings (numbered subsections, each with a table or figure and a caption 'Figure n: …'); 4 Discussion; 5 Recommendations; References in a consistent citation style. Cite every factual claim.\n\nVisual style \"Academic Serif\": use exactly this palette (background #FFFFFF, surface #F7F5F0, text #1A1A1A, muted #666666, accent #7A1F2B); headings in Source Serif 4, body in Source Serif 4, code/labels in IBM Plex Mono. Layout: A4, 1in margins, numbered section headings (1, 1.1), 11pt justified serif body, figure captions in small italic, booktabs-style tables (top/bottom rules only). Art direction: LaTeX-report look without LaTeX; burgundy only for section numbers. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/docs-research-report.webp",
            styleName: "Academic Serif",
            swatches: ["#FFFFFF", "#1A1A1A", "#7A1F2B", "#F7F5F0"],
            placeholder: ["#FFFFFF", "#F7F5F0"],
            accent: "#7A1F2B"
        ),
        ArtifactTemplate(
            id: "docs-executive-memo",
            mode: .docs,
            title: "Executive Memo",
            description: "One-page decision memo with a clear recommendation",
            prompt: "Write a one-page executive decision memo. Ask me for the decision needed, the audience, options considered and the deadline. Structure: memo header (To, From, Date, Re); Bottom line up front in a shaded box (the recommendation in two sentences); Context (one paragraph); Options table comparing 2-3 options on cost, impact, risk and time; Recommendation and rationale; Next steps with owners and dates. Keep it to one page.\n\nVisual style \"Board Room\": use exactly this palette (background #FFFFFF, surface #F1F3F5, text #111111, muted #555F6D, accent #0F766E); headings in Inter, body in Inter. Layout: Letter, single page, memo header as a 2-column label/value grid with a heavy rule beneath, BLUF box on the grey surface with a teal left border, compact comparison table. Art direction: Direct and brief; no adjectives the reader can skip. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/docs-executive-memo.webp",
            styleName: "Board Room",
            swatches: ["#FFFFFF", "#111111", "#0F766E", "#F1F3F5"],
            placeholder: ["#FFFFFF", "#F1F3F5"],
            accent: "#0F766E"
        ),
        ArtifactTemplate(
            id: "docs-case-study",
            mode: .docs,
            title: "Customer Case Study",
            description: "Challenge, solution and results with hero metrics",
            prompt: "Write a customer case study. Ask me for the customer, their industry, the challenge, what we delivered and measurable results. Structure: headline with the outcome ('How X cut Y by 40%'); a three-metric results band at the top; customer snapshot sidebar table (industry, size, region, products used); The challenge; The solution; The results with the metrics restated and explained; a pull quote from the customer; What's next; call to action.\n\nVisual style \"Signal Orange\": use exactly this palette (background #FFFFFF, surface #FFF4EC, text #1C1917, muted #78716C, accent #EA580C); headings in Manrope, body in Inter. Layout: A4, bold 28pt outcome headline, metrics band of three large orange numbers with captions, two-column body with the snapshot table in the narrow column, large italic pull quote. Art direction: Marketing polish with evidence; numbers do the persuading. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/docs-case-study.webp",
            styleName: "Signal Orange",
            swatches: ["#FFFFFF", "#1C1917", "#EA580C", "#FFF4EC"],
            placeholder: ["#FFFFFF", "#FFF4EC"],
            accent: "#EA580C"
        ),
        ArtifactTemplate(
            id: "docs-modern-resume",
            mode: .docs,
            title: "Modern Résumé",
            description: "Clean one-page résumé with a skills sidebar",
            prompt: "Create a one-page résumé. Ask me for my name, target role, contact details, experience (company, title, dates, 3-4 achievement bullets each with a number), education, and skills; or read my attached CV. Lay it out with a name header and role line, a short profile, Experience in reverse-chronological order, Education, and a sidebar for Skills, Tools, Languages and Links. Rewrite bullets as quantified achievements.\n\nVisual style \"Slate & Sky\": use exactly this palette (background #FFFFFF, surface #F1F5F9, text #0F172A, muted #64748B, accent #0284C7); headings in Inter, body in Inter. Layout: A4 single page, 0.6in margins, name at 26pt bold, two columns (70/30) with the sidebar on the slate surface, dates right-aligned in muted, section labels in small sky-blue caps. Art direction: ATS-friendly: real text, no images, no tables for body content. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/docs-modern-resume.webp",
            styleName: "Slate & Sky",
            swatches: ["#FFFFFF", "#0F172A", "#0284C7", "#F1F5F9"],
            placeholder: ["#FFFFFF", "#F1F5F9"],
            accent: "#0284C7"
        ),
        ArtifactTemplate(
            id: "slides-investor-pitch",
            mode: .slides,
            title: "Investor Pitch",
            description: "Ten-slide seed deck with traction, market and the ask",
            prompt: "Create a 10-slide investor pitch deck. Ask me for the company name, one-line pitch, problem, product, traction numbers, market size, business model, team and the raise. Slides: 1 title hero with the one-liner; 2 problem as one bold statement plus three pain points; 3 solution with a product screenshot slot; 4 how it works as a three-step flow; 5 traction as a big-number hero with a small growth line chart; 6 market as nested TAM/SAM/SOM rings; 7 business model as three pricing cards; 8 competition as a 2x2 positioning matrix; 9 team as four portrait cards; 10 the ask with use-of-funds bars. Every content slide gets a different layout.\n\nVisual style \"Midnight Signal\": use exactly this palette (background #0B1020, surface #151B2E, text #F4F6FB, muted #8A93AD, accent #C6F432, secondary accent #5B8CFF); headings in Space Grotesk, body in Inter. Layout: 16:9, 64px margins, left-aligned headlines at 40-44pt, one idea per slide, numbers set huge in the accent color, thin 1px rules in the muted color. Art direction: Confident and technical; lime is reserved for the single most important number or word on each slide. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/slides-investor-pitch.webp",
            styleName: "Midnight Signal",
            swatches: ["#0B1020", "#F4F6FB", "#C6F432", "#5B8CFF"],
            placeholder: ["#0B1020", "#151B2E"],
            accent: "#C6F432"
        ),
        ArtifactTemplate(
            id: "slides-editorial-keynote",
            mode: .slides,
            title: "Editorial Keynote",
            description: "Story-led talk in a warm magazine style",
            prompt: "Create a 12-slide story-led keynote presentation. Ask me for the topic, audience and the one idea they should remember. Open with a full-bleed photo and a serif title, then a chapter structure (three chapters, each with a numbered chapter divider), pull-quote slides, one timeline slide, image-left/text-right slides, and close with a single-sentence takeaway slide. Use real photography found by image search for every image slot.\n\nVisual style \"Ivory Serif\": use exactly this palette (background #F6F1E7, surface #EDE5D6, text #1B1A17, muted #6B6457, accent #C2553A, secondary accent #2F4A3A); headings in Fraunces, body in Inter. Layout: 16:9, generous 80px margins, serif display titles at 48-60pt with tight leading, small-caps chapter labels, asymmetric image/text splits, lots of whitespace. Art direction: Magazine feel; terracotta only for chapter numbers and quote marks. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/slides-editorial-keynote.webp",
            styleName: "Ivory Serif",
            swatches: ["#F6F1E7", "#1B1A17", "#C2553A", "#2F4A3A"],
            placeholder: ["#F6F1E7", "#EDE5D6"],
            accent: "#C2553A"
        ),
        ArtifactTemplate(
            id: "slides-quarterly-review",
            mode: .slides,
            title: "Quarterly Business Review",
            description: "KPI tiles, charts and next-quarter plan",
            prompt: "Create a quarterly business review deck (10-12 slides). Ask me for the quarter, team, KPIs with targets vs actuals, wins, misses and next-quarter priorities. Include: an agenda slide; a KPI scorecard of six tiles each showing value, target and a green/amber/red delta; a revenue bar chart by month; a funnel slide; a table of initiatives with owner and status; a wins/misses two-column slide; and a next-quarter OKR slide. Use native charts and tables so the numbers stay editable.\n\nVisual style \"Graphite Data\": use exactly this palette (background #FFFFFF, surface #F3F4F6, text #111827, muted #6B7280, accent #2563EB, secondary accent #10B981); headings in IBM Plex Sans, body in IBM Plex Sans, code/labels in IBM Plex Mono. Layout: 16:9, 12-column grid, 48px margins, section label top-left in mono caps, cards with 12px radius on the light grey surface, charts in blue with green for positive deltas. Art direction: Calm and precise; data first, no decorative imagery. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/slides-quarterly-review.webp",
            styleName: "Graphite Data",
            swatches: ["#FFFFFF", "#111827", "#2563EB", "#10B981"],
            placeholder: ["#FFFFFF", "#F3F4F6"],
            accent: "#2563EB"
        ),
        ArtifactTemplate(
            id: "slides-product-launch",
            mode: .slides,
            title: "Product Launch",
            description: "Gradient-lit launch deck for a new feature or product",
            prompt: "Create an 8-slide product launch presentation. Ask me for the product name, the headline benefit, three key features, a customer quote, pricing and launch date. Slides: hero with the product name over a glowing gradient; the before/after problem; three feature slides each with a large UI mockup slot and one-line benefit; a quote slide with avatar; pricing tiers; a launch-date countdown slide with a clear call to action.\n\nVisual style \"Aurora\": use exactly this palette (background #140B2E, surface #221447, text #FFFFFF, muted #B7A9D9, accent #FF4FD8, secondary accent #FF9F43); headings in Syne, body in Manrope. Layout: 16:9, centered hero compositions, magenta-to-orange radial glows behind key visuals, rounded 20px device frames, headlines 54pt+ bold. Art direction: Energetic and premium; gradients only as light sources, never behind body text. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/slides-product-launch.webp",
            styleName: "Aurora",
            swatches: ["#140B2E", "#FFFFFF", "#FF4FD8", "#FF9F43"],
            placeholder: ["#140B2E", "#221447"],
            accent: "#FF4FD8"
        ),
        ArtifactTemplate(
            id: "slides-swiss-research",
            mode: .slides,
            title: "Research Briefing",
            description: "Swiss-grid deck for findings, methods and evidence",
            prompt: "Create a research briefing presentation (10 slides) from my findings or attached material. Ask me for the research question and sources if none are attached. Structure: title; question and why it matters; method in three numbered steps; four findings slides, each one claim headline plus one chart or figure and a source line; limitations; recommendations as a numbered list; references. Every finding must cite its source on the slide.\n\nVisual style \"Swiss Grid\": use exactly this palette (background #FFFFFF, surface #F2F2F2, text #0A0A0A, muted #707070, accent #E10600); headings in Inter, body in Inter, code/labels in JetBrains Mono. Layout: 16:9, strict 6-column grid, flush-left ragged-right text, huge section numbers (01, 02…) in red, heavy black rules, no rounded corners, no shadows. Art direction: International Typographic Style: objective, asymmetric, high contrast. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/slides-swiss-research.webp",
            styleName: "Swiss Grid",
            swatches: ["#FFFFFF", "#0A0A0A", "#E10600", "#F2F2F2"],
            placeholder: ["#FFFFFF", "#F2F2F2"],
            accent: "#E10600"
        ),
        ArtifactTemplate(
            id: "slides-team-onboarding",
            mode: .slides,
            title: "Team Onboarding",
            description: "Friendly welcome deck for new hires",
            prompt: "Create a new-hire onboarding presentation (10 slides). Ask me for the company, team, the new hire's role, key tools and the first-week schedule. Include: welcome hero; our mission in one sentence; values as four icon cards; org snapshot as a simple tree; meet-the-team portrait grid; tools you'll use as logo tiles; your first week as a five-day timeline; 30/60/90-day goals; who to ask for what; a warm closing slide.\n\nVisual style \"Soft Mint\": use exactly this palette (background #F3FBF7, surface #FFFFFF, text #0F3D2E, muted #5D7D70, accent #34D399, secondary accent #FBBF24); headings in Manrope, body in Manrope. Layout: 16:9, rounded 24px white cards with soft shadows on the mint background, emoji-free line icons, 56px margins, friendly 40pt headlines. Art direction: Welcoming and clear; yellow only for small highlights. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/slides-team-onboarding.webp",
            styleName: "Soft Mint",
            swatches: ["#F3FBF7", "#0F3D2E", "#34D399", "#FBBF24"],
            placeholder: ["#F3FBF7", "#FFFFFF"],
            accent: "#34D399"
        ),
        ArtifactTemplate(
            id: "sites-saas-landing",
            mode: .website,
            title: "SaaS Landing Page",
            description: "Dark product site with glow hero, features and pricing",
            prompt: "Build a production-ready, responsive landing page for a software product. Ask me for the product name, one-line value proposition, three key features and pricing tiers. Sections: sticky translucent nav; hero with headline, subcopy, two CTAs and a product screenshot in a glowing frame; logo cloud; three-column feature grid with icons; a split section with a code or UI sample; testimonials; three pricing cards with the middle one highlighted; FAQ accordion; footer. Single HTML file with Tailwind, semantic markup, accessible contrast, and subtle scroll-in motion that respects reduced-motion.\n\nVisual style \"Nightfall Indigo\": use exactly this palette (background #08090C, surface #111318, text #F7F8F8, muted #8A8F98, accent #7C83FF, secondary accent #22D3EE); headings in Inter, body in Inter. Layout: Max width 1200px, 96px section spacing, 1px hairline borders at 8% white, radial indigo glow behind the hero shot, 12px card radius. Art direction: Quiet, premium dark UI; motion is subtle. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sites-saas-landing.webp",
            styleName: "Nightfall Indigo",
            swatches: ["#08090C", "#F7F8F8", "#7C83FF", "#22D3EE"],
            placeholder: ["#08090C", "#111318"],
            accent: "#7C83FF"
        ),
        ArtifactTemplate(
            id: "sites-editorial-portfolio",
            mode: .website,
            title: "Editorial Portfolio",
            description: "Minimal serif portfolio with a project grid",
            prompt: "Build a personal portfolio website. Ask me for my name, discipline, a short bio, 6 projects (title, year, role, image) and contact links. Layout: minimal top bar with name and three links; oversized serif intro statement; a two-column project grid with large images, titles and years; an about section with portrait; a selected clients list; a contact footer with a large email link. Single HTML file, responsive, image hover reveals, no heavy frameworks.\n\nVisual style \"Paper & Ink\": use exactly this palette (background #F5F4F0, surface #EBE9E3, text #111111, muted #77736A, accent #111111); headings in Instrument Serif, body in Inter. Layout: Max width 1320px, 72-96px serif intro, 24px gutters, captions in 13px uppercase tracking, images with no radius. Art direction: Gallery restraint: typography and images do all the work. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sites-editorial-portfolio.webp",
            styleName: "Paper & Ink",
            swatches: ["#F5F4F0", "#111111", "#111111", "#EBE9E3"],
            placeholder: ["#F5F4F0", "#EBE9E3"],
            accent: "#111111"
        ),
        ArtifactTemplate(
            id: "sites-restaurant",
            mode: .website,
            title: "Restaurant & Café",
            description: "Warm one-pager with menu, hours and booking",
            prompt: "Build a one-page website for a restaurant or café. Ask me for the name, cuisine, location, hours, signature dishes with prices and a booking link. Sections: full-bleed hero photo with name and 'Book a table' button; short story; menu in two columns with dotted leaders between dish and price, grouped by course; a photo strip; hours and map embed; reviews; footer with address and socials. Use real food photography from image search.\n\nVisual style \"Trattoria\": use exactly this palette (background #FBF7F0, surface #F1E8D8, text #2B1D14, muted #7D6A58, accent #9F2B1E, secondary accent #C99A3B); headings in Playfair Display, body in Inter. Layout: Centered serif headings with small ornamental rules, menu with dotted leaders, warm cream background, photos with 4px radius. Art direction: Hospitable and classic. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sites-restaurant.webp",
            styleName: "Trattoria",
            swatches: ["#FBF7F0", "#2B1D14", "#9F2B1E", "#C99A3B"],
            placeholder: ["#FBF7F0", "#F1E8D8"],
            accent: "#9F2B1E"
        ),
        ArtifactTemplate(
            id: "sites-conference-event",
            mode: .website,
            title: "Conference Event",
            description: "Bold event site with speakers, schedule and tickets",
            prompt: "Build an event website for a conference or meetup. Ask me for the name, dates, city, venue, speakers, schedule and ticket tiers. Sections: oversized hero with date and city and a ticket CTA; marquee of topics; speaker grid with photos, names and talks; day-by-day schedule with tabs; venue with map; ticket tiers; sponsors; FAQ; footer. Single responsive HTML file with Tailwind.\n\nVisual style \"Brutal Yellow\": use exactly this palette (background #FFE14D, surface #FFFFFF, text #0A0A0A, muted #3D3D3D, accent #0A0A0A, secondary accent #FF3B30); headings in Archivo Black, body in Space Grotesk. Layout: Neo-brutalist: 3px black borders, hard 6px offset shadows, giant uppercase display type, cards with no radius, scrolling marquee band. Art direction: Loud and confident; red only for sold-out and deadlines. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sites-conference-event.webp",
            styleName: "Brutal Yellow",
            swatches: ["#FFE14D", "#0A0A0A", "#0A0A0A", "#FF3B30"],
            placeholder: ["#FFE14D", "#FFFFFF"],
            accent: "#0A0A0A"
        ),
        ArtifactTemplate(
            id: "sites-docs-site",
            mode: .website,
            title: "Documentation Site",
            description: "Docs layout with sidebar, search and code blocks",
            prompt: "Build a documentation website for a developer product. Ask me for the product name and the sections to document, or read the attached README. Layout: top bar with logo, search box and version switcher; left sidebar navigation with grouped sections; main article with breadcrumbs, H1, intro, steps, tabbed code blocks with copy buttons, callouts (note, warning) and a prev/next footer; right-hand 'On this page' table of contents. Light and dark themes, responsive with a collapsible sidebar.\n\nVisual style \"Plex Docs\": use exactly this palette (background #FFFFFF, surface #F6F7F9, text #16181D, muted #5F6673, accent #0F62FE, secondary accent #24A148); headings in IBM Plex Sans, body in IBM Plex Sans, code/labels in IBM Plex Mono. Layout: Three columns (260px sidebar, 720px article, 220px TOC), 15px body at 1.7 line height, dark code blocks, callouts with a colored left border. Art direction: Optimized for reading and copying code. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sites-docs-site.webp",
            styleName: "Plex Docs",
            swatches: ["#FFFFFF", "#16181D", "#0F62FE", "#24A148"],
            placeholder: ["#FFFFFF", "#F6F7F9"],
            accent: "#0F62FE"
        ),
        ArtifactTemplate(
            id: "sites-admin-dashboard",
            mode: .website,
            title: "Admin Dashboard",
            description: "Analytics web app with sidebar, KPIs and charts",
            prompt: "Build a responsive admin analytics dashboard as a web app. Ask me what the product tracks, or use an e-commerce example. Layout: collapsible left sidebar with icons and labels; top bar with search, date-range picker and avatar; four KPI cards with sparkline and delta; a large revenue area chart; a traffic-source donut; a recent orders table with status badges and pagination; an activity feed. Use Chart.js from a CDN, Tailwind for styling, and realistic sample data.\n\nVisual style \"Console Slate\": use exactly this palette (background #F8FAFC, surface #FFFFFF, text #0F172A, muted #64748B, accent #6366F1, secondary accent #10B981); headings in Inter, body in Inter. Layout: 240px sidebar on dark slate (#0F172A), white cards with 1px slate-200 borders and 12px radius, 24px grid gaps, charts in indigo with emerald positive deltas. Art direction: Dense but calm; everything aligned to an 8px grid. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sites-admin-dashboard.webp",
            styleName: "Console Slate",
            swatches: ["#F8FAFC", "#0F172A", "#6366F1", "#10B981"],
            placeholder: ["#F8FAFC", "#FFFFFF"],
            accent: "#6366F1"
        ),
        ArtifactTemplate(
            id: "sheets-saas-kpi-dashboard",
            mode: .data,
            title: "SaaS KPI Dashboard",
            description: "MRR, churn and growth with live formulas and charts",
            prompt: "Build a SaaS KPI dashboard workbook. Ask me for monthly data (new MRR, expansion, churned MRR, customers) or paste/attach it; otherwise seed 12 months of realistic sample data. Sheets: Data (one row per month, inputs in blue font); Metrics (formulas for total MRR, net new MRR, growth %, logo churn %, NRR, ARPA, all referencing Data); Dashboard (KPI tiles for the latest month with month-over-month delta, an MRR line chart, a stacked bar of new/expansion/churn, conditional formatting red/green on deltas). Freeze header rows and format currency and percentages.\n\nVisual style \"Ledger Blue\": use exactly this palette (background #FFFFFF, surface #EEF2FF, text #1E1B4B, muted #6B7280, accent #4F46E5, secondary accent #16A34A); headings in Inter, body in Inter. Layout: Header rows filled indigo with white bold text, inputs in blue font, formulas in black, thin grey gridlines, KPI tiles as merged cells on the light indigo surface. Art direction: Finance-model conventions: inputs vs formulas visually distinct. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sheets-saas-kpi-dashboard.webp",
            styleName: "Ledger Blue",
            swatches: ["#FFFFFF", "#1E1B4B", "#4F46E5", "#16A34A"],
            placeholder: ["#FFFFFF", "#EEF2FF"],
            accent: "#4F46E5"
        ),
        ArtifactTemplate(
            id: "sheets-startup-financial-model",
            mode: .data,
            title: "Startup Financial Model",
            description: "24-month revenue, burn and runway model",
            prompt: "Build a 24-month startup financial model. Ask me for starting cash, pricing, expected customer growth, headcount plan and major costs. Sheets: Assumptions (all inputs in one place, blue font, with notes); Revenue (customers, ARPA, MRR by month); Headcount (roles, start month, salary, loaded cost); Opex (categories by month); P&L summary; Cash (monthly burn, ending cash, runway in months with a formula that flags the month cash goes negative). Add a burn-vs-revenue line chart. Every number outside Assumptions must be a formula.\n\nVisual style \"Banker Green\": use exactly this palette (background #FFFFFF, surface #ECFDF5, text #064E3B, muted #6B7280, accent #047857, secondary accent #DC2626); headings in Inter, body in Inter. Layout: Months across columns, line items down rows, section subtotals bold with a top border, negative values red in parentheses, assumptions highlighted on the mint surface. Art direction: Model hygiene: one input per cell, no hard-coded numbers in formulas. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sheets-startup-financial-model.webp",
            styleName: "Banker Green",
            swatches: ["#FFFFFF", "#064E3B", "#047857", "#DC2626"],
            placeholder: ["#FFFFFF", "#ECFDF5"],
            accent: "#047857"
        ),
        ArtifactTemplate(
            id: "sheets-project-tracker",
            mode: .data,
            title: "Project Tracker",
            description: "Tasks, owners and a conditional-format Gantt",
            prompt: "Build a project tracker workbook. Ask me for the project, phases and tasks (or seed a realistic 20-task plan). Columns: ID, Task, Phase, Owner, Status (dropdown: Not started / In progress / Blocked / Done), Priority, Start, End, Duration (formula), % complete. To the right, a week-by-week Gantt grid filled by conditional formatting from Start/End, colored by Status. Add a Summary sheet with counts by status and owner (COUNTIFS) and a progress bar chart.\n\nVisual style \"Planner\": use exactly this palette (background #FFFFFF, surface #F5F3FF, text #1F2937, muted #6B7280, accent #7C3AED, secondary accent #F59E0B); headings in Inter, body in Inter. Layout: Frozen header row and first two columns, status chips via conditional fill (grey, violet, amber, green), Gantt cells narrow and borderless, weekend columns shaded. Art direction: Readable at a glance on a projector. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sheets-project-tracker.webp",
            styleName: "Planner",
            swatches: ["#FFFFFF", "#1F2937", "#7C3AED", "#F59E0B"],
            placeholder: ["#FFFFFF", "#F5F3FF"],
            accent: "#7C3AED"
        ),
        ArtifactTemplate(
            id: "sheets-budget-planner",
            mode: .data,
            title: "Budget vs Actual",
            description: "Monthly budget with variance and category charts",
            prompt: "Build a monthly budget-vs-actual workbook for a team or household. Ask me for income, categories and budgets, or seed sensible examples. Sheets: Budget (category, monthly budget); Transactions (date, description, category dropdown, amount); Summary (per category: budget, actual via SUMIFS, variance, variance %, with red/green conditional formatting and data bars); a doughnut chart of spend by category and a budget-vs-actual clustered bar chart.\n\nVisual style \"Sandstone\": use exactly this palette (background #FFFFFF, surface #FBF6EE, text #3F2D20, muted #8C7B6B, accent #C2410C, secondary accent #15803D); headings in Inter, body in Inter. Layout: Warm header fill, currency formats, data bars for actuals, totals row with double top border. Art direction: Friendly but exact. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sheets-budget-planner.webp",
            styleName: "Sandstone",
            swatches: ["#FFFFFF", "#3F2D20", "#C2410C", "#15803D"],
            placeholder: ["#FFFFFF", "#FBF6EE"],
            accent: "#C2410C"
        ),
        ArtifactTemplate(
            id: "sheets-sales-pipeline",
            mode: .data,
            title: "Sales Pipeline",
            description: "Deals by stage with weighted forecast",
            prompt: "Build a sales pipeline workbook. Ask me for deals or seed 25 realistic ones. Deals sheet: Company, Contact, Owner, Stage (dropdown: Lead / Qualified / Proposal / Negotiation / Won / Lost), Amount, Probability (lookup from stage), Weighted (formula), Close date, Next step. Pipeline sheet: totals and weighted totals by stage and by owner (SUMIFS), a funnel-style bar chart by stage, and this-quarter forecast. Highlight overdue close dates in red.\n\nVisual style \"Pipeline Teal\": use exactly this palette (background #FFFFFF, surface #F0FDFA, text #134E4A, muted #6B7280, accent #0D9488, secondary accent #E11D48); headings in Inter, body in Inter. Layout: Teal header row, stage column color-coded, currency without decimals, filters on the header row. Art direction: Built for weekly pipeline reviews. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sheets-sales-pipeline.webp",
            styleName: "Pipeline Teal",
            swatches: ["#FFFFFF", "#134E4A", "#0D9488", "#E11D48"],
            placeholder: ["#FFFFFF", "#F0FDFA"],
            accent: "#0D9488"
        ),
        ArtifactTemplate(
            id: "sheets-content-calendar",
            mode: .data,
            title: "Content Calendar",
            description: "Channel-by-week editorial plan with status",
            prompt: "Build a content calendar workbook for the next quarter. Ask me for channels, cadence and themes, or seed a plan. Calendar sheet: Week, Publish date, Channel (dropdown), Format, Title, Owner, Status (Idea / Drafting / Review / Scheduled / Published), Link, Notes; color rows by channel. Overview sheet: posts per channel per month (COUNTIFS) as a heatmap with color scales, and a status breakdown chart.\n\nVisual style \"Studio Pink\": use exactly this palette (background #FFFFFF, surface #FDF2F8, text #3B0A2A, muted #7E6573, accent #DB2777, secondary accent #2563EB); headings in Inter, body in Inter. Layout: Pink header, alternating week bands, channel colors as soft fills, color-scale heatmap on the overview. Art direction: Playful color, strict structure. Keep this one design system consistent across every page/section; if a design system is selected, its tokens take precedence.",
            preview: "/images/templates/catalog/sheets-content-calendar.webp",
            styleName: "Studio Pink",
            swatches: ["#FFFFFF", "#3B0A2A", "#DB2777", "#2563EB"],
            placeholder: ["#FFFFFF", "#FDF2F8"],
            accent: "#DB2777"
        ),
    ]
}
