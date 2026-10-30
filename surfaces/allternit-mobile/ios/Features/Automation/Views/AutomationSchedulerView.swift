import SwiftUI
import UIKit

/// One entry on the calendar (RoutineScheduler.tsx `SchedulerItem`).
struct SchedulerItem: Identifiable, Equatable {
    let id: String
    /// The key for its mark (a bot id, or `type|agent` on Automation Tasks).
    let botId: String
    let title: String
    let rule: AutomationRoutineRule
    let enabled: Bool
    /// A one-day, all-day item (a goal's target date): shows only on this day, above the hours.
    var on: Date? = nil
    /// Can't be dragged to a new time.
    var locked = false

    func runs(on day: Date) -> Bool {
        if let on { return Calendar.current.isDate(on, inSameDayAs: day) }
        return rule.runs(on: day)
    }

    /// "9:00a", "every 5 min", or "" for all-day items.
    var timeLabel: String {
        if on != nil { return "" }
        if rule.repeatKind == .interval { return "every \(AutomationRoutineRule.everyLabel(rule.every ?? 60))" }
        return AutomationRoutineRule.fmtClock(rule.at)
    }
}

/// A port of RoutineScheduler.tsx at phone width (`compact`): Day (default),
/// Week and Month. Hour grid from 6a to 10p opening at 7a, timed chips in lanes,
/// frequent items as one repeating band, all-day items above the hours, month
/// cells with dots and a day list. Tap an empty time to add, tap an item for its
/// details, press and hold a chip then drag to move it (15-minute steps).
struct AutomationScheduler: View {
    enum Mode: String, CaseIterable { case day, week, month }

    let items: [SchedulerItem]
    let avatar: (String, CGFloat) -> AnyView
    let colorOf: (SchedulerItem) -> Color
    let hint: String
    let label: String
    let onNew: (Date, Int) -> Void
    let onMove: (String, Int) -> Void
    /// The details for an item: its id, the day it was picked on, and close.
    let details: (String, Date, @escaping () -> Void) -> AnyView

    @State private var mode: Mode = .day
    @State private var anchor = Calendar.current.startOfDay(for: Date())
    @State private var picked: Pick? = nil
    @State private var dayList: DayPick? = nil

    private struct Pick: Identifiable {
        let itemId: String
        let day: Date
        var id: String { "\(itemId)@\(day.timeIntervalSince1970)" }
    }

    private struct DayPick: Identifiable {
        let day: Date
        var id: TimeInterval { day.timeIntervalSince1970 }
    }

    private static let h0 = 6
    private static let h1 = 22

    private var px: CGFloat { mode == .day ? 44 : 40 }
    private var calendar: Calendar { Calendar.current }
    private var today: Date { calendar.startOfDay(for: Date()) }

    private func addDays(_ day: Date, _ n: Int) -> Date {
        calendar.date(byAdding: .day, value: n, to: day) ?? day
    }

    /// Weeks start on Monday.
    private func startOfWeek(_ day: Date) -> Date {
        let weekday = calendar.component(.weekday, from: day) - 1
        return addDays(calendar.startOfDay(for: day), -((weekday + 6) % 7))
    }

    private var days: [Date] {
        switch mode {
        case .day: return [anchor]
        case .week:
            let start = startOfWeek(anchor)
            return (0..<7).map { addDays(start, $0) }
        case .month:
            let c = calendar.dateComponents([.year, .month], from: anchor)
            let first = calendar.date(from: c) ?? anchor
            let last = calendar.date(byAdding: DateComponents(month: 1, day: -1), to: first) ?? first
            let from = startOfWeek(first)
            let to = addDays(startOfWeek(last), 7)
            let count = calendar.dateComponents([.day], from: from, to: to).day ?? 35
            return (0..<count).map { addDays(from, $0) }
        }
    }

    private var title: String {
        switch mode {
        case .day:
            return anchor.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day())
        case .month:
            return anchor.formatted(.dateTime.month(.wide).year())
        case .week:
            let list = days
            let f: (Date) -> String = { $0.formatted(.dateTime.month(.abbreviated).day()) }
            return "\(f(list[0])) – \(f(list[6]))"
        }
    }

    private var timed: [SchedulerItem] { items.filter { $0.enabled && $0.on == nil } }
    private var allDay: [SchedulerItem] { items.filter { $0.enabled && $0.on != nil } }

    private func step(_ direction: Int) {
        switch mode {
        case .day: anchor = addDays(anchor, direction)
        case .week: anchor = addDays(anchor, 7 * direction)
        case .month:
            let c = calendar.dateComponents([.year, .month], from: anchor)
            let first = calendar.date(from: c) ?? anchor
            anchor = calendar.date(byAdding: .month, value: direction, to: first) ?? anchor
        }
    }

    private func openDay(_ day: Date) {
        anchor = day
        mode = .day
    }

    // MARK: - Body

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            toolbar
            if mode == .month {
                monthGrid
            } else {
                hourGrid
            }
            Text(hint)
                .font(.system(size: 11))
                .foregroundColor(Color("TextTertiary"))
                .fixedSize(horizontal: false, vertical: true)
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(label)
        .sheet(item: $picked) { pick in
            details(pick.itemId, pick.day, { picked = nil })
        }
        .sheet(item: $dayList) { pick in
            SchedulerDayList(
                day: pick.day,
                items: monthDayItems(pick.day),
                colorOf: colorOf,
                avatar: avatar,
                details: details,
                onOpenDay: {
                    dayList = nil
                    openDay(pick.day)
                },
                close: { dayList = nil }
            )
        }
    }

    // MARK: - Toolbar

    private var toolbar: some View {
        HStack(spacing: 8) {
            HStack(spacing: 4) {
                toolbarButton(icon: "chevron.left", label: "Previous") { step(-1) }
                Button { anchor = today } label: {
                    Text("Today")
                        .font(.system(size: 12))
                        .foregroundColor(Color("TextPrimary"))
                        .padding(.horizontal, 10)
                        .frame(height: 28)
                        .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color("BgPanel")))
                        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                        .contentShape(Rectangle().inset(by: -8))
                }
                .buttonStyle(.plain)
                toolbarButton(icon: "chevron.right", label: "Next") { step(1) }
            }
            Text(title)
                .font(.system(size: 13, weight: .medium))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 0) {
                ForEach(Mode.allCases, id: \.self) { value in
                    let on = mode == value
                    Button { mode = value } label: {
                        Text(value.rawValue.capitalized)
                            .font(.system(size: 12, weight: .medium))
                            .foregroundColor(on ? Color("TextPrimary") : Color("TextTertiary"))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 4)
                            .background(
                                RoundedRectangle(cornerRadius: 6, style: .continuous)
                                    .fill(on ? Color("BgPanel") : Color.clear)
                                    .shadow(color: .black.opacity(on ? 0.06 : 0), radius: 2, y: 1)
                            )
                            .contentShape(Rectangle().inset(by: -10))
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            .padding(2)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("FillNeutral")))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Calendar view")
        }
    }

    private func toolbarButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 11, weight: .medium))
                .foregroundColor(Color("TextSecondary"))
                .frame(width: 28, height: 28)
                .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color("BgPanel")))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
                .contentShape(Rectangle().inset(by: -8))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }

    // MARK: - Day / Week

    private var hourGrid: some View {
        let shownDays = days
        let shownAllDay = allDay
        let gridHeight = CGFloat(Self.h1 - Self.h0) * px
        return VStack(spacing: 0) {
            if mode == .week { weekHeader(shownDays) }
            if shownDays.contains(where: { day in shownAllDay.contains { $0.runs(on: day) } }) {
                allDayRow(shownDays, shownAllDay)
            }
            GeometryReader { geo in
                let columnWidth = max(1, (geo.size.width - 40) / CGFloat(shownDays.count))
                ScrollViewReader { proxy in
                    ScrollView(.vertical, showsIndicators: false) {
                        ZStack(alignment: .topLeading) {
                            // Scroll anchors: the grid opens at 7 AM.
                            VStack(spacing: 0) {
                                ForEach(Self.h0..<Self.h1, id: \.self) { hour in
                                    Color.clear.frame(height: px).id("hour-\(hour)")
                                }
                            }
                            HStack(spacing: 0) {
                                hourLabels(height: gridHeight)
                                ForEach(shownDays, id: \.self) { day in
                                    column(day, width: columnWidth, height: gridHeight, narrow: mode == .week)
                                }
                            }
                        }
                        .frame(height: gridHeight)
                    }
                    .onAppear {
                        DispatchQueue.main.async { proxy.scrollTo("hour-7", anchor: .top) }
                    }
                    .onChange(of: mode) { _, _ in
                        DispatchQueue.main.async { proxy.scrollTo("hour-7", anchor: .top) }
                    }
                }
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
        .frame(maxHeight: .infinity)
    }

    private func weekHeader(_ list: [Date]) -> some View {
        HStack(spacing: 0) {
            Color.clear.frame(width: 40, height: 1)
            ForEach(list, id: \.self) { day in
                let isToday = calendar.isDate(day, inSameDayAs: today)
                let weekday = AutomationRoutineRule.dayShort[calendar.component(.weekday, from: day) - 1]
                Button { openDay(day) } label: {
                    Text(verbatim: "\(weekday.prefix(1)) \(calendar.component(.day, from: day))")
                        .font(.system(size: 11, weight: isToday ? .semibold : .regular))
                        .foregroundColor(isToday ? Color("AccentPrimary") : Color("TextSecondary"))
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                        .contentShape(Rectangle().inset(by: -10))
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Open \(day.formatted(.dateTime.weekday(.wide).month(.wide).day()))")
            }
        }
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    private func allDayRow(_ list: [Date], _ all: [SchedulerItem]) -> some View {
        HStack(spacing: 0) {
            Text("All day")
                .font(.system(size: 10))
                .foregroundColor(Color("TextTertiary"))
                .frame(width: 34, alignment: .trailing)
                .padding(.trailing, 6)
            ForEach(list, id: \.self) { day in
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(all.filter { $0.runs(on: day) }) { item in
                        Button { picked = Pick(itemId: item.id, day: day) } label: {
                            HStack(spacing: 4) {
                                if mode == .day { avatar(item.botId, 14) }
                                Text(item.title).font(.system(size: 11, weight: .medium)).lineLimit(1)
                            }
                            .foregroundColor(SchedulerTint.text(colorOf(item)))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .schedulerTint(colorOf(item), percent: 16, radius: 6)
                            .contentShape(Rectangle().inset(by: -10))
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("\(item.title), all day. Show details.")
                    }
                }
                .padding(2)
                .frame(maxWidth: .infinity, alignment: .topLeading)
                .overlay(alignment: .leading) { Rectangle().fill(Color("BorderSubtle")).frame(width: 1) }
            }
        }
        .overlay(alignment: .bottom) { Rectangle().fill(Color("BorderSubtle")).frame(height: 1) }
    }

    private func hourLabels(height: CGFloat) -> some View {
        ZStack(alignment: .topLeading) {
            Color.clear
            ForEach(1..<(Self.h1 - Self.h0), id: \.self) { h in
                Text(AutomationRoutineRule.fmtShort((Self.h0 + h) * 60))
                    .font(.system(size: 10).monospacedDigit())
                    .foregroundColor(Color("TextTertiary"))
                    .frame(width: 34, alignment: .trailing)
                    .position(x: 17, y: CGFloat(h) * px)
            }
        }
        .frame(width: 40, height: height)
        .accessibilityHidden(true)
    }

    private func column(_ day: Date, width: CGFloat, height: CGFloat, narrow: Bool) -> some View {
        let list = timed.filter { $0.rule.runs(on: day) }
        let bands = list.filter { $0.rule.repeatKind == .interval }
        let singles = list.filter { $0.rule.repeatKind != .interval }.sorted { $0.rule.at < $1.rule.at }
        // Side-by-side lanes for items within 30 minutes of each other.
        var laneOf: [String: Int] = [:]
        var lanes: [[SchedulerItem]] = []
        for item in singles {
            var k = 0
            while k < lanes.count, lanes[k].contains(where: { abs($0.rule.at - item.rule.at) < 30 }) { k += 1 }
            if k == lanes.count { lanes.append([]) }
            lanes[k].append(item)
            laneOf[item.id] = k
        }
        let laneCount = max(1, lanes.count) + (!bands.isEmpty && !narrow ? 1 : 0)
        let isToday = calendar.isDate(day, inSameDayAs: today)
        let nowParts = calendar.dateComponents([.hour, .minute], from: Date())
        let nowMinute = (nowParts.hour ?? 0) * 60 + (nowParts.minute ?? 0)

        let lanesById = laneOf
        let rowCount = Self.h1 - Self.h0
        let rowHeight = px
        let border = Color("BorderSubtle")

        return ZStack(alignment: .topLeading) {
            // Empty time: the tap target under everything.
            Color.clear
                .contentShape(Rectangle())
                .onTapGesture(coordinateSpace: .local) { point in
                    let raw = (Double(point.y / px) * 60 / 30).rounded() * 30
                    let minute = Int(raw) + Self.h0 * 60
                    onNew(day, min(Self.h1 * 60 - 30, max(Self.h0 * 60, minute)))
                }
                .accessibilityElement()
                .accessibilityLabel(day.formatted(.dateTime.weekday(.wide).month(.wide).day()))
                .accessibilityHint("Adds a task at the time you tap")
                .accessibilityAddTraits(.isButton)

            Canvas { context, size in
                for h in 0..<rowCount {
                    let y = CGFloat(h) * rowHeight
                    context.fill(Path(CGRect(x: 0, y: y, width: size.width, height: 1)), with: .color(border.opacity(0.6)))
                }
                context.fill(Path(CGRect(x: 0, y: 0, width: 1, height: size.height)), with: .color(border))
            }
            .allowsHitTesting(false)
            .accessibilityHidden(true)

            ForEach(bands) { item in
                band(item, day: day, width: width, narrow: narrow, laneCount: laneCount)
            }
            ForEach(singles) { item in
                if item.rule.at >= Self.h0 * 60, item.rule.at < Self.h1 * 60 {
                    SchedulerChip(
                        item: item, px: px, narrow: narrow, lane: lanesById[item.id] ?? 0, laneCount: laneCount,
                        columnWidth: width, hour0: Self.h0, hour1: Self.h1, color: colorOf(item),
                        avatar: avatar(item.botId, 16),
                        onOpen: { picked = Pick(itemId: item.id, day: day) },
                        onMove: { onMove(item.id, $0) }
                    )
                }
            }
            if isToday, nowMinute > Self.h0 * 60, nowMinute < Self.h1 * 60 {
                nowLine(top: CGFloat(nowMinute - Self.h0 * 60) / 60 * px, width: width)
            }
        }
        .frame(width: width, height: height)
    }

    private func nowLine(top: CGFloat, width: CGFloat) -> some View {
        Rectangle()
            .fill(Color("AccentPrimary"))
            .frame(width: width, height: 2)
            .overlay(alignment: .leading) {
                Circle().fill(Color("AccentPrimary")).frame(width: 8, height: 8).offset(x: -4)
            }
            .offset(y: top)
            .zIndex(20)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }

    private func band(_ item: SchedulerItem, day: Date, width: CGFloat, narrow: Bool, laneCount: Int) -> some View {
        let color = colorOf(item)
        let every = item.rule.every ?? 60
        let top = CGFloat(max(item.rule.at, Self.h0 * 60) - Self.h0 * 60) / 60 * px
        let bottom = CGFloat(min((item.rule.until ?? item.rule.at) + 30, Self.h1 * 60) - Self.h0 * 60) / 60 * px
        let height = max(px / 2, bottom - top)
        let bandWidth = narrow ? width * 0.34 - 2 : width / CGFloat(laneCount) - 4
        let gap = max(4, CGFloat(every) / 60 * px)
        let words = "every \(AutomationRoutineRule.everyLabel(every))"
        return Button { picked = Pick(itemId: item.id, day: day) } label: {
            ZStack(alignment: .topLeading) {
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color("BgPanel"))
                RoundedRectangle(cornerRadius: 8, style: .continuous).fill(color.opacity(0.10))
                Canvas { context, size in
                    var y = gap - 3
                    while y < size.height {
                        context.fill(Path(CGRect(x: 0, y: y, width: size.width, height: 3)), with: .color(color.opacity(0.40)))
                        y += gap
                    }
                }
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .strokeBorder(color.opacity(0.55), style: StrokeStyle(lineWidth: 1, dash: [3, 3]))
                if !narrow {
                    HStack(alignment: .top, spacing: 4) {
                        avatar(item.botId, 14)
                        VStack(alignment: .leading, spacing: 0) {
                            Text(item.title).lineLimit(2)
                            Text(words).opacity(0.75)
                        }
                    }
                    .font(.system(size: 11, weight: .medium))
                    .foregroundColor(SchedulerTint.text(color))
                    .padding(.horizontal, 6)
                    .padding(.top, 4)
                }
            }
            .frame(width: bandWidth, height: height)
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .offset(x: width - bandWidth - 2, y: top)
        .zIndex(10)
        .accessibilityLabel("\(item.title), \(words). Show details.")
    }

    // MARK: - Month

    /// A day's items in month order: all-day first, then by time.
    private func monthDayItems(_ day: Date) -> [SchedulerItem] {
        items.filter { $0.enabled && $0.runs(on: day) }.sorted { a, b in
            if (a.on != nil) != (b.on != nil) { return a.on != nil }
            if a.rule.at != b.rule.at { return a.rule.at < b.rule.at }
            return a.title.localizedCompare(b.title) == .orderedAscending
        }
    }

    private var monthGrid: some View {
        let columns = Array(repeating: GridItem(.flexible(minimum: 0), spacing: 4), count: 7)
        return ScrollView(.vertical, showsIndicators: false) {
            LazyVGrid(columns: columns, spacing: 4) {
                ForEach(Array(["M", "T", "W", "T", "F", "S", "S"].enumerated()), id: \.offset) { _, letter in
                    Text(letter)
                        .font(.system(size: 11))
                        .foregroundColor(Color("TextTertiary"))
                        .padding(.horizontal, 4)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                ForEach(days, id: \.self) { day in
                    monthCell(day)
                }
            }
        }
        .frame(maxHeight: .infinity, alignment: .top)
    }

    /// Phone: the cell is too small for chips. Dots, and a tap lists the day.
    private func monthCell(_ day: Date) -> some View {
        let list = monthDayItems(day)
        let isToday = calendar.isDate(day, inSameDayAs: today)
        let inMonth = calendar.component(.month, from: day) == calendar.component(.month, from: anchor)
        let dateLabel = day.formatted(.dateTime.weekday(.wide).month(.wide).day())
        return Button {
            if list.isEmpty { openDay(day) } else { dayList = DayPick(day: day) }
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text("\(calendar.component(.day, from: day))")
                    .font(.system(size: 12, weight: isToday ? .semibold : .regular).monospacedDigit())
                    .foregroundColor(isToday ? Color("AccentPrimary") : Color("TextSecondary"))
                    .padding(.horizontal, 4)
                if !list.isEmpty {
                    HStack(spacing: 2) {
                        ForEach(list.prefix(3)) { item in
                            Circle().fill(colorOf(item)).frame(width: 8, height: 8)
                        }
                        if list.count > 3 {
                            Text("+\(list.count - 3)")
                                .font(.system(size: 10).monospacedDigit())
                                .foregroundColor(Color("TextTertiary"))
                        }
                    }
                    .padding(.horizontal, 4)
                    .lineLimit(1)
                    .minimumScaleFactor(0.7)
                }
            }
            .padding(.vertical, 4)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .topLeading)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(isToday ? Color("AccentPrimary") : Color("BorderSubtle"), lineWidth: 1)
            )
            .opacity(inMonth ? 1 : 0.5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(list.isEmpty
            ? "\(dateLabel), nothing scheduled. Open the day"
            : "\(dateLabel), \(list.count) item\(list.count == 1 ? "" : "s"). Show the list")
    }
}

// MARK: - Chip

/// A timed block in Day / Week: name and time, draggable unless locked. A bare
/// drag scrolls the grid on a phone, so moving starts with a short press.
private struct SchedulerChip: View {
    let item: SchedulerItem
    let px: CGFloat
    let narrow: Bool
    let lane: Int
    let laneCount: Int
    let columnWidth: CGFloat
    let hour0: Int
    let hour1: Int
    let color: Color
    let avatar: AnyView
    let onOpen: () -> Void
    let onMove: (Int) -> Void

    private struct DragState: Equatable {
        var active = false
        var y: CGFloat = 0
    }

    @GestureState private var drag = DragState()

    private func snapped(_ y: CGFloat) -> Int {
        Int((Double(y / px) * 60 / 15).rounded()) * 15
    }

    private func move(by delta: Int) {
        guard delta != 0 else { return }
        onMove(max(hour0 * 60, min(hour1 * 60 - 15, item.rule.at + delta)))
    }

    private var dragGesture: some Gesture {
        LongPressGesture(minimumDuration: 0.25)
            .sequenced(before: DragGesture(minimumDistance: 0, coordinateSpace: .global))
            .updating($drag) { value, state, _ in
                if case .second(true, let moved) = value {
                    state.active = true
                    state.y = moved?.translation.height ?? 0
                }
            }
            .onEnded { value in
                guard case .second(true, let moved?) = value else { return }
                move(by: snapped(moved.translation.height))
            }
    }

    var body: some View {
        let height = max(22, px / 2 - 2)
        let width = narrow ? columnWidth - 4 : columnWidth / CGFloat(laneCount) - 6
        let x = narrow ? 2 : columnWidth * CGFloat(lane) / CGFloat(laneCount) + 3
        let top = CGFloat(item.rule.at - hour0 * 60) / 60 * px
        let time = AutomationRoutineRule.fmtClock(item.rule.at)
        let inset = max(0, (44 - height) / 2)

        HStack(spacing: 4) {
            if !narrow {
                avatar.clipShape(RoundedRectangle(cornerRadius: 4, style: .continuous))
            }
            Text(item.title)
                .font(.system(size: 11, weight: .semibold))
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(time)
                .font(.system(size: 10.5).monospacedDigit())
                .opacity(0.75)
                .fixedSize()
        }
        .foregroundColor(SchedulerTint.text(color))
        .padding(.leading, 7)
        .padding(.trailing, 6)
        .frame(width: width, height: height)
        .schedulerTint(color, percent: 18, radius: 6)
        .overlay(alignment: .leading) { Rectangle().fill(color).frame(width: 3) }
        .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        .contentShape(Rectangle().inset(by: -inset))
        .onTapGesture(perform: onOpen)
        .gesture(item.locked ? nil : dragGesture)
        .scaleEffect(drag.active ? 1.03 : 1)
        .shadow(color: .black.opacity(drag.active ? 0.35 : 0), radius: 12, y: 10)
        .offset(x: x, y: top + drag.y)
        .zIndex(drag.active ? 30 : 10)
        .animation(.easeOut(duration: 0.12), value: drag.active)
        .onChange(of: drag.active) { _, active in
            if active { UIImpactFeedbackGenerator(style: .light).impactOccurred() }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(item.title), \(AutomationRoutineRule.fmtTime(item.rule.at)). Show details\(item.locked ? "" : ", swipe up or down to move").")
        .accessibilityAddTraits(.isButton)
        .accessibilityAction { onOpen() }
        .accessibilityAdjustableAction { direction in
            guard !item.locked else { return }
            switch direction {
            case .increment: move(by: 15)
            case .decrement: move(by: -15)
            @unknown default: break
            }
        }
    }
}

// MARK: - Day list

/// One day's items as a list (DayList in RoutineScheduler.tsx); a row opens the
/// details in place, with a way back.
private struct SchedulerDayList: View {
    let day: Date
    let items: [SchedulerItem]
    let colorOf: (SchedulerItem) -> Color
    let avatar: (String, CGFloat) -> AnyView
    let details: (String, Date, @escaping () -> Void) -> AnyView
    let onOpenDay: () -> Void
    let close: () -> Void

    @State private var detail: String? = nil

    var body: some View {
        Group {
            if let detail {
                VStack(alignment: .leading, spacing: 6) {
                    Button { self.detail = nil } label: {
                        Label(day.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()), systemImage: "arrow.left")
                            .font(.system(size: 12))
                            .foregroundColor(Color("TextSecondary"))
                            .frame(minHeight: 44)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .padding(.horizontal, 16)
                    details(detail, day, close)
                }
                .padding(.top, 8)
            } else {
                list
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
        .presentationBackground(Color("BgPrimary"))
    }

    private var list: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 2) {
                Text(day.formatted(.dateTime.weekday(.wide).month(.wide).day()))
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.bottom, 6)
                ForEach(items) { item in
                    Button { detail = item.id } label: {
                        HStack(spacing: 8) {
                            RoundedRectangle(cornerRadius: 2).fill(colorOf(item)).frame(width: 4, height: 24)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(item.title)
                                    .font(.system(size: 14, weight: .medium))
                                    .foregroundColor(Color("TextPrimary"))
                                    .lineLimit(1)
                                Text(item.timeLabel.isEmpty ? "All day" : item.timeLabel)
                                    .font(.system(size: 12))
                                    .foregroundColor(Color("TextTertiary"))
                                    .lineLimit(1)
                            }
                            Spacer(minLength: 0)
                            avatar(item.botId, 18)
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                }
                Button(action: onOpenDay) {
                    Label("Open the day", systemImage: "calendar")
                        .font(.system(size: 13))
                        .foregroundColor(Color("TextSecondary"))
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: - Tint

extension View {
    /// `tintBg`: a soft tint of `color` over the card surface; readable in light and dark.
    func schedulerTint(_ color: Color, percent: Double, radius: CGFloat) -> some View {
        background(
            RoundedRectangle(cornerRadius: radius, style: .continuous)
                .fill(color.opacity(percent / 100))
                .background(RoundedRectangle(cornerRadius: radius, style: .continuous).fill(Color("BgPanel")))
        )
    }
}
