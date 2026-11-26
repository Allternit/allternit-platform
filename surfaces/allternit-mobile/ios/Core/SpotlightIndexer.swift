import AppIntents
import CoreSpotlight
import Foundation
import UniformTypeIdentifiers

@available(iOS 18.0, *) extension BotEntity: IndexedEntity {}
@available(iOS 18.0, *) extension ConversationEntity: IndexedEntity {}
@available(iOS 18.0, *) extension RoutineEntity: IndexedEntity {}
@available(iOS 18.0, *) extension ConnectionEntity: IndexedEntity {}

/// Spotlight index for bots, conversations, routines and connections (bar
/// apps, `ConnectionEntity`; a tap opens the in-app bar on that app) (plan I4f).
///
/// Every item's unique identifier is an `allternit://` URL, so a tap
/// (`CSSearchableItemActionType`, handled in `AllternitApp`) goes through
/// `DeepLinkRouter` on every iOS version. On iOS 18 each item is also tied to
/// its App Entity (`associateAppEntity`). Approvals are never indexed: they
/// carry private text and can be approved only after the full text is shown.
/// The whole index is removed on sign-out.
@MainActor
enum SpotlightIndexer {
    private static let botDomain = "com.allternit.mobile.bots"
    private static let conversationDomain = "com.allternit.mobile.conversations"
    private static let routineDomain = "com.allternit.mobile.routines"
    private static let connectionDomain = "com.allternit.mobile.connections"
    private static let maxConversations = 50
    private static var refreshing = false

    /// Re-reads bots, conversations and routines through the intent service
    /// and replaces the index. Safe to call often (one run at a time).
    static func refresh() async {
        guard !refreshing, let svc = IntentServiceRegistry.current, svc.isSignedIn() else { return }
        refreshing = true
        defer { refreshing = false }
        async let bots = try? await svc.listBots()
        async let conversations = try? await svc.listConversations(matching: nil)
        async let routines = try? await svc.listRoutines()
        async let connections = try? await (svc as? any BarIntentServicing)?.listBarApps(query: nil)
        let (b, c, r, a) = await (bots, conversations, routines, connections)
        if let b { await replace(domain: botDomain, items: b.map(botItem)) }
        if let c { await replace(domain: conversationDomain, items: c.prefix(maxConversations).map(conversationItem)) }
        if let r { await replace(domain: routineDomain, items: r.map(routineItem)) }
        if let a { await replace(domain: connectionDomain, items: a.map(connectionItem)) }
    }

    /// Sign-out: nothing of this account stays in Spotlight.
    static func removeAll() {
        let domains = [botDomain, conversationDomain, routineDomain, connectionDomain]
        CSSearchableIndex.default().deleteSearchableItems(withDomainIdentifiers: domains) { _ in }
    }

    // MARK: Items

    private static func botItem(_ bot: BotInfo) -> CSSearchableItem {
        let attrs = attributes(title: bot.name, description: bot.status.isEmpty ? "Bot" : "Bot, \(bot.status)")
        if #available(iOS 18.0, *) { attrs.associateAppEntity(BotEntity(bot), priority: 2) }
        return item(url: IntentLinks.botChat(bot.id), domain: botDomain, attrs: attrs)
    }

    private static func conversationItem(_ c: ConversationInfo) -> CSSearchableItem {
        let attrs = attributes(title: c.title, description: "Conversation")
        if #available(iOS 18.0, *) { attrs.associateAppEntity(ConversationEntity(c), priority: 1) }
        // A conversation opens its session when known, else its bot's chat.
        let url = c.sessionId.flatMap { URL(string: "allternit://home?session=\($0.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed) ?? $0)") }
            ?? c.botId.map(IntentLinks.botChat) ?? IntentLinks.home
        return item(url: url, domain: conversationDomain, attrs: attrs)
    }

    private static func routineItem(_ r: RoutineInfo) -> CSSearchableItem {
        let attrs = attributes(title: r.name, description: r.state.isEmpty ? "Routine" : "Routine, \(r.state)")
        if #available(iOS 18.0, *) { attrs.associateAppEntity(RoutineEntity(r), priority: 1) }
        let url = URL(string: "allternit://routine/\(r.id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? r.id)") ?? IntentLinks.home
        return item(url: url, domain: routineDomain, attrs: attrs)
    }

    private static func connectionItem(_ app: BarAppInfo) -> CSSearchableItem {
        let attrs = attributes(title: app.name, description: "Connection, \(ConnectionWording.subtitle(app))")
        if #available(iOS 18.0, *) { attrs.associateAppEntity(ConnectionEntity(app), priority: 1) }
        return item(url: DoWording.barLink(app: app, tool: nil, values: [:]), domain: connectionDomain, attrs: attrs)
    }

    private static func attributes(title: String, description: String) -> CSSearchableItemAttributeSet {
        let attrs = CSSearchableItemAttributeSet(contentType: .content)
        attrs.title = title
        attrs.displayName = title
        attrs.contentDescription = description
        return attrs
    }

    private static func item(url: URL, domain: String, attrs: CSSearchableItemAttributeSet) -> CSSearchableItem {
        attrs.contentURL = url
        return CSSearchableItem(uniqueIdentifier: url.absoluteString, domainIdentifier: domain, attributeSet: attrs)
    }

    /// Deletes the domain, then indexes the new items (so removed bots go).
    private static func replace(domain: String, items: [CSSearchableItem]) async {
        let index = CSSearchableIndex.default()
        try? await index.deleteSearchableItems(withDomainIdentifiers: [domain])
        guard !items.isEmpty else { return }
        try? await index.indexSearchableItems(items)
    }

    /// The route for a tapped Spotlight result, nil when the identifier is not ours.
    static func url(from activity: NSUserActivity) -> URL? {
        guard let id = activity.userInfo?[CSSearchableItemActivityIdentifier] as? String,
              let url = URL(string: id), url.scheme == "allternit" else { return nil }
        return url
    }
}
