import SwiftUI

/// The Plans chip's content (design after/R3-plans-phone.png; web
/// PlansList.tsx): every plan the person has, grouped by project, newest
/// first. Plans live only here — the server drops them from every other list.
@MainActor
final class PlansModel: ObservableObject {
    enum Phase: Equatable { case loading, loaded, failed }
    @Published private(set) var plans: [ArtifactV2Summary] = []
    @Published private(set) var phase: Phase = .loading

    func load() async {
        if plans.isEmpty { phase = .loading }
        do {
            plans = try await ArtifactLibraryStore.shared.fetchPlans()
            phase = .loaded
        } catch is CancellationError {
            return
        } catch {
            phase = plans.isEmpty ? .failed : .loaded
        }
    }
}

struct PlanGroup: Identifiable, Equatable {
    let project: String
    let plans: [ArtifactV2Summary]
    var id: String { project }
}

enum PlansGrouping {
    static let noProject = "No project"

    /// Newest first inside each project; "No project" last.
    static func groups(_ items: [ArtifactV2Summary]) -> [PlanGroup] {
        let sorted = items.sorted { ($0.updatedDate ?? .distantPast) > ($1.updatedDate ?? .distantPast) }
        var order: [String] = []
        var map: [String: [ArtifactV2Summary]] = [:]
        for plan in sorted {
            let name = text(plan.origin?.projectTitle) ?? noProject
            if map[name] == nil { order.append(name) }
            map[name, default: []].append(plan)
        }
        let rest = order.filter { $0 != noProject } + order.filter { $0 == noProject }
        return rest.map { PlanGroup(project: $0, plans: map[$0] ?? []) }
    }

    static func text(_ value: String?) -> String? {
        let t = value?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return t.isEmpty ? nil : t
    }

    static func chat(_ plan: ArtifactV2Summary) -> String? {
        text(plan.origin?.sessionTitle) ?? text(plan.origin?.chatTitle)
    }

    static func relative(_ date: Date?) -> String {
        guard let date else { return "" }
        let s = Int(Date().timeIntervalSince(date))
        if s < 60 { return "just now" }
        if s < 3600 { return "\(s / 60) min ago" }
        if s < 86400 { return "\(s / 3600) h ago" }
        if s < 86400 * 2 { return "yesterday" }
        if s < 86400 * 7 { return "\(s / 86400) days ago" }
        return date.formatted(.dateTime.month(.abbreviated).day())
    }
}

struct PlansListView: View {
    @StateObject private var model = PlansModel()
    let onOpen: (ArtifactV2Summary) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label {
                Text("Plans stay here. They don't show in Everything or Recent.")
            } icon: {
                Image(systemName: "info.circle")
            }
            .font(.system(size: 13))
            .foregroundColor(Color("TextTertiary"))
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .task { await model.load() }
        .refreshable { await model.load() }
    }

    @ViewBuilder private var content: some View {
        switch model.phase {
        case .loading:
            HStack(spacing: 8) {
                ProgressView()
                Text("Loading plans…").font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
            }
            .padding(.vertical, 24)
            .accessibilityElement(children: .combine)
        case .failed:
            VStack(spacing: 10) {
                Text("Couldn't load your plans.")
                    .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                Button("Retry") { Task { await model.load() } }
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 14).padding(.vertical, 6)
                    .background(Capsule().fill(Color("FillNeutral")))
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 24)
        case .loaded:
            if model.plans.isEmpty {
                Text("No plans yet. Plans the agent writes show up here.")
                    .font(.system(size: 14)).foregroundColor(Color("TextSecondary"))
                    .frame(maxWidth: .infinity).multilineTextAlignment(.center)
                    .padding(.vertical, 24)
            } else {
                ForEach(PlansGrouping.groups(model.plans)) { group in
                    section(group)
                }
            }
        }
    }

    private func section(_ group: PlanGroup) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text(group.project).font(.system(size: 15, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                Text("\(group.plans.count)").font(.system(size: 14)).foregroundColor(Color("TextTertiary"))
            }
            .accessibilityElement(children: .combine)
            VStack(spacing: 0) {
                ForEach(Array(group.plans.enumerated()), id: \.element.id) { index, plan in
                    if index > 0 { Divider().overlay(Color("BorderSubtle")) }
                    row(plan)
                }
            }
            .background(RoundedRectangle(cornerRadius: 16).fill(Color("BgPanel")))
            .overlay(RoundedRectangle(cornerRadius: 16).stroke(Color("BorderSubtle"), lineWidth: 1))
        }
    }

    private func row(_ plan: ArtifactV2Summary) -> some View {
        HStack(alignment: .center, spacing: 10) {
            VStack(alignment: .leading, spacing: 3) {
                Text(plan.title).font(.system(size: 16, weight: .semibold)).foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                if let chat = PlansGrouping.chat(plan) {
                    Text("Chat: \(chat)").font(.system(size: 13)).foregroundColor(Color("TextSecondary")).lineLimit(1)
                }
                HStack(spacing: 8) {
                    PlanStatusPill(status: plan.plan?.status)
                    Text("v\(plan.currentVersion)").font(.system(size: 13)).foregroundColor(Color("TextTertiary"))
                    Text(PlansGrouping.relative(plan.updatedDate)).font(.system(size: 13)).foregroundColor(Color("TextTertiary"))
                }
                .padding(.top, 3)
            }
            Spacer(minLength: 6)
            Button {
                Haptics.light()
                onOpen(plan)
            } label: {
                Text("Open").font(.system(size: 14, weight: .medium)).foregroundColor(Color("TextPrimary"))
                    .padding(.horizontal, 16).padding(.vertical, 8)
                    .background(Capsule().fill(Color("FillNeutral")))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Open \(plan.title)")
        }
        .padding(.horizontal, 14).padding(.vertical, 12)
    }
}

struct PlanStatusPill: View {
    let status: String?

    private var spec: (label: String, fg: Color, bg: Color) {
        switch status {
        case "approved": return ("Approved", Color(red: 0.10, green: 0.50, blue: 0.27), Color(red: 0.89, green: 0.95, blue: 0.91))
        case "done": return ("Done", Color("TextSecondary"), Color("FillNeutral"))
        default: return ("Open", Color(red: 0.11, green: 0.31, blue: 0.85), Color(red: 0.91, green: 0.94, blue: 1.0))
        }
    }

    var body: some View {
        Text(spec.label)
            .font(.system(size: 13, weight: .medium))
            .foregroundColor(spec.fg)
            .padding(.horizontal, 10).padding(.vertical, 3)
            .background(Capsule().fill(spec.bg))
    }
}
