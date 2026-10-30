import AVFoundation
import SwiftUI
import UIKit

// -----------------------------------------------------------------------------
// The top half of the "Meet your bots" card on the phone (web MeetYourBotsCard
// `.mb-vis`, stacked): the bots network on the first step, the team grid, the
// main bot's stage, the routine calendar and the first-job hand-off.
// Colours are the card's `--bl-*` tokens (white in light, graphite in dark).
// -----------------------------------------------------------------------------

enum MeetPalette {
    static let ink = SetupPalette.ink
    static let sub = SetupPalette.sub
    static let mut = SetupPalette.mut
    static let line = SetupPalette.line
    static let lineDash = SetupPalette.lineDash
    static let fill = SetupPalette.fill
    static let fill2 = SetupPalette.fill2
    static let bg = SetupPalette.bg
    static let soft = Color(UIColor { $0.userInterfaceStyle == .dark ? UIColor(Color(hex: "#18181A")) : UIColor(Color(hex: "#FAFAFB")) })
    static let coral = SetupPalette.coral
    static let term = SetupPalette.term
    static func mono(_ size: CGFloat, _ weight: Font.Weight) -> Font { .system(size: size, weight: weight, design: .monospaced) }
}

// MARK: - Tiles

/// A look's tile: the A:// block mark, Gizzi, a cast sprite or a vendor's own character (web MeetTile).
struct MeetLookTile: View {
    let id: String
    var size: CGFloat = 40
    var live = false

    struct Vendor: Identifiable, Sendable { let id: String; let label: String; let field: String; let art: String }

    /// The vendor looks, captioned with what people call the product (web bot-looks + MainBotPanel CAPTION).
    static let vendors: [Vendor] = [
        Vendor(id: "claude", label: "Claude", field: "#2B2240", art: "MeetVendorClaude"),
        Vendor(id: "duet", label: "Duet", field: "#F0EEE6", art: "MeetVendorDuet"),
        Vendor(id: "dots", label: "ChatGPT", field: "#5B3FD9", art: "MeetVendorChatGPT"),
        Vendor(id: "grok", label: "Grok", field: "#141413", art: "MeetVendorGrok"),
        Vendor(id: "muse", label: "Meta AI", field: "#2D4BD8", art: "MeetVendorMeta"),
    ]

    var body: some View {
        let radius = size * 0.28
        ZStack {
            RoundedRectangle(cornerRadius: radius, style: .continuous).fill(Color(hex: field))
            switch id {
            case "a":
                AllternitOrbView(state: live ? .working : .idle, size: size * 0.52)
            case "gizzi":
                SetupGizzi(size: size * 0.78, emotion: live ? .pleased : .focused).offset(y: size * 0.04)
            default:
                if let vendor = Self.vendors.first(where: { $0.id == id }) {
                    Image(vendor.art).resizable().interpolation(.high).scaledToFit()
                        .frame(width: size * 0.8, height: size * 0.8).offset(y: size * 0.05)
                } else if let url = MeetBotCatalog.spriteURL(for: id) {
                    SpriteAvatar(url: url, size: size * 0.92, live: live)
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: radius, style: .continuous))
        .accessibilityHidden(true)
    }

    private var field: String {
        if id == "a" { return "#141413" }
        if let v = Self.vendors.first(where: { $0.id == id }) { return v.field }
        return MeetBotCatalog.shared.looks.first(where: { $0.id == id })?.field ?? "#141413"
    }
}

// MARK: - Start: the bots network

/// Bots hand a job to each other (web TeamNetwork): the web's own animation,
/// recorded as a 16 s loop, with the speech bubbles drawn natively on top.
/// Reduced motion shows the web's still frame and no bubbles.
struct MeetNetworkView: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var t: Double = 0

    /// The loop's speech (web SAYS), at the bubble anchors of a 440×340 frame.
    private static let says: [(from: Double, to: Double, name: String, text: String, x: CGFloat, bottom: CGFloat)] = [
        (0.9, 2.6, "Claude", "A new client brief came in. Dots, can you draft the plan?", 12, 76),
        (2.7, 4.4, "Dots", "Plan drafted. Grok, can you check the numbers?", 208, 76),
        (4.5, 6.2, "Grok", "Two numbers were off. Fixed. Muse, send it for sign-off?", 208, 241.5),
        (6.3, 8.4, "Muse", "Sent to you on WhatsApp for approval.", 12, 237.7),
    ]

    var body: some View {
        GeometryReader { geo in
            let scale = max(geo.size.width / 440, geo.size.height / 340)
            let ox = (geo.size.width - 440 * scale) / 2
            let oy = (geo.size.height - 340 * scale) / 2
            ZStack(alignment: .topLeading) {
                Color(hex: "#FAFAFB")
                if reduceMotion {
                    Image("MeetTeamNetworkStill").resizable().scaledToFill()
                        .frame(width: geo.size.width, height: geo.size.height).clipped()
                } else {
                    MeetLoopingVideo(name: "meet-team-network", time: $t)
                        .frame(width: geo.size.width, height: geo.size.height)
                    if let say = Self.says.first(where: { t >= $0.from && t <= $0.to }) {
                        bubble(say.name, say.text)
                            .frame(width: min(220, geo.size.width - 24), alignment: .leading)
                            .alignmentGuide(.top) { d in d[.bottom] }
                            .offset(x: min(geo.size.width - min(220, geo.size.width - 24) - 12, max(12, ox + say.x * scale)),
                                    y: max(76, oy + say.bottom * scale))
                            .transition(.opacity.combined(with: .scale(scale: 0.94)))
                            .id(say.text)
                    }
                }
            }
            .animation(.easeOut(duration: 0.2), value: Self.says.firstIndex(where: { t >= $0.from && t <= $0.to }))
        }
        .clipped()
        .accessibilityElement()
        .accessibilityLabel("Bots handing a job to each other: Claude, Dots, Grok and Muse pass work along, then more bots join the team.")
    }

    private func bubble(_ name: String, _ text: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(name).font(.system(size: 11)).foregroundStyle(Color(hex: "#8A8A93"))
            Text(text).font(.system(size: 12.5)).foregroundStyle(Color(hex: "#141413")).lineSpacing(1.5)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color.white, in: RoundedRectangle(cornerRadius: 16))
        .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color.black.opacity(0.1)))
        .shadow(color: Color(white: 0.06).opacity(0.25), radius: 12, y: 8)
    }
}

/// A muted, looping bundled video that reports its loop time.
struct MeetLoopingVideo: UIViewRepresentable {
    let name: String
    @Binding var time: Double

    final class PlayerView: UIView {
        override class var layerClass: AnyClass { AVPlayerLayer.self }
        var playerLayer: AVPlayerLayer { layer as! AVPlayerLayer }
        var looper: AVPlayerLooper?
        var observer: Any?
    }

    func makeUIView(context: Context) -> PlayerView {
        let view = PlayerView()
        view.backgroundColor = UIColor(Color(hex: "#FAFAFB"))
        guard let url = Bundle.main.url(forResource: name, withExtension: "mp4") else { return view }
        let player = AVQueuePlayer()
        player.isMuted = true
        player.preventsDisplaySleepDuringVideoPlayback = false
        view.looper = AVPlayerLooper(player: player, templateItem: AVPlayerItem(url: url))
        view.playerLayer.player = player
        view.playerLayer.videoGravity = .resizeAspectFill
        let binding = $time
        view.observer = player.addPeriodicTimeObserver(forInterval: CMTime(value: 1, timescale: 10), queue: .main) { t in
            let seconds = t.seconds.truncatingRemainder(dividingBy: 16)
            MainActor.assumeIsolated { binding.wrappedValue = seconds }
        }
        player.play()
        return view
    }

    func updateUIView(_ uiView: PlayerView, context: Context) {}

    static func dismantleUIView(_ uiView: PlayerView, coordinator: ()) {
        if let observer = uiView.observer { uiView.playerLayer.player?.removeTimeObserver(observer) }
        uiView.playerLayer.player?.pause()
    }
}

// MARK: - The team grid

/// Your team (web TeamGrid): the main bot in the middle (a dashed "Main bot"
/// until one is chosen), the picked bots around it on dotted links. Drag a bot
/// to move it; tap its × to take it off.
struct MeetTeamGrid: View {
    let members: [String]
    let catalog: MeetBotCatalog
    /// The main bot's tile, or nil for the dashed placeholder.
    let center: AnyView?
    let centerName: String
    var size: CGFloat = 52
    var label: String?
    var lockedIds: Set<String> = []
    var onRemove: (String) -> Void

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var spots: [String: CGPoint] = [:]
    @State private var dragging: (id: String, offset: CGSize)?

    static let slots: [CGPoint] = [
        .init(x: 0.5, y: 0.17), .init(x: 0.82, y: 0.32), .init(x: 0.82, y: 0.7), .init(x: 0.5, y: 0.85),
        .init(x: 0.18, y: 0.7), .init(x: 0.18, y: 0.32), .init(x: 0.68, y: 0.12), .init(x: 0.93, y: 0.5),
        .init(x: 0.68, y: 0.9), .init(x: 0.32, y: 0.9), .init(x: 0.07, y: 0.5), .init(x: 0.32, y: 0.12),
    ]

    private func spot(_ id: String, _ index: Int) -> CGPoint {
        if let s = spots[id] { return s }
        if index < Self.slots.count { return Self.slots[index] }
        let n = Double(index - Self.slots.count)
        let angle = n * 2.399963, r = 0.22 + 0.2 * (n.truncatingRemainder(dividingBy: 5) / 5)
        return CGPoint(x: min(0.93, max(0.07, 0.5 + cos(angle) * r * 1.3)), y: min(0.9, max(0.1, 0.5 + sin(angle) * r)))
    }

    var body: some View {
        GeometryReader { geo in
            let w = geo.size.width, h = geo.size.height
            let mid = CGPoint(x: w / 2, y: h / 2)
            ZStack(alignment: .topLeading) {
                MeetPalette.soft
                MeetBlockPattern()
                // links
                Path { p in
                    for (i, id) in members.enumerated() {
                        let s = spot(id, i)
                        p.move(to: mid)
                        p.addLine(to: CGPoint(x: s.x * w, y: s.y * h))
                    }
                }
                .stroke(MeetPalette.ink.opacity(0.3), style: StrokeStyle(lineWidth: 1.6, lineCap: .round, dash: [0.1, 7]))
                // centre
                VStack(spacing: 4) {
                    if let center {
                        center
                    } else {
                        Text("Main\nbot")
                            .font(.system(size: 11))
                            .multilineTextAlignment(.center)
                            .foregroundStyle(MeetPalette.mut)
                            .frame(width: size * 1.3, height: size * 1.3)
                            .overlay(RoundedRectangle(cornerRadius: 16).stroke(MeetPalette.lineDash, style: StrokeStyle(lineWidth: 2, dash: [5, 4])))
                    }
                    name(center == nil ? " " : centerName, bold: true)
                }
                .position(x: mid.x, y: mid.y + 8)
                ForEach(Array(members.enumerated()), id: \.element) { i, id in
                    if let t = catalog.template(id) {
                        let s = spot(id, i)
                        let drag = dragging?.id == id ? dragging!.offset : .zero
                        node(t, locked: lockedIds.contains(id))
                            .scaleEffect(dragging?.id == id ? 1.12 : 1)
                            .position(x: s.x * w + drag.width, y: s.y * h + drag.height + 8)
                            .zIndex(dragging?.id == id ? 40 : 10)
                            .transition(reduceMotion ? .opacity : .scale.combined(with: .opacity))
                            .gesture(
                                DragGesture(minimumDistance: 4)
                                    .onChanged { v in dragging = (id, v.translation) }
                                    .onEnded { v in
                                        let nx = min(0.93, max(0.07, (s.x * w + v.translation.width) / w))
                                        let ny = min(0.9, max(0.1, (s.y * h + v.translation.height) / h))
                                        spots[id] = CGPoint(x: nx, y: ny)
                                        dragging = nil
                                    }
                            )
                    }
                }
                if let label {
                    Text(label)
                        .font(.system(size: 12))
                        .foregroundStyle(MeetPalette.sub)
                        .frame(width: w - 24)
                        .position(x: w / 2, y: h - 16)
                        .allowsHitTesting(false)
                }
            }
            .animation(reduceMotion ? nil : .spring(response: 0.35, dampingFraction: 0.6), value: members)
        }
        .clipped()
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Your team")
    }

    private func node(_ t: MeetBotTemplate, locked: Bool) -> some View {
        VStack(spacing: 4) {
            MeetTemplateTile(color: t.color, spriteURL: t.spriteURL, size: size, live: true, label: t.id)
            name(t.name, bold: false).frame(maxWidth: size * 1.9)
        }
        .overlay(alignment: .topTrailing) {
            if !locked {
                Button { onRemove(t.id) } label: {
                    Image(systemName: "xmark").font(.system(size: 9, weight: .bold))
                        .foregroundStyle(MeetPalette.sub)
                        .frame(width: 22, height: 22)
                        .background(MeetPalette.bg, in: Circle())
                        .shadow(color: .black.opacity(0.25), radius: 2, y: 1)
                }
                .buttonStyle(.plain)
                .offset(x: 8, y: -8)
                .accessibilityLabel("Remove \(t.name)")
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(t.name), on your team")
    }

    private func name(_ text: String, bold: Bool) -> some View {
        Text(text)
            .font(.system(size: 11, weight: bold ? .semibold : .medium))
            .foregroundStyle(MeetPalette.ink)
            .multilineTextAlignment(.center)
            .lineLimit(2)
            .padding(.horizontal, 4)
            .background(MeetPalette.bg.opacity(0.82), in: RoundedRectangle(cornerRadius: 4))
    }
}

/// The 34 pt square pattern under the grid (web `.mb-sq`).
struct MeetBlockPattern: View {
    var body: some View {
        Canvas { ctx, sz in
            var path = Path()
            var y: CGFloat = 0
            while y < sz.height {
                var x: CGFloat = 0
                while x < sz.width {
                    path.addRoundedRect(in: CGRect(x: x + 4, y: y + 4, width: 26, height: 26), cornerSize: CGSize(width: 6, height: 6))
                    x += 34
                }
                y += 34
            }
            ctx.fill(path, with: .color(MeetPalette.fill))
        }
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }
}

// MARK: - Main bot stage

/// The picked main bot on its field (web BotLookStage), with its status chip.
struct MeetMainStage: View {
    let field: Color
    let tile: AnyView
    let label: String

    var body: some View {
        ZStack(alignment: .bottomLeading) {
            field
            tile.frame(maxWidth: .infinity, maxHeight: .infinity)
            HStack(spacing: 6) {
                Circle().fill(MeetPalette.coral).frame(width: 6, height: 6)
                Text(label).font(MeetPalette.mono(11.5, .semibold)).foregroundStyle(.white)
            }
            .padding(.horizontal, 10)
            .frame(height: 26)
            .background(Color.black.opacity(0.35), in: RoundedRectangle(cornerRadius: 8))
            .padding(14)
        }
        .clipped()
        .accessibilityHidden(true)
    }
}

// MARK: - Routines: the calendar

/// When the team's routines run (web RoutineScheduler, compact): Day
/// (default) and Week are 6a–10p hour grids, Month shows dots per day and a
/// tap lists the day. Tap an empty time to add a routine there, tap one to
/// change it, drag it up or down to move it (15-minute steps). Frequent
/// routines ("every 2 h from 8a to 6p") draw as one band.
struct MeetRoutineCalendar: View {
    struct Item: Identifiable {
        let id: String
        let title: String
        let rule: RoutineRule?
        let enabled: Bool
        let color: Color
        let avatar: AnyView
    }

    enum Mode: String, CaseIterable { case day, week, month }

    let items: [Item]
    var onNew: (_ day: Date, _ minute: Int) -> Void
    var onEdit: (_ id: String) -> Void
    var onMove: (_ id: String, _ minute: Int) -> Void

    @State private var mode: Mode = .day
    @State private var anchor = Calendar.current.startOfDay(for: Date())
    @State private var listedDay: ListedDay?

    static let h0 = 6
    static let h1 = 22
    private var px: CGFloat { mode == .day ? 44 : 40 }
    private var cal: Calendar { Calendar.current }
    private var today: Date { cal.startOfDay(for: Date()) }
    /// Paused routines don't show (web `live`).
    private var live: [Item] { items.filter { $0.enabled && $0.rule != nil } }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            toolbar
            if mode == .month { month } else { grid }
            Text("Tap an empty time to add a routine · tap one to change it · drag it to move it")
                .font(.system(size: 11))
                .foregroundStyle(MeetPalette.mut)
        }
        .padding(12)
        .background(MeetPalette.bg)
        .sheet(item: $listedDay) { listed in
            MeetDayList(day: listed.day, items: dayItems(listed.day), onEdit: { id in
                listedDay = nil
                onEdit(id)
            }, onOpenDay: {
                listedDay = nil
                open(listed.day)
            })
            .presentationDetents([.medium])
        }
    }

    // MARK: Toolbar

    private var toolbar: some View {
        HStack(spacing: 8) {
            HStack(spacing: 4) {
                Button { step(-1) } label: { Image(systemName: "chevron.left").font(.system(size: 11, weight: .semibold)) }
                    .buttonStyle(MeetCalButtonStyle(square: true)).accessibilityLabel("Previous")
                Button("Today") { anchor = today }.buttonStyle(MeetCalButtonStyle())
                Button { step(1) } label: { Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)) }
                    .buttonStyle(MeetCalButtonStyle(square: true)).accessibilityLabel("Next")
            }
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(MeetPalette.ink)
                .lineLimit(1)
                .truncationMode(.tail)
                .frame(maxWidth: .infinity, alignment: .leading)
                .accessibilityAddTraits(.updatesFrequently)
            HStack(spacing: 0) {
                ForEach(Mode.allCases, id: \.self) { m in
                    Button { mode = m } label: {
                        Text(m.rawValue.capitalized)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(mode == m ? MeetPalette.ink : MeetPalette.mut)
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(mode == m ? MeetPalette.bg : .clear, in: RoundedRectangle(cornerRadius: 6))
                            .shadow(color: .black.opacity(mode == m ? 0.06 : 0), radius: 1, y: 1)
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(mode == m ? .isSelected : [])
                }
            }
            .padding(2)
            .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 8))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Calendar view")
        }
    }

    private var title: String {
        switch mode {
        case .day: return anchor.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())
        case .month: return anchor.formatted(.dateTime.month(.wide).year())
        case .week:
            let d = days
            return "\(d[0].formatted(.dateTime.month(.abbreviated).day())) – \(d[6].formatted(.dateTime.month(.abbreviated).day()))"
        }
    }

    private func step(_ dir: Int) {
        switch mode {
        case .day: anchor = cal.date(byAdding: .day, value: dir, to: anchor) ?? anchor
        case .week: anchor = cal.date(byAdding: .day, value: 7 * dir, to: anchor) ?? anchor
        case .month:
            let first = cal.date(from: cal.dateComponents([.year, .month], from: anchor)) ?? anchor
            anchor = cal.date(byAdding: .month, value: dir, to: first) ?? anchor
        }
    }

    private func open(_ day: Date) {
        anchor = day
        mode = .day
    }

    // MARK: Days

    private func startOfWeek(_ d: Date) -> Date {
        let dow = cal.component(.weekday, from: d) - 1
        return cal.date(byAdding: .day, value: -((dow + 6) % 7), to: cal.startOfDay(for: d)) ?? d
    }

    private var days: [Date] {
        switch mode {
        case .day: return [anchor]
        case .week:
            let s = startOfWeek(anchor)
            return (0..<7).compactMap { cal.date(byAdding: .day, value: $0, to: s) }
        case .month:
            let first = cal.date(from: cal.dateComponents([.year, .month], from: anchor)) ?? anchor
            let last = cal.date(byAdding: DateComponents(month: 1, day: -1), to: first) ?? first
            let from = startOfWeek(first)
            let to = cal.date(byAdding: .day, value: 7, to: startOfWeek(last)) ?? last
            let n = cal.dateComponents([.day], from: from, to: to).day ?? 35
            return (0..<n).compactMap { cal.date(byAdding: .day, value: $0, to: from) }
        }
    }

    private func runsOn(_ r: RoutineRule, _ d: Date) -> Bool {
        let dow = cal.component(.weekday, from: d) - 1
        switch r.repeatKind {
        case .daily: return true
        case .weekdays, .weekly, .interval: return r.days.contains(dow)
        case .monthly: return (r.dom ?? []).contains(cal.component(.day, from: d))
        }
    }

    /// A day's items in month order (by time).
    private func dayItems(_ d: Date) -> [Item] {
        live.filter { runsOn($0.rule!, d) }.sorted { ($0.rule!.at, $0.title) < ($1.rule!.at, $1.title) }
    }

    // MARK: Day / Week grid

    private var grid: some View {
        VStack(spacing: 0) {
            if mode == .week {
                HStack(spacing: 0) {
                    Color.clear.frame(width: 40)
                    ForEach(days, id: \.self) { d in
                        Button { open(d) } label: {
                            Text("\(String(RoutineRule.dayShort[cal.component(.weekday, from: d) - 1].prefix(1))) \(cal.component(.day, from: d))")
                                .font(.system(size: 11, weight: cal.isDate(d, inSameDayAs: today) ? .semibold : .regular))
                                .foregroundStyle(cal.isDate(d, inSameDayAs: today) ? Color("AccentPrimary") : MeetPalette.sub)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 4)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Open \(d.formatted(.dateTime.weekday(.wide).month(.wide).day()))")
                    }
                }
                .overlay(alignment: .bottom) { Rectangle().fill(MeetPalette.line).frame(height: 1) }
            }
            ScrollViewReader { proxy in
                ScrollView {
                    HStack(alignment: .top, spacing: 0) {
                        hourLabels
                        ForEach(days, id: \.self) { d in column(d, narrow: mode == .week) }
                    }
                }
                .onAppear { proxy.scrollTo(7, anchor: .top) }
                .onChange(of: mode) { _, _ in proxy.scrollTo(7, anchor: .top) }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(MeetPalette.line))
    }

    private var hourLabels: some View {
        ZStack(alignment: .topTrailing) {
            ForEach(Self.h0..<Self.h1, id: \.self) { h in
                Color.clear.frame(height: 1).offset(y: CGFloat(h - Self.h0) * px).id(h)
                if h > Self.h0 {
                    Text(RoutineRuleClock.hour(h * 60))
                        .font(.system(size: 10).monospacedDigit())
                        .foregroundStyle(MeetPalette.mut)
                        .padding(.trailing, 6)
                        .offset(y: CGFloat(h - Self.h0) * px - 6)
                }
            }
        }
        .frame(width: 40, height: CGFloat(Self.h1 - Self.h0) * px, alignment: .topTrailing)
        .accessibilityHidden(true)
    }

    private func column(_ d: Date, narrow: Bool) -> some View {
        let list = live.filter { runsOn($0.rule!, d) }
        let bands = list.filter { $0.rule!.repeatKind == .interval }
        let singles = list.filter { $0.rule!.repeatKind != .interval }.sorted { $0.rule!.at < $1.rule!.at }
        // Side-by-side lanes for routines within 30 minutes of each other.
        var lanes: [[Item]] = []
        var laneOf: [String: Int] = [:]
        for r in singles {
            var k = 0
            while k < lanes.count, lanes[k].contains(where: { abs($0.rule!.at - r.rule!.at) < 30 }) { k += 1 }
            if k == lanes.count { lanes.append([]) }
            lanes[k].append(r)
            laneOf[r.id] = k
        }
        let laneCount = max(1, lanes.count) + (!bands.isEmpty && !narrow ? 1 : 0)
        let height = CGFloat(Self.h1 - Self.h0) * px
        let isToday = cal.isDate(d, inSameDayAs: today)
        let nowMin = cal.component(.hour, from: Date()) * 60 + cal.component(.minute, from: Date())

        return GeometryReader { geo in
            let w = geo.size.width
            ZStack(alignment: .topLeading) {
                // Empty time: tap adds a routine at the half hour.
                Color.clear
                    .contentShape(Rectangle())
                    .onTapGesture { loc in
                        let raw = Int((loc.y / px * 60 / 30).rounded()) * 30 + Self.h0 * 60
                        onNew(d, min(Self.h1 * 60 - 30, max(Self.h0 * 60, raw)))
                    }
                    .accessibilityElement()
                    .accessibilityLabel(d.formatted(.dateTime.weekday(.wide).month(.wide).day()))
                    .accessibilityHint("Adds a routine")
                ForEach(0..<(Self.h1 - Self.h0), id: \.self) { h in
                    Rectangle().fill(MeetPalette.line.opacity(0.6)).frame(height: 1)
                        .offset(y: CGFloat(h) * px)
                        .allowsHitTesting(false)
                }
                if isToday, nowMin > Self.h0 * 60, nowMin < Self.h1 * 60 {
                    ZStack(alignment: .leading) {
                        Rectangle().fill(Color("AccentPrimary")).frame(height: 2)
                        Circle().fill(Color("AccentPrimary")).frame(width: 8, height: 8).offset(x: -4)
                    }
                    .offset(y: CGFloat(nowMin - Self.h0 * 60) / 60 * px - 1)
                    .allowsHitTesting(false)
                    .zIndex(20)
                }
                ForEach(bands) { r in
                    band(r, width: narrow ? w * 0.34 - 2 : w / CGFloat(laneCount) - 4, narrow: narrow)
                        .offset(x: w - (narrow ? w * 0.34 - 2 : w / CGFloat(laneCount) - 4) - 2,
                                y: CGFloat(max(r.rule!.at, Self.h0 * 60) - Self.h0 * 60) / 60 * px)
                        .zIndex(10)
                }
                ForEach(singles) { r in
                    if r.rule!.at >= Self.h0 * 60, r.rule!.at < Self.h1 * 60 {
                        let lane = CGFloat(laneOf[r.id] ?? 0)
                        MeetCalendarChip(item: r, px: px, narrow: narrow,
                                         width: narrow ? w - 4 : w / CGFloat(laneCount) - 6,
                                         onEdit: onEdit, onMove: onMove)
                            .offset(x: narrow ? 2 : lane * w / CGFloat(laneCount) + 3,
                                    y: CGFloat(r.rule!.at - Self.h0 * 60) / 60 * px)
                            .zIndex(10)
                    }
                }
            }
        }
        .frame(height: height)
        .frame(maxWidth: .infinity)
        .overlay(alignment: .leading) { Rectangle().fill(MeetPalette.line).frame(width: 1) }
    }

    /// One band for a frequent routine: dashed border, a tick per run.
    private func band(_ r: Item, width: CGFloat, narrow: Bool) -> some View {
        let rule = r.rule!
        let every = max(15, rule.every ?? 60)
        let top = CGFloat(max(rule.at, Self.h0 * 60) - Self.h0 * 60) / 60 * px
        let bottom = CGFloat(min((rule.until ?? rule.at) + 30, Self.h1 * 60) - Self.h0 * 60) / 60 * px
        let h = max(px / 2, bottom - top)
        let tick = CGFloat(every) / 60 * px
        return Button { onEdit(r.id) } label: {
            ZStack(alignment: .topLeading) {
                RoundedRectangle(cornerRadius: 8).fill(r.color.opacity(0.10))
                VStack(spacing: 0) {
                    ForEach(0..<max(1, Int(h / max(tick, 1))), id: \.self) { _ in
                        VStack(spacing: 0) {
                            Spacer(minLength: 0)
                            Rectangle().fill(r.color.opacity(0.4)).frame(height: 3)
                        }
                        .frame(height: tick)
                    }
                    Spacer(minLength: 0)
                }
                .clipShape(RoundedRectangle(cornerRadius: 8))
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(r.color.opacity(0.55), style: StrokeStyle(lineWidth: 1, dash: [4, 3]))
                if !narrow {
                    HStack(alignment: .top, spacing: 4) {
                        r.avatar.frame(width: 14, height: 14).clipShape(RoundedRectangle(cornerRadius: 3))
                        VStack(alignment: .leading, spacing: 0) {
                            Text(r.title).lineLimit(2)
                            Text("every \(RoutineRuleClock.every(every))").opacity(0.75)
                        }
                    }
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(meetTintText(r.color))
                    .padding(.horizontal, 6)
                    .padding(.top, 4)
                }
            }
            .frame(width: max(8, width), height: h)
        }
        .buttonStyle(.plain)
        .accessibilityLabel("\(r.title), every \(RoutineRuleClock.every(every)). Tap to edit.")
    }

    // MARK: Month

    private var month: some View {
        let cols = Array(repeating: GridItem(.flexible(), spacing: 4), count: 7)
        return ScrollView {
            LazyVGrid(columns: cols, spacing: 4) {
                ForEach(["M", "T", "W", "T", "F", "S", "S"].indices, id: \.self) { i in
                    Text(["M", "T", "W", "T", "F", "S", "S"][i])
                        .font(.system(size: 11))
                        .foregroundStyle(MeetPalette.mut)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 4)
                        .accessibilityHidden(true)
                }
                ForEach(days, id: \.self) { d in monthCell(d) }
            }
        }
    }

    private func monthCell(_ d: Date) -> some View {
        let list = dayItems(d)
        let isToday = cal.isDate(d, inSameDayAs: today)
        let inMonth = cal.component(.month, from: d) == cal.component(.month, from: anchor)
        let label = d.formatted(.dateTime.weekday(.wide).month(.wide).day())
        return Button {
            if list.isEmpty { open(d) } else { listedDay = ListedDay(day: d) }
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text("\(cal.component(.day, from: d))")
                    .font(.system(size: 12, weight: isToday ? .semibold : .regular).monospacedDigit())
                    .foregroundStyle(isToday ? Color("AccentPrimary") : MeetPalette.sub)
                    .padding(.horizontal, 4)
                if !list.isEmpty {
                    HStack(spacing: 2) {
                        ForEach(list.prefix(3)) { r in Circle().fill(r.color).frame(width: 8, height: 8) }
                        if list.count > 3 {
                            Text("+\(list.count - 3)").font(.system(size: 10).monospacedDigit()).foregroundStyle(MeetPalette.mut)
                        }
                    }
                    .padding(.horizontal, 4)
                }
                Spacer(minLength: 0)
            }
            .padding(4)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .topLeading)
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(isToday ? Color("AccentPrimary") : MeetPalette.line))
            .opacity(inMonth ? 1 : 0.5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(list.isEmpty
            ? "\(label), nothing scheduled. Open the day"
            : "\(label), \(list.count) item\(list.count == 1 ? "" : "s"). Show the list")
    }

    struct ListedDay: Identifiable {
        let day: Date
        var id: Date { day }
    }
}

/// A timed block in Day / Week: name and time; drag it up or down to move it.
private struct MeetCalendarChip: View {
    let item: MeetRoutineCalendar.Item
    let px: CGFloat
    let narrow: Bool
    let width: CGFloat
    var onEdit: (String) -> Void
    var onMove: (String, Int) -> Void

    @State private var drag: CGFloat = 0
    @State private var dragging = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let at = item.rule?.at ?? 0
        HStack(spacing: 4) {
            if !narrow {
                item.avatar.frame(width: 16, height: 16).clipShape(RoundedRectangle(cornerRadius: 4))
            }
            Text(item.title).font(.system(size: 11, weight: .semibold)).lineLimit(1).frame(maxWidth: .infinity, alignment: .leading)
            Text(RoutineRuleClock.short(at)).font(.system(size: 10.5).monospacedDigit()).opacity(0.75)
        }
        .foregroundStyle(meetTintText(item.color))
        .padding(.leading, 4)
        .padding(.trailing, 6)
        .frame(width: max(8, width), height: max(22, px / 2 - 2))
        .background(item.color.opacity(0.18), in: RoundedRectangle(cornerRadius: 6))
        .overlay(alignment: .leading) {
            UnevenRoundedRectangle(topLeadingRadius: 6, bottomLeadingRadius: 6).fill(item.color).frame(width: 3)
        }
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .scaleEffect(dragging ? 1.03 : 1)
        .shadow(color: .black.opacity(dragging ? 0.35 : 0), radius: 12, y: 10)
        .offset(y: drag)
        .zIndex(dragging ? 30 : 0)
        .contentShape(Rectangle())
        .onTapGesture { onEdit(item.id) }
        .gesture(
            DragGesture(minimumDistance: 6)
                .onChanged { v in
                    dragging = true
                    drag = v.translation.height
                }
                .onEnded { v in
                    let delta = Int((v.translation.height / px * 60 / 15).rounded()) * 15
                    withAnimation(reduceMotion ? nil : .easeOut(duration: 0.2)) {
                        dragging = false
                        drag = 0
                    }
                    if delta != 0 {
                        onMove(item.id, max(MeetRoutineCalendar.h0 * 60, min(MeetRoutineCalendar.h1 * 60 - 15, at + delta)))
                    }
                }
        )
        .accessibilityElement()
        .accessibilityLabel("\(item.title), \(RoutineRule.time(at)). Tap to edit, drag to move.")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction(named: "Move 15 minutes earlier") { onMove(item.id, max(MeetRoutineCalendar.h0 * 60, at - 15)) }
        .accessibilityAction(named: "Move 15 minutes later") { onMove(item.id, min(MeetRoutineCalendar.h1 * 60 - 15, at + 15)) }
    }
}

/// One day's routines (a month day tapped): rows open the editor, then "Open the day".
private struct MeetDayList: View {
    let day: Date
    let items: [MeetRoutineCalendar.Item]
    var onEdit: (String) -> Void
    var onOpenDay: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(day.formatted(.dateTime.weekday(.wide).month(.wide).day()))
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(MeetPalette.ink)
                .padding(.horizontal, 6)
                .padding(.top, 16)
            ScrollView {
                VStack(spacing: 2) {
                    ForEach(items) { r in
                        Button { onEdit(r.id) } label: {
                            HStack(spacing: 8) {
                                Capsule().fill(r.color).frame(width: 4, height: 24)
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(r.title).font(.system(size: 12.5, weight: .medium)).foregroundStyle(MeetPalette.ink).lineLimit(1)
                                    Text(r.rule.map { $0.repeatKind == .interval ? "every \(RoutineRuleClock.every($0.every ?? 60))" : RoutineRuleClock.short($0.at) } ?? "All day")
                                        .font(.system(size: 11)).foregroundStyle(MeetPalette.mut).lineLimit(1)
                                }
                                Spacer(minLength: 0)
                                r.avatar.frame(width: 18, height: 18).clipShape(RoundedRectangle(cornerRadius: 4))
                            }
                            .padding(.horizontal, 6)
                            .padding(.vertical, 6)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
            Button(action: onOpenDay) {
                Label("Open the day", systemImage: "calendar")
                    .font(.system(size: 12))
                    .foregroundStyle(MeetPalette.sub)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 4)
            }
            .buttonStyle(.plain)
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(MeetPalette.bg)
    }
}

/// Text in a tinted chip: the colour pulled toward the ink (web tintText).
private func meetTintText(_ color: Color) -> Color {
    SignInPalette.blend(MeetPalette.ink, color, 0.62)
}

enum RoutineRuleClock {
    /// "7a" (web fmtShort for whole hours in the gutter).
    static func hour(_ m: Int) -> String {
        let h = m / 60
        let h12 = h % 12 == 0 ? 12 : h % 12
        return "\(h12)\(h < 12 ? "a" : "p")"
    }

    /// "30 min", "1 h", "2 h" (web everyLabel).
    static func every(_ m: Int) -> String {
        m % 60 == 0 ? "\(m / 60) h" : "\(m) min"
    }

    /// "7:30a" (web fmtShort).
    static func short(_ m: Int) -> String {
        let h = m / 60, mm = m % 60
        let h12 = h % 12 == 0 ? 12 : h % 12
        return "\(h12):\(String(format: "%02d", mm))\(h < 12 ? "a" : "p")"
    }
}

private struct MeetCalButtonStyle: ButtonStyle {
    var square = false
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.system(size: 11.5, weight: .medium))
            .foregroundStyle(MeetPalette.ink)
            .padding(.horizontal, square ? 0 : 10)
            .frame(width: square ? 28 : nil, height: 28)
            .background(configuration.isPressed ? MeetPalette.fill : MeetPalette.bg, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(MeetPalette.line))
            .contentShape(Rectangle().inset(by: -8))
    }
}

// MARK: - First job

/// The hand-off picture (web FirstJobVisual).
struct MeetFirstJobVisual: View {
    let main: AnyView
    let other: AnyView?
    let caption: String
    let expect: String?

    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var go = false

    var body: some View {
        VStack(spacing: 16) {
            HStack(spacing: 16) {
                main
                if let other {
                    ZStack(alignment: .leading) {
                        Line().stroke(MeetPalette.lineDash, style: StrokeStyle(lineWidth: 2, dash: [2, 3])).frame(height: 2)
                        if !reduceMotion {
                            Circle().fill(MeetPalette.coral).frame(width: 10, height: 10)
                                .offset(x: go ? 54 : 0)
                                .opacity(go ? 0 : 1)
                                .animation(.easeInOut(duration: 1.4).repeatForever(autoreverses: false), value: go)
                                .onAppear { go = true }
                        }
                    }
                    .frame(width: 64)
                    other
                }
            }
            Text(caption).font(.system(size: 14, weight: .semibold)).foregroundStyle(MeetPalette.ink)
            if let expect {
                (Text("You’ll get: ").foregroundColor(MeetPalette.ink).fontWeight(.semibold) + Text(expect))
                    .font(.system(size: 12.5))
                    .foregroundStyle(MeetPalette.sub)
                    .frame(maxWidth: 340, alignment: .leading)
                    .padding(12)
                    .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 12))
                    .overlay(RoundedRectangle(cornerRadius: 12).stroke(MeetPalette.line))
            }
            Label("Nothing is sent, posted or paid without your approval.", systemImage: "lock")
                .font(.system(size: 12))
                .foregroundStyle(MeetPalette.mut)
        }
        .multilineTextAlignment(.center)
        .padding(20)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(MeetPalette.soft)
    }

    private struct Line: Shape {
        func path(in rect: CGRect) -> Path {
            Path { p in p.move(to: CGPoint(x: 0, y: rect.midY)); p.addLine(to: CGPoint(x: rect.maxX, y: rect.midY)) }
        }
    }
}
