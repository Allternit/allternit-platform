import SwiftUI
import UIKit

// -----------------------------------------------------------------------------
// The channel connect wizards (allternit-ai src/components/gateway/connect/
// *Wizard.tsx) as native flows on the ConnectWizardView engine: a phone
// number, email, Telegram, Slack, Discord, Microsoft Teams and WhatsApp
// Business. Each one owns its steps; copy follows the web.
// -----------------------------------------------------------------------------

/// The providers "Give a bot a phone" offers (web CONNECT_ORDER).
enum ChannelProvider: String, CaseIterable, Identifiable {
    case sms, email, telegram, discord, slack, teams, whatsapp
    var id: String { rawValue }

    var label: String {
        switch self {
        case .sms: return "Phone number"
        case .email: return "Email"
        case .telegram: return "Telegram"
        case .discord: return "Discord"
        case .slack: return "Slack"
        case .teams: return "Microsoft Teams"
        case .whatsapp: return "WhatsApp"
        }
    }

    var blurb: String {
        switch self {
        case .sms: return "Call or text your bots on a real phone number. Calls work instantly; texting follows carrier approval."
        case .email: return "Anyone can email your bots. You approve replies, or automate them for known senders."
        case .telegram: return "Chat with your bots in Telegram. Allternit creates the bot — you tap Confirm once."
        case .discord: return "Your bots answer in a Discord server through the shared Allternit app."
        case .slack: return "Your bots answer in Slack through the shared Allternit app."
        case .teams: return "Your bots answer in Microsoft Teams chats through the shared Allternit app."
        case .whatsapp: return "Your bots answer on a WhatsApp Business number, connected through Meta."
        }
    }

    var icon: String { BotPhoneAPI.providerIcon(self == .sms ? "sms" : rawValue) }

    @MainActor
    func flow(botId: String?) -> ConnectFlow {
        switch self {
        case .sms: return PhoneNumberFlow(botId: botId)
        case .email: return EmailConnectFlow(botId: botId)
        case .telegram: return TelegramConnectFlow(botId: botId)
        case .discord, .slack, .teams, .whatsapp: return OAuthChannelFlow(provider: self, botId: botId)
        }
    }
}

@MainActor
private func botOptions() -> [WizardOption] {
    BotModeStore.shared.bots.map { WizardOption(value: $0.id, label: $0.botDisplayName) }
}

// MARK: - Phone number (PhoneWizard.tsx)

@MainActor
final class PhoneNumberFlow: ConnectFlow {
    private var pickedBot: String?
    private var tollFree = false
    private var options: [PhoneNumberOption] = []
    private var number: BoughtNumber?
    private var registration: PhoneRegistrationState?
    private var individual = false
    /// "Already have a number? Port it in." (PhoneWizard.tsx porting path).
    private var porting = false
    private var portAccountId: String?
    private var portedE164 = ""

    init(botId: String?) {
        pickedBot = botId
        super.init(title: "Connect a phone number", icon: "phone", botId: botId, firstStep: "intro")
    }

    override func prepare() async {
        if (await ChannelConnectAPI.availability())["phone"] != true {
            comingSoon = "Give a bot a real phone number: anyone can call it or text it, and it answers or follows up. Phone numbers aren’t available on your account yet — they come with an upcoming update."
        }
    }

    private var doneBody: String {
        if porting {
            return "\(portedE164.isEmpty ? "Your number" : portedE164) answers as your bot once the port completes. Carriers take a few days; we email you when it's done, and the number keeps working on your old carrier until then."
        }
        let e164 = number?.e164 ?? "Your number"
        var text = "\(e164) answers calls now, and texts arrive as threads."
        switch registration?.state {
        case "approved": text += " Texting is approved — the bot can text back."
        case "in_review": text += " Texting is in carrier review; it turns on by itself when approved."
        case "waiting": text += " Texting registration is filed and waiting on Allternit’s side; it continues by itself."
        case "needs_code": text += " Texting needs the code the carrier texted to your mobile — reopen this to enter it."
        case "rejected": text += " Texting registration was rejected\(registration?.reason.map { ": \($0)" } ?? ""). Fix the details and resubmit from Allternit on your computer."
        default: break
        }
        return text
    }

    private func submitRegistration(_ v: WizardValues, individual who: Bool) async throws -> String? {
        guard let number else { throw ConnectError(status: nil, code: nil, message: "Pick a number first.") }
        individual = who
        do {
            registration = try await ChannelConnectAPI.submitRegistration(accountId: number.accountId, individual: who, values: v.text)
        } catch {
            throw ConnectError(status: nil, code: nil, message: "Couldn’t submit the registration: \(error.localizedDescription)")
        }
        changed()
        return registration?.state == "needs_code" ? "code" : "registration"
    }

    private var portSteps: [ConnectStep] {
        [
            ConnectStep(id: "port", title: "Which number should we port?", kind: .form(
                body: "Porting moves your existing number to Allternit’s carrier. Your number keeps working until the port completes.",
                fields: [
                    WizardField(key: "e164", label: "Phone number", placeholder: "+1 415 555 0132",
                                hint: "Include the country code.", contentType: .telephoneNumber, keyboard: .phonePad),
                ],
                initial: WizardValues(text: ["e164": portedE164]),
                submitLabel: "Start the port",
                onSubmit: { [unowned self] v in
                    guard let bot = self.pickedBot else { throw ConnectError(status: nil, code: nil, message: "Pick which bot answers this number first.") }
                    let e164 = v.string("e164")
                    do {
                        self.portAccountId = try await ChannelConnectAPI.portNumber(e164: e164, botId: bot)
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t start the port: \(error.localizedDescription)")
                    }
                    self.portedE164 = e164
                    self.accountId = self.portAccountId
                    self.changed()
                    return nil
                })),
            ConnectStep(id: "port-waiting", title: "Port in progress", kind: .waiting(
                body: "Starting the port…", status: "Waiting for the carrier…", pollSeconds: 10, skippable: true, cancelLabel: nil,
                poll: { [unowned self] in
                    guard let id = self.portAccountId else { return WaitingPoll(state: .pending, detail: "Starting the port…") }
                    switch try await ChannelConnectAPI.portStatus(accountId: id) {
                    case .done: return WaitingPoll(state: .done, detail: "The port completed.")
                    case .rejected(let reason):
                        return WaitingPoll(state: .failed, error: reason.map { "The port was rejected: \($0). Fix the details and try again." }
                            ?? "The port was rejected. Check the account details and try again.")
                    case .pending:
                        return WaitingPoll(state: .pending, detail: "Carriers take a few days (usually a few business days). You can keep waiting or continue; we email you when it completes.")
                    }
                })),
        ]
    }

    private static let verticals: [WizardOption] = [
        ("PROFESSIONAL", "Professional services"), ("HEALTHCARE", "Healthcare"), ("RETAIL", "Retail"),
        ("REAL_ESTATE", "Real estate"), ("EDUCATION", "Education"), ("HOSPITALITY", "Restaurants and hospitality"),
        ("TECHNOLOGY", "Technology"), ("CONSTRUCTION", "Construction and home services"), ("NON_PROFIT", "Non-profit"),
        ("TRANSPORTATION", "Transportation"),
    ].map { WizardOption(value: $0.0, label: $0.1) }

    override var steps: [ConnectStep] {
        let bots = botOptions()
        let useCase = pickedBot.flatMap { BotModeStore.shared.bot($0)?.botTagline } ?? ""
        let regInitial = WizardValues(text: ["useCase": useCase, "vertical": "PROFESSIONAL"])
        let optIn = WizardField(key: "optInPageUrl", label: "Opt-in page", placeholder: "https://acme.com/text-us",
                                hint: "A public page that shows this number and says texting it means agreeing to replies. Leave blank and Allternit hosts one for you.",
                                optional: true, contentType: .URL, keyboard: .URL)
        return [
            ConnectStep(id: "intro", title: "A real number for your bot", kind: .intro(
                body: "Pick a number and your bot can take calls and texts.",
                bullets: ["Search by area code and pick a local or toll-free number.",
                          "Calls work the moment the number is yours.",
                          "SMS turns on when the carrier approves your registration.",
                          "Already have a number? Choose to port it in on the next step."])),
            ConnectStep(id: "bot", title: "Which bot answers this number?", kind: .form(
                body: bots.isEmpty ? "Create a bot first — the number needs a bot to answer it." : nil,
                fields: (botId == nil && !bots.isEmpty
                    ? [WizardField(key: "botId", label: "Bot", kind: .radio(bots), hint: "Calls and texts for the number land in this bot’s threads.")]
                    : []) + (bots.isEmpty && botId == nil ? [] : [
                        WizardField(key: "source", label: "Number", kind: .radio([
                            WizardOption(value: "new", label: "Get a new number", hint: "Search by area code and buy one now."),
                            WizardOption(value: "port", label: "Port my existing number", hint: "Move a number you already have to Allternit’s carrier. It keeps working until the port completes."),
                        ])),
                    ]),
                initial: WizardValues(text: ["botId": pickedBot ?? bots.first?.value ?? "", "source": porting ? "port" : "new"]),
                submitLabel: "Continue",
                onSubmit: { [unowned self] v in
                    let chosen = self.botId ?? (v.string("botId").isEmpty ? nil : v.string("botId")) ?? bots.first?.value
                    guard let chosen else { throw ConnectError(status: nil, code: nil, message: "Create a bot first — the number needs a bot to answer it.") }
                    self.pickedBot = chosen
                    self.porting = v.string("source") == "port"
                    self.changed()
                    return self.porting ? "port" : nil
                })),
            ConnectStep(id: "search", title: "Find a number", kind: .form(
                body: nil,
                fields: [
                    WizardField(key: "kind", label: "Number type", kind: .radio([
                        WizardOption(value: "local", label: "Local", hint: "A number with an area code, e.g. +1 (212) 555 0132."),
                        WizardOption(value: "tollfree", label: "Toll-free", hint: "An 8xx number with no area code. Texting verification usually takes a few days."),
                    ])),
                    WizardField(key: "areaCode", label: "Area code", placeholder: "212", hint: "US/Canada area code, or leave blank for any.", optional: true, keyboard: .numberPad),
                ],
                initial: WizardValues(text: ["kind": "local"]),
                submitLabel: "Search",
                onSubmit: { [unowned self] v in
                    let toll = v.string("kind") == "tollfree"
                    let found: [PhoneNumberOption]
                    do {
                        found = try await ChannelConnectAPI.searchNumbers(tollFree: toll, areaCode: v.string("areaCode"))
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t search numbers: \(error.localizedDescription)")
                    }
                    if found.isEmpty { throw ConnectError(status: nil, code: nil, message: "No numbers found for that search. Try a nearby area code, or toll-free.") }
                    self.tollFree = toll
                    self.options = found
                    self.changed()
                    return "pick"
                })),
            ConnectStep(id: "pick", title: "Pick your number", kind: .form(
                body: nil,
                fields: [WizardField(key: "e164", label: "Available numbers",
                                     kind: .radio(options.map { WizardOption(value: $0.e164, label: $0.e164, hint: $0.detail.isEmpty ? nil : $0.detail) }))],
                initial: WizardValues(),
                submitLabel: "Buy this number",
                onSubmit: { [unowned self] v in
                    guard let bot = self.pickedBot ?? self.botId ?? bots.first?.value else {
                        throw ConnectError(status: nil, code: nil, message: "Pick which bot answers this number first.")
                    }
                    do {
                        self.number = try await ChannelConnectAPI.buyNumber(e164: v.string("e164"), tollFree: self.tollFree, botId: bot)
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t buy the number: \(error.localizedDescription)")
                    }
                    self.accountId = self.number?.accountId
                    self.changed()
                    return nil
                })),
            ConnectStep(id: "calls", title: "Calls work now", kind: .intro(
                body: "\(number?.e164 ?? "Your number") is wired: calls are answered in the cloud and land in your bot’s threads, and a missed call gets a follow-up. One more step turns on texting.",
                bullets: [])),
            ConnectStep(id: "sender", title: "Turn on texting", kind: .form(
                body: "US carriers ask who is texting before a number can send texts. Calls already work while this is reviewed.",
                fields: [WizardField(key: "sender", label: "Who is texting?", kind: .radio([
                    WizardOption(value: "business", label: "A business", hint: "Has a US tax ID (EIN)."),
                    WizardOption(value: "individual", label: "Just me", hint: "A person or sole proprietor without an EIN. The carrier texts you a code to confirm it’s you."),
                ]))],
                initial: WizardValues(text: ["sender": "business"]),
                submitLabel: "Next",
                onSubmit: { v in v.string("sender") == "individual" ? "individual" : "business" })),
            ConnectStep(id: "business", title: "Your business", kind: .form(
                body: "Use the details on file for the business. We pre-filled the use case from your bot.",
                fields: [
                    WizardField(key: "legalName", label: "Legal business name", placeholder: "Acme Corp", contentType: .organizationName),
                    WizardField(key: "ein", label: "EIN (tax ID)", placeholder: "12-3456789", keyboard: .numbersAndPunctuation),
                    WizardField(key: "vertical", label: "Industry", kind: .select(Self.verticals)),
                    WizardField(key: "street", label: "Street address", contentType: .fullStreetAddress),
                    WizardField(key: "city", label: "City", contentType: .addressCity),
                    WizardField(key: "state", label: "State", placeholder: "MN", contentType: .addressState),
                    WizardField(key: "postalCode", label: "ZIP code", contentType: .postalCode, keyboard: .numberPad),
                    WizardField(key: "firstName", label: "Contact first name", contentType: .givenName),
                    WizardField(key: "lastName", label: "Contact last name", contentType: .familyName),
                    WizardField(key: "email", label: "Contact email", contentType: .emailAddress, keyboard: .emailAddress),
                    WizardField(key: "phone", label: "Contact phone", placeholder: "+1 612 555 0100", contentType: .telephoneNumber, keyboard: .phonePad),
                    WizardField(key: "website", label: "Website", placeholder: "https://acme.com", optional: true, contentType: .URL, keyboard: .URL),
                    WizardField(key: "useCase", label: "Use case", kind: .multiline, hint: "What the number texts about, in plain language."),
                    WizardField(key: "sampleMessages", label: "Sample messages", kind: .multiline, hint: "Two or three example texts, one per line. Include your business name.", optional: true),
                    optIn,
                ],
                initial: regInitial,
                submitLabel: "Submit registration",
                onSubmit: { [unowned self] v in try await self.submitRegistration(v, individual: false) })),
            ConnectStep(id: "individual", title: "About you", kind: .form(
                body: "For a person or sole proprietor without an EIN. Carriers allow one number per person this way.",
                fields: [
                    WizardField(key: "firstName", label: "First name", contentType: .givenName),
                    WizardField(key: "lastName", label: "Last name", contentType: .familyName),
                    WizardField(key: "legalName", label: "Name people will see", placeholder: "Dana Lee Tutoring", hint: "Leave blank to use your name.", optional: true),
                    WizardField(key: "mobilePhone", label: "Your mobile number", placeholder: "+1 612 555 0188", hint: "The carrier texts a code here. Not the bot’s number.", contentType: .telephoneNumber, keyboard: .phonePad),
                    WizardField(key: "email", label: "Email", contentType: .emailAddress, keyboard: .emailAddress),
                    WizardField(key: "street", label: "Street address", contentType: .fullStreetAddress),
                    WizardField(key: "city", label: "City", contentType: .addressCity),
                    WizardField(key: "state", label: "State", placeholder: "MN", contentType: .addressState),
                    WizardField(key: "postalCode", label: "ZIP code", contentType: .postalCode, keyboard: .numberPad),
                    WizardField(key: "vertical", label: "What you do", kind: .select(Self.verticals)),
                    WizardField(key: "useCase", label: "Use case", kind: .multiline, hint: "What the number texts about, in plain language."),
                    WizardField(key: "sampleMessages", label: "Sample messages", kind: .multiline, hint: "One per line.", optional: true),
                    optIn,
                ],
                initial: regInitial,
                submitLabel: "Send me the code",
                onSubmit: { [unowned self] v in try await self.submitRegistration(v, individual: true) })),
            ConnectStep(id: "code", title: "Enter the code", kind: .code(
                body: "The carrier texted a code to your mobile number. Enter it to confirm it’s you; then the registration continues by itself.",
                length: 6,
                onSubmit: { [unowned self] code in
                    guard let number = self.number else { throw ConnectError(status: nil, code: nil, message: "Pick a number first.") }
                    self.registration = try await ChannelConnectAPI.verifyCode(accountId: number.accountId, pin: code)
                    self.changed()
                    return "registration"
                },
                onResend: { [unowned self] in
                    if let number = self.number { _ = try? await ChannelConnectAPI.verifyCode(accountId: number.accountId, pin: nil) }
                })),
            ConnectStep(id: "registration", title: "Texting registration", kind: .waiting(
                body: nil, status: "Waiting for carrier approval…", pollSeconds: 5, skippable: true, cancelLabel: nil,
                poll: { [unowned self] in
                    guard let number = self.number else { return WaitingPoll(state: .pending, detail: "Submitting…") }
                    let r = try await ChannelConnectAPI.registrationStatus(accountId: number.accountId, individual: self.individual)
                    self.registration = r
                    self.changed()
                    switch r.state {
                    case "approved": return WaitingPoll(state: .done, detail: "Approved — texting is on.")
                    case "rejected":
                        return WaitingPoll(state: .failed, error: "The carrier rejected the registration\(r.reason.map { ": \($0)" } ?? ""). Go back, fix the details and submit again.")
                    case "waiting": return WaitingPoll(state: .pending, detail: "Filed. It is waiting on Allternit’s side and continues by itself — you can close this.")
                    case "needs_code": return WaitingPoll(state: .pending, detail: "Waiting for the code the carrier texted to your mobile.")
                    default: return WaitingPoll(state: .pending, detail: "In carrier review (usually a few days to two weeks; the carriers set the pace). You can continue — texting turns on by itself when approved.")
                    }
                })),
        ] + (porting ? portSteps : []) + [
            ConnectStep(id: "done", title: porting ? "Port started" : "Your number is live", kind: .done(body: doneBody)),
        ]
    }
}

// MARK: - Email (EmailWizard.tsx)

@MainActor
final class EmailConnectFlow: ConnectFlow {
    private var target: String?
    private var address = ""
    private var replyMode = "approve"
    private var allowlist: [String] = []
    private var senderPolicy = "owner"
    private var allowedSenders: [String] = []
    private var ownerAddress: String?
    private var domainCheck: ChannelConnectAPI.DomainCheck?

    init(botId: String?) {
        target = botId
        super.init(title: "Connect Email", icon: "envelope", botId: botId, firstStep: "intro")
    }

    override func prepare() async {
        // The address belongs to one bot: the one this was opened for, else the first bot.
        guard let bot = target ?? BotModeStore.shared.bots.first?.id else {
            comingSoon = "Create a bot first, then give it its own email address here."
            return
        }
        target = bot
        do {
            if let made = try await ChannelConnectAPI.emailAddress(botId: bot) {
                address = made.address
                replyMode = made.replyMode ?? "approve"
                allowlist = made.allowlist
                if let senders = try? await ChannelConnectAPI.emailSenders(botId: bot) {
                    senderPolicy = senders.policy
                    allowedSenders = senders.allowed
                    ownerAddress = senders.owner
                }
            } else {
                comingSoon = "Give a bot an email address that anyone can write to, with replies you approve or automate. Bot email isn’t switched on for your account yet."
            }
        } catch {
            comingSoon = "Couldn’t reach your computer to set up email (\(error.localizedDescription)). Check that it’s online, then try again."
        }
    }

    override var steps: [ConnectStep] {
        let name = target.flatMap { BotModeStore.shared.bot($0)?.botDisplayName } ?? "Your bot"
        return [
            ConnectStep(id: "intro", title: "Your bots get an email address", kind: .intro(body: nil, bullets: [
                "The address is live the moment this finishes.",
                "Only you can make it act until you add others.",
                "Add a company domain later for branded addresses.",
            ])),
            ConnectStep(id: "address", title: "Your address is live", kind: .form(
                body: "\(name) answers at \(address). Anyone can write to it; replies follow the settings on the next steps. To use a different name, type it below. The old address stops working once you continue.",
                fields: [
                    WizardField(key: "localPart", label: "New name (optional)", placeholder: String(address.split(separator: "@").first ?? ""),
                                hint: "Letters, numbers and single hyphens, 3–40 long. The part after the @ stays \(address.split(separator: "@").last.map { "@\($0)" } ?? "the same").",
                                optional: true, keyboard: .asciiCapable),
                ],
                initial: WizardValues(),
                submitLabel: "Continue",
                onSubmit: { [unowned self] v in
                    let local = v.string("localPart").lowercased()
                    guard let bot = self.target, !local.isEmpty,
                          local != String(self.address.split(separator: "@").first ?? "") else { return nil }
                    do {
                        self.address = try await ChannelConnectAPI.changeEmailAddress(botId: bot, localPart: local)
                    } catch let e as ChannelConnectAPI.EmailAddressError {
                        if let restored = e.restored { self.address = restored }
                        self.changed()
                        throw e
                    }
                    self.changed()
                    return nil
                })),
            ConnectStep(id: "senders", title: "Who can email this bot?", kind: .form(
                body: ownerAddress.map { "Only people allowed here can make \(name) act by email. Includes you (\($0)); add anyone else below." }
                    ?? "Only people allowed here can make \(name) act by email. Add your own address too.",
                fields: [
                    WizardField(key: "senderPolicy", label: "Who can email it", kind: .radio([
                        WizardOption(value: "owner", label: "Only me and people I add", hint: "Mail from anyone else is held for you to look at."),
                        WizardOption(value: "anyone", label: "Anyone", hint: "Any sender can start a thread with the bot."),
                    ])),
                    WizardField(key: "allowedSenders", label: "Also allow", kind: .multiline, placeholder: "jane@acme.com, acme.com",
                                hint: "Email addresses or whole domains. Separate entries with commas.", optional: true,
                                keyboard: .emailAddress),
                ],
                initial: WizardValues(text: ["senderPolicy": senderPolicy, "allowedSenders": allowedSenders.joined(separator: ", ")]),
                submitLabel: "Save",
                onSubmit: { [unowned self] v in
                    guard let bot = self.target else { return nil }
                    let list = v.string("allowedSenders").split(whereSeparator: { $0 == "," || $0.isNewline })
                        .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                    do {
                        try await ChannelConnectAPI.saveEmailSenders(botId: bot, policy: v.string("senderPolicy"), allowed: list)
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t save who can email the bot: \(error.localizedDescription)")
                    }
                    self.senderPolicy = v.string("senderPolicy")
                    self.allowedSenders = list
                    return nil
                })),
            ConnectStep(id: "mode", title: "Who can the bot reply to?", kind: .form(
                body: nil,
                fields: [
                    WizardField(key: "replyMode", label: "Reply mode", kind: .radio([
                        WizardOption(value: "approve", label: "Approve each reply", hint: "Every reply waits for your approval before it sends. The safest setting, and the default."),
                        WizardOption(value: "known", label: "Auto for known senders", hint: "The bot replies on its own to people on the allowlist or already in a thread, and asks before anyone else."),
                        WizardOption(value: "auto", label: "Full auto", hint: "The bot replies on its own to anyone. Only available on a verified company domain."),
                    ])),
                    WizardField(key: "allowlist", label: "Allowlist", kind: .multiline, placeholder: "jane@acme.com, acme.com",
                                hint: "People and domains the bot may reply to on its own. Separate entries with commas.", optional: true,
                                keyboard: .emailAddress),
                ],
                initial: WizardValues(text: ["replyMode": replyMode, "allowlist": allowlist.joined(separator: ", ")]),
                submitLabel: "Save reply settings",
                onSubmit: { [unowned self] v in
                    guard let bot = self.target else { return nil }
                    let list = v.string("allowlist").split(whereSeparator: { $0 == "," || $0.isNewline })
                        .map { $0.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }
                    do {
                        try await ChannelConnectAPI.saveEmailReplies(botId: bot, mode: v.string("replyMode"), allowlist: list)
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t save the reply settings: \(error.localizedDescription)")
                    }
                    return nil
                })),
            ConnectStep(id: "domain", title: "Company domain (optional)", kind: .form(
                body: "For addresses like support@yourcompany.com. You add four DNS records where your domain is managed, and we check them live. Use a domain or subdomain that doesn’t already receive mail elsewhere: its mail will go to your bots.",
                fields: [
                    WizardField(key: "domain", label: "Domain", placeholder: "bots.acme.com",
                                hint: "Leave blank to stay on the shared address.", optional: true, keyboard: .URL),
                ],
                initial: WizardValues(),
                submitLabel: "Check domain",
                onSubmit: { [unowned self] v in
                    let domain = v.string("domain")
                    guard !domain.isEmpty else { return "done" }
                    do {
                        self.domainCheck = try await ChannelConnectAPI.setEmailDomain(domain)
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t start the domain check: \(error.localizedDescription)")
                    }
                    self.changed()
                    return "domain-status"
                })),
        ] + (domainCheck == nil ? [] : [
            ConnectStep(id: "domain-status", title: "Checking your domain", kind: .waiting(
                body: domainCheck.map(Self.recordsText), status: "Verifying DNS records…", pollSeconds: 4, skippable: true, cancelLabel: nil,
                poll: { [unowned self] in await self.pollDomain() })),
        ]) + [
            ConnectStep(id: "done", title: "Email is connected", kind: .done(
                body: "Mail to \(address) lands in \(name)’s threads. To try it, email it from your own inbox. You approve replies unless you chose otherwise.")),
        ]
    }

    private func pollDomain() async -> WaitingPoll {
        guard let current = domainCheck else { return WaitingPoll(state: .done) }
        let check: ChannelConnectAPI.DomainCheck
        do {
            check = try await ChannelConnectAPI.emailDomainStatus(current.domain)
        } catch {
            return WaitingPoll(state: .pending, detail: "Couldn’t check just now. Trying again…\n\n\(Self.recordsText(current))")
        }
        domainCheck = check
        guard check.verified else {
            let waiting = check.records.filter { $0.ok == false }.count
            let lead = waiting > 0 ? "\(waiting) of \(check.records.count) records not found yet. " : ""
            return WaitingPoll(state: .pending, detail: "\(lead)DNS can take a few minutes to update. You can keep waiting or skip; email works on the shared address meanwhile.\n\n\(Self.recordsText(check))")
        }
        guard let bot = target, !check.domain.isEmpty, !address.lowercased().hasSuffix("@\(check.domain)") else {
            return WaitingPoll(state: .done, detail: "Records verified. Your domain is ready for bot addresses.")
        }
        do {
            address = try await ChannelConnectAPI.moveEmailToDomain(botId: bot, address: address, domain: check.domain)
            changed()
            return WaitingPoll(state: .done, detail: "Records verified. The bot’s address is now \(address).")
        } catch let e as ChannelConnectAPI.EmailAddressError {
            if let restored = e.restored { address = restored }
            changed()
            return WaitingPoll(state: .failed, error: e.message)
        } catch {
            return WaitingPoll(state: .failed, error: "The records verified, but the bot couldn’t move to the domain. Try again.")
        }
    }

    /// The DNS records to add, one block each (press and hold to copy).
    nonisolated private static func recordsText(_ check: ChannelConnectAPI.DomainCheck) -> String {
        check.records.map { r in
            let found = r.ok == true ? " · found" : r.ok == false ? " · not yet" : ""
            let purpose = r.purpose.map { " (\($0))" } ?? ""
            return "\(r.type)\(found)\(purpose)\nHost: \(r.host)\nValue: \(r.value)"
        }.joined(separator: "\n\n")
    }

    override func finish() async {
        await BotPhonesStore.shared.refresh()
    }
}

// MARK: - Telegram (TelegramWizard.tsx)

@MainActor
final class TelegramConnectFlow: ConnectFlow {
    private var managed = true
    private var onboardingId: String?
    private var confirmURL: URL?

    init(botId: String?) {
        super.init(title: "Connect Telegram", icon: "paperplane", botId: botId, firstStep: "intro")
    }

    override func prepare() async {
        managed = await ChannelConnectAPI.telegramManagedAvailable()
        if !managed { step = "token" }
    }

    override var steps: [ConnectStep] {
        let botName = botId.flatMap { BotModeStore.shared.bot($0)?.botDisplayName }
        if !managed {
            // Managed bots aren't on this computer: the token form (ChannelConnectForm).
            return [
                ConnectStep(id: "token", title: "Connect your Telegram bot", kind: .form(
                    body: "Create a bot with @BotFather in Telegram (send /newbot), then paste the token it gives you.",
                    fields: [
                        WizardField(key: "botToken", label: "Bot token", kind: .secure, placeholder: "123456:ABC-DEF…"),
                        WizardField(key: "displayName", label: "Name", placeholder: botName.map { "\($0) (Telegram)" }, optional: true),
                    ],
                    initial: WizardValues(),
                    submitLabel: "Connect",
                    onSubmit: { [unowned self] v in
                        var body: [String: JSONValue] = ["botToken": .string(v.string("botToken"))]
                        if !v.string("displayName").isEmpty { body["displayName"] = .string(v.string("displayName")) }
                        let r = try await ConnectHTTP.call("POST", "gateway/channel-accounts/telegram", body: .object(body))
                        self.accountId = r.at("account")?.at("id")?.jString
                        return nil
                    })),
                ConnectStep(id: "done", title: "Telegram is connected", kind: .done(
                    body: "Your Telegram bot answers the Allternit bots you switch it on for. Messages wait while your computer is off and arrive in order.")),
            ]
        }
        return [
            ConnectStep(id: "intro", title: "A real Telegram bot, without a token", kind: .intro(
                body: "Allternit creates a Telegram bot for you through Telegram’s managed-bot program. You never paste a token.",
                bullets: ["Name the bot and we create it on Telegram.",
                          "Tap Confirm once in the Telegram chat that opens.",
                          "Open your new bot and tap Start to pair it with Allternit."])),
            ConnectStep(id: "name", title: "Name your Telegram bot", kind: .form(
                body: nil,
                fields: [WizardField(key: "name", label: "Bot name", placeholder: botName.map { "\($0) (Telegram)" } ?? "Acme Support",
                                     hint: "Shown as the bot’s display name in Telegram.")],
                initial: WizardValues(text: botName.map { ["name": "\($0) (Telegram)"] } ?? [:]),
                submitLabel: "Create the bot",
                onSubmit: { [unowned self] v in
                    do {
                        let made = try await ChannelConnectAPI.createManagedTelegramBot(name: v.string("name"), botId: self.botId)
                        self.onboardingId = made.id
                        self.confirmURL = made.confirmURL
                        self.changed()
                    } catch let e as ConnectError where e.code == "runtime_not_found" {
                        throw e
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Telegram didn’t accept that (\(error.localizedDescription)). Try a different name.")
                    }
                    return nil
                })),
            ConnectStep(id: "pair", title: "Pairing with Telegram", kind: .waiting(
                body: nil, status: "Waiting for Telegram…", pollSeconds: 3, skippable: false, cancelLabel: "Cancel connection",
                poll: { [unowned self] in
                    guard let id = self.onboardingId else { return WaitingPoll(state: .pending, detail: "Creating your bot…") }
                    let s = try await ChannelConnectAPI.managedTelegramStatus(id)
                    if s.paired { return WaitingPoll(state: .done) }
                    if let pair = s.pairURL {
                        return WaitingPoll(state: .pending, detail: "Your bot is created. Open it in Telegram and tap Start so Allternit knows it is yours.",
                                           action: (label: "Open your new bot in Telegram", url: pair))
                    }
                    return WaitingPoll(state: .pending,
                                       detail: "Tap Confirm in the Telegram chat that opens. This is the only step that happens in Telegram.",
                                       action: self.confirmURL.map { (label: "Open Telegram to confirm", url: $0) })
                })),
            ConnectStep(id: "done", title: "Telegram is connected", kind: .done(
                body: "Your new bot answers the Allternit bots you switch it on for. Messages wait while your computer is off and arrive in order.")),
        ]
    }
}

// MARK: - Slack, Discord, Teams, WhatsApp Business (shared-app OAuth)

@MainActor
final class OAuthChannelFlow: ConnectFlow {
    let provider: ChannelProvider
    private var start: ChannelConnectStart?
    private var guildId: String?
    private var channels: [(id: String, name: String, selected: Bool)] = []
    private var workspace: String?

    init(provider: ChannelProvider, botId: String?) {
        self.provider = provider
        super.init(title: "Connect \(provider.label)", icon: provider.icon, botId: botId, firstStep: "add")
    }

    override func prepare() async {
        do {
            start = try await ChannelConnectAPI.start(provider.rawValue)
            if start == nil {
                comingSoon = "\(provider.blurb) This connection type isn’t available on your account yet — it ships with an upcoming update."
            }
        } catch {
            comingSoon = "Couldn’t start \(provider.label): \(error.localizedDescription)"
        }
    }

    private var copy: (title: String, body: String, label: String, hint: String, waiting: String, done: String) {
        switch provider {
        case .slack:
            return ("Add Allternit to Slack", "You authorize the Allternit app on Slack’s own page. Pick your workspace, then close the page to come back.",
                    "Add to Slack", "If your workspace needs admin approval, Slack tells you right away — your admin gets the request, and this keeps waiting for the grant.",
                    "Waiting for Slack…", "The bots answer where you invite them. Mention a bot by name to hand it a conversation, or DM it directly.")
        case .discord:
            return ("Add Allternit to your server", "You authorize the Allternit app on Discord’s own page. Pick your server, then close the page to come back.",
                    "Add to Discord", "Discord asks for a server and the permissions the bots need. The app’s token stays in Allternit’s cloud, never on your computer.",
                    "Waiting for Discord…", "The bots answer in the channels you allowed. \"@botname\" in a message picks who answers.")
        case .teams:
            return ("Sign in with Microsoft", "You sign in on Microsoft’s own page and consent for yourself. If your org blocks new apps, ask your IT admin to allow \"Allternit\" in the Teams admin center → Teams apps.",
                    "Connect Teams", "A work or school account works. Allternit asks for the profile and the ability to chat as you.",
                    "Waiting for Microsoft…", "Teams sees one app (@Allternit), so start a message with the bot’s name — \"@Allternit researcher, draft the reply\".")
        default:
            return ("Connect your WhatsApp Business number", "You connect the number on Meta’s own signup page, then close the page to come back.",
                    "Continue with Meta", "You need a WhatsApp Business account. Personal numbers aren’t supported on the phone.",
                    "Waiting for Meta…", "Your bots answer on your WhatsApp Business number.")
        }
    }

    override var steps: [ConnectStep] {
        guard let start else { return [] }
        let c = copy
        var out = [
            ConnectStep(id: "add", title: c.title, kind: .action(body: c.body, label: c.label, url: start.authorizeURL,
                                                              hint: c.hint, doneLabel: "I’ve finished", signIn: true)),
            ConnectStep(id: "grant", title: "Finishing the connection", kind: .waiting(
                body: nil, status: c.waiting, pollSeconds: 3, skippable: false, cancelLabel: "Cancel connection",
                poll: { [unowned self] in
                    switch try await ChannelConnectAPI.status(self.provider.rawValue) {
                    case .waiting:
                        return WaitingPoll(state: .pending, detail: "Finish the page that opened if you haven’t. Nothing else to do here.",
                                           action: (label: "Open the page again", url: start.authorizeURL))
                    case .connected(let account, let workspace, let handle):
                        self.accountId = account
                        self.workspace = workspace
                        if self.provider == .discord, let handle {
                            self.guildId = handle
                            self.channels = (try? await ChannelConnectAPI.discordChannels(guildId: handle)) ?? []
                        }
                        self.changed()
                        return WaitingPoll(state: .done, detail: workspace.map { "Connected to \($0)." } ?? "Connected.")
                    }
                })),
        ]
        if provider == .discord, !channels.isEmpty {
            out.append(ConnectStep(id: "channels", title: "Which channels can the bots use?", kind: .form(
                body: "The bots only answer where you allow; messages in other channels never reach your computer. Direct messages and slash commands always work.",
                fields: [WizardField(key: "channels", label: "Channels", kind: .checkboxes(channels.map { WizardOption(value: $0.id, label: "#\($0.name)") }),
                                     hint: "Pick at least one. \"@name\" in a message chooses which bot answers.")],
                initial: WizardValues(many: ["channels": Set(channels.filter(\.selected).map(\.id))]),
                submitLabel: "Save channels",
                onSubmit: { [unowned self] v in
                    let picked = Array(v.many["channels"] ?? [])
                    guard let guild = self.guildId else { throw ConnectError(status: nil, code: nil, message: "The server isn’t connected yet.") }
                    do {
                        // Every channel ticked = no limit, so channels made later work too.
                        try await ChannelConnectAPI.saveDiscordChannels(guildId: guild, ids: picked.count == self.channels.count ? [] : picked)
                    } catch {
                        throw ConnectError(status: nil, code: nil, message: "Couldn’t save the channels: \(error.localizedDescription). The bots keep answering in every channel; try again.")
                    }
                    return nil
                })))
        }
        out.append(ConnectStep(id: "done", title: "\(provider.label) is connected", kind: .done(
            body: (workspace.map { "Connected to \($0). " } ?? "") + c.done)))
        return out
    }
}

// MARK: - "Give a bot a phone"

/// Pick how a bot is reached — a number, email or a messaging app — and run
/// that connect flow natively (web PhonesRailButton / BotPhoneReachSection).
struct GiveBotPhoneSheet: View {
    /// The bot this was opened for; nil = the flow asks (or uses the first bot).
    var botId: String?

    @Environment(\.dismiss) private var dismiss
    @ObservedObject private var auth = AuthManager.shared
    @ObservedObject private var relay = RuntimeRelayStore.shared
    @ObservedObject private var botStore = BotModeStore.shared
    @State private var pickedBot: String?
    @State private var flow: ConnectFlow?

    private var target: String? { botId ?? pickedBot }

    var body: some View {
        NavigationStack {
            List {
                if !auth.isSignedIn {
                    FriendlyStateView(style: .empty, icon: "person.crop.circle", title: "Sign in to connect",
                                      message: "Numbers, email addresses and channels belong to your Allternit account.",
                                      actionTitle: "Sign in", action: {
                                          dismiss()
                                          auth.isPresentingAuth = true
                                      })
                        .listRowBackground(Color.clear)
                } else {
                    if relay.availability == .none || relay.availability == .unreachable {
                        Section {
                            Label(relay.availability == .none
                                  ? "Connect a computer first: your bots and their connections live on it."
                                  : "Your computer is offline. Numbers can still be bought; email and messaging apps finish once it’s back.",
                                  systemImage: "desktopcomputer.trianglebadge.exclamationmark")
                                .font(.footnote)
                                .foregroundColor(Color("TextSecondary"))
                        }
                    }
                    if botId == nil {
                        Section("Which bot?") {
                            if botStore.bots.isEmpty {
                                Text("You don’t have a bot yet. Make one in Bots first.")
                                    .font(.footnote).foregroundColor(Color("TextSecondary"))
                            }
                            ForEach(botStore.bots, id: \.id) { bot in
                                Button {
                                    pickedBot = bot.id
                                } label: {
                                    HStack(spacing: 12) {
                                        BotAvatarView(bot: bot, size: 28)
                                        Text(bot.botDisplayName).foregroundColor(Color("TextPrimary"))
                                        Spacer()
                                        if target == bot.id {
                                            Image(systemName: "checkmark").foregroundColor(Color("AccentPrimary"))
                                        }
                                    }
                                    .contentShape(Rectangle())
                                }
                                .buttonStyle(.plain)
                                .accessibilityAddTraits(target == bot.id ? .isSelected : [])
                            }
                        }
                    }
                    Section {
                        ForEach(ChannelProvider.allCases) { provider in
                            Button {
                                flow = provider.flow(botId: target)
                            } label: {
                                HStack(alignment: .top, spacing: 12) {
                                    Image(systemName: provider.icon)
                                        .font(.system(size: 15))
                                        .foregroundColor(Color("TextPrimary"))
                                        .frame(width: 32, height: 32)
                                        .background(Circle().fill(Color("BgSecondary")))
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(provider.label).font(.subheadline.weight(.semibold)).foregroundColor(Color("TextPrimary"))
                                        Text(provider.blurb).font(.caption).foregroundColor(Color("TextSecondary"))
                                    }
                                    Spacer(minLength: 0)
                                    Image(systemName: "chevron.right").font(.caption).foregroundColor(Color("TextSecondary"))
                                }
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .disabled(botStore.bots.isEmpty)
                        }
                    } header: {
                        Text("How people reach \(target.flatMap { botStore.bot($0)?.botDisplayName } ?? "your bot")")
                    }
                }
            }
            .navigationTitle("Give a bot a phone")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } } }
            .task {
                if pickedBot == nil { pickedBot = botStore.bots.first?.id }
                await relay.refresh()
            }
            .sheet(item: $flow) { flow in
                ConnectWizardView(flow: flow) { self.flow = nil }
            }
        }
    }
}
