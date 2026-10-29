import AVFoundation
import SwiftUI
import UIKit

// -----------------------------------------------------------------------------
// The top half of the "Meet your bots" card on the phone (web MeetYourBotsCard
// `.mb-vis`, stacked): the bots network on the first step, the team grid, the
// main bot's stage, the day calendar of routines and the first-job hand-off.
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

// MARK: - Routines: the day calendar

/// When the team's routines run (web RoutineScheduler, Day view, compact):
/// tap an empty hour to add a routine there, tap one to change it.
struct MeetDayCalendar: View {
    struct Item: Identifiable {
        let id: String
        let title: String
        let rule: RoutineRule?
        let enabled: Bool
        let color: Color
        let avatar: AnyView
    }

    let items: [Item]
    var onNew: (_ day: Date, _ minute: Int) -> Void
    var onEdit: (_ id: String) -> Void

    @State private var day = Calendar.current.startOfDay(for: Date())
    private let rowH: CGFloat = 39
    private let firstHour = 0

    var body: some View {
        VStack(spacing: 0) {
            toolbar
            ScrollViewReader { proxy in
                ScrollView {
                    ZStack(alignment: .topLeading) {
                        VStack(spacing: 0) {
                            ForEach(0..<24, id: \.self) { hour in
                                HStack(alignment: .top, spacing: 6) {
                                    Text(hourLabel(hour))
                                        .font(.system(size: 10))
                                        .foregroundStyle(MeetPalette.mut)
                                        .frame(width: 26, alignment: .trailing)
                                        .offset(y: -6)
                                    Rectangle().fill(MeetPalette.line).frame(height: 1)
                                }
                                .frame(height: rowH, alignment: .top)
                                .contentShape(Rectangle())
                                .onTapGesture { onNew(day, hour * 60) }
                                .id(hour)
                                .accessibilityElement()
                                .accessibilityLabel("Add a routine at \(hourLabel(hour))")
                                .accessibilityAddTraits(.isButton)
                            }
                        }
                        ForEach(runs, id: \.key) { run in
                            Button { onEdit(run.item.id) } label: {
                                HStack(spacing: 6) {
                                    run.item.avatar
                                    Text(run.item.title).font(.system(size: 11.5, weight: .medium)).foregroundStyle(MeetPalette.ink).lineLimit(1)
                                    Spacer(minLength: 4)
                                    Text(RoutineRuleClock.short(run.minute)).font(.system(size: 10.5)).foregroundStyle(MeetPalette.sub)
                                }
                                .padding(.horizontal, 6)
                                .frame(height: 22)
                                .background(run.item.color.opacity(run.item.enabled ? 0.18 : 0.08), in: RoundedRectangle(cornerRadius: 6))
                                .overlay(RoundedRectangle(cornerRadius: 6).stroke(run.item.color.opacity(0.35)))
                                .opacity(run.item.enabled ? 1 : 0.55)
                            }
                            .buttonStyle(.plain)
                            .padding(.leading, 38)
                            .padding(.trailing, 4)
                            .offset(y: CGFloat(run.minute) / 60 * rowH + 2)
                            .accessibilityLabel("\(run.item.title), \(RoutineRuleClock.short(run.minute))\(run.item.enabled ? "" : ", paused")")
                        }
                    }
                    .padding(.top, 8)
                }
                .onAppear { proxy.scrollTo(max(0, firstRunHour - 1), anchor: .top) }
                .onChange(of: day) { _, _ in proxy.scrollTo(max(0, firstRunHour - 1), anchor: .top) }
            }
            .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(MeetPalette.line))
            Text("Tap an empty time to add a routine · tap one to change it")
                .font(.system(size: 11))
                .foregroundStyle(MeetPalette.coral)
                .padding(.top, 6)
        }
        .padding(12)
        .background(MeetPalette.bg)
    }

    private var toolbar: some View {
        HStack(spacing: 6) {
            Button { shift(-1) } label: { Image(systemName: "chevron.left").font(.system(size: 11, weight: .semibold)) }
                .buttonStyle(MeetCalButtonStyle(square: true)).accessibilityLabel("Previous day")
            Button("Today") { day = Calendar.current.startOfDay(for: Date()) }.buttonStyle(MeetCalButtonStyle())
            Button { shift(1) } label: { Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)) }
                .buttonStyle(MeetCalButtonStyle(square: true)).accessibilityLabel("Next day")
            Text(day.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()))
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundStyle(MeetPalette.ink)
                .padding(.leading, 4)
            Spacer(minLength: 0)
            Text("Day")
                .font(.system(size: 11.5, weight: .medium))
                .foregroundStyle(MeetPalette.ink)
                .padding(.horizontal, 10)
                .frame(height: 26)
                .background(MeetPalette.bg, in: RoundedRectangle(cornerRadius: 7))
                .padding(2)
                .background(MeetPalette.fill, in: RoundedRectangle(cornerRadius: 9))
        }
        .padding(.bottom, 8)
    }

    private func shift(_ n: Int) {
        day = Calendar.current.date(byAdding: .day, value: n, to: day) ?? day
    }

    private func hourLabel(_ h: Int) -> String {
        h == 0 ? "12a" : h < 12 ? "\(h)a" : h == 12 ? "12p" : "\(h - 12)p"
    }

    private struct Run { let key: String; let item: Item; let minute: Int }

    private var runs: [Run] {
        let cal = Calendar.current
        let dow = cal.component(.weekday, from: day) - 1
        let dom = cal.component(.day, from: day)
        return items.flatMap { item -> [Run] in
            guard let r = item.rule else { return [] }
            var minutes: [Int] = []
            switch r.repeatKind {
            case .daily: minutes = [r.at]
            case .weekdays, .weekly: minutes = r.days.contains(dow) ? [r.at] : []
            case .monthly: minutes = (r.dom ?? []).contains(dom) ? [r.at] : []
            case .interval:
                if r.days.isEmpty || r.days.contains(dow) {
                    let every = max(15, r.every ?? 60)
                    minutes = Array(stride(from: r.at, through: r.until ?? r.at, by: every))
                }
            }
            return minutes.map { Run(key: "\(item.id)@\($0)", item: item, minute: $0) }
        }
    }

    private var firstRunHour: Int { (runs.map(\.minute).min() ?? 8 * 60) / 60 }
}

enum RoutineRuleClock {
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
