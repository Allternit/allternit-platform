import SwiftUI

/// Minimal right-rail progress panel for the Cowork workspace.
///
/// Parses tasks and working-file paths from the assistant message stream so
/// the user can follow what the agent is doing. Phase 1 uses lightweight
/// regex/text heuristics; later phases can consume structured SSE parts.
struct CoworkProgressPanel: View {
    let messages: [MessageRecord]
    let onOpenTasks: () -> Void

    private var assistantMessages: [MessageRecord] {
        messages.filter { $0.role == "assistant" }
    }

    private var tasks: [ParsedTask] {
        parseTasks(from: assistantMessages)
    }

    private var workingFiles: [String] {
        parseWorkingFiles(from: assistantMessages)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            Divider().background(Color("BorderSubtle"))

            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    tasksSection
                    filesSection
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 16)
            }
        }
        .background(Color("BgPanel").edgesIgnoringSafeArea(.bottom))
    }

    // MARK: - Header

    private var header: some View {
        HStack {
            Text("Progress")
                .font(.system(size: 15, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))

            Spacer()

            Button(action: onOpenTasks) {
                Text("Tasks")
                    .font(.system(size: 13, weight: .medium))
                    .foregroundColor(Color("AccentPrimary"))
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    // MARK: - Tasks

    @ViewBuilder
    private var tasksSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            sectionTitle("Tasks", icon: "checklist")

            if tasks.isEmpty {
                emptyState("No tasks parsed yet.", art: .bell)
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(tasks) { task in
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: task.isDone ? "checkmark.square.fill" : "square")
                                .font(.system(size: 13))
                                .foregroundColor(task.isDone ? Color("AccentPrimary") : Color("TextSecondary"))

                            Text(task.text)
                                .font(.system(size: 13))
                                .foregroundColor(Color("TextPrimary"))
                                .lineLimit(2)

                            Spacer()
                        }
                    }
                }
            }
        }
    }

    // MARK: - Files

    @ViewBuilder
    private var filesSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            sectionTitle("Working Files", icon: "folder")

            if workingFiles.isEmpty {
                emptyState("No files detected yet.", art: .sheets)
            } else {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(workingFiles, id: \.self) { path in
                        HStack(spacing: 6) {
                            Image(systemName: "doc.text")
                                .font(.system(size: 12))
                                .foregroundColor(Color("TextSecondary"))

                            Text(path)
                                .font(.system(size: 12, weight: .medium))
                                .foregroundColor(Color("AccentPrimary"))
                                .lineLimit(1)
                        }
                    }
                }
            }
        }
    }

    // MARK: - Helpers

    private func sectionTitle(_ text: String, icon: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: icon)
                .font(.system(size: 12, weight: .semibold))
                .foregroundColor(Color("TextSecondary"))

            Text(text)
                .font(.system(size: 13, weight: .semibold))
                .foregroundColor(Color("TextPrimary"))
        }
    }

    private func emptyState(_ text: String, art: EmptyArtKind) -> some View {
        FriendlyInlineStateView(title: text, art: art)
    }

    // MARK: - Parsing

    private struct ParsedTask: Identifiable {
        let id: String
        let text: String
        let isDone: Bool
    }

    /// Parses Markdown task markers and simple "TODO:/DONE:" lines.
    private func parseTasks(from messages: [MessageRecord]) -> [ParsedTask] {
        var seen = Set<String>()
        var out: [ParsedTask] = []

        let taskLineRE = try? NSRegularExpression(pattern: "^\\s*[-*]\\s*\\[([ xX])\\]\\s*(.+)$", options: [.anchorsMatchLines])
        let todoRE = try? NSRegularExpression(pattern: "^\\s*(TODO|DONE):\\s*(.+)$", options: [.caseInsensitive, .anchorsMatchLines])

        for message in messages {
            let text = message.content
            let nsRange = NSRange(text.startIndex..., in: text)

            if let taskLineRE {
                for match in taskLineRE.matches(in: text, options: [], range: nsRange) {
                    guard let statusRange = Range(match.range(at: 1), in: text),
                          let bodyRange = Range(match.range(at: 2), in: text) else { continue }
                    let body = String(text[bodyRange]).trimmingCharacters(in: .whitespaces)
                    guard !body.isEmpty else { continue }
                    let isDone = String(text[statusRange]).lowercased() == "x"
                    if seen.insert(body).inserted {
                        out.append(ParsedTask(id: "\(message.id)-\(body.hashValue)", text: body, isDone: isDone))
                    }
                }
            }

            if let todoRE {
                for match in todoRE.matches(in: text, options: [], range: nsRange) {
                    guard let kindRange = Range(match.range(at: 1), in: text),
                          let bodyRange = Range(match.range(at: 2), in: text) else { continue }
                    let body = String(text[bodyRange]).trimmingCharacters(in: .whitespaces)
                    guard !body.isEmpty else { continue }
                    let isDone = String(text[kindRange]).lowercased() == "done"
                    if seen.insert(body).inserted {
                        out.append(ParsedTask(id: "\(message.id)-\(body.hashValue)", text: body, isDone: isDone))
                    }
                }
            }
        }

        return out
    }

    /// Parses file paths that look like files the agent wrote or read.
    private func parseWorkingFiles(from messages: [MessageRecord]) -> [String] {
        let fileRE = try? NSRegularExpression(
            pattern: "(?:^|[^\\w/])(\\./|/)?([\\w/\\.-]+\\.(?:swift|ts|tsx|js|jsx|py|rs|go|json|toml|yaml|yml|md|txt|sh|css|html|sql|env|docx|pdf|xlsx|png|jpg|jpeg))",
            options: []
        )

        var seen = OrderedSet<String>()
        for message in messages {
            let text = message.content
            let nsRange = NSRange(text.startIndex..., in: text)
            guard let fileRE else { continue }
            for match in fileRE.matches(in: text, options: [], range: nsRange) {
                let fullRange = match.range(at: 0)
                guard let range = Range(fullRange, in: text) else { continue }
                let raw = String(text[range])
                    .trimmingCharacters(in: .whitespaces.union(.punctuationCharacters))
                let path = raw.hasPrefix("/") || raw.hasPrefix("./") ? raw : "/\(raw)"
                if !path.isEmpty {
                    seen.append(path)
                }
            }
        }
        return seen.elements
    }
}

// MARK: - OrderedSet helper

private struct OrderedSet<Element: Hashable> {
    private var storage: [Element] = []
    private var seen: Set<Element> = []

    mutating func append(_ element: Element) {
        if seen.insert(element).inserted {
            storage.append(element)
        }
    }

    var elements: [Element] { storage }
}
