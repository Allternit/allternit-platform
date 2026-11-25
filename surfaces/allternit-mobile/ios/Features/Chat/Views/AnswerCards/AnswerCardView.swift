import SwiftUI

/// Draws one answer envelope under a reply: a lookup fact card or a writing
/// draft. A template this app has no view for shows its title, summary and,
/// for lookup, the tool's own text.
struct AnswerCardView: View {
    let card: AnswerCardEnvelope
    /// The tool's text, shown when the template has no view here.
    var fallbackText: String? = nil

    var body: some View {
        switch card.payload {
        case .weather(let data): WeatherAnswerCard(title: card.title, data: data)
        case .timeZones(let data): TimeZonesAnswerCard(title: card.title, data: data)
        case .conversion(let data): ConversionAnswerCard(title: card.title, data: data)
        case .linkPreview(let data): LinkPreviewAnswerCard(data: data)
        case .places(let data): PlacesAnswerCard(title: card.title, data: data)
        case .imageGallery(let data): ImageGalleryAnswerCard(title: card.title, data: data)
        case .writing(let draft): WritingDraftCard(title: card.title, draft: draft)
        case .other: TextAnswerCard(title: card.title, summary: card.summary, text: fallbackText)
        }
    }
}

// MARK: - Shared chrome

/// The bordered card every answer sits in (same look as the reply's other cards).
struct AnswerCardChrome<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 10) { content }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color("BgPrimary"))
            .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).stroke(Color("BorderSubtle"), lineWidth: 1))
            .accessibilityElement(children: .contain)
    }
}

/// The card's small heading row.
struct AnswerCardHeader: View {
    let symbol: String
    let title: String

    var body: some View {
        Label {
            Text(title)
                .foregroundColor(Color("TextSecondary"))
                .lineLimit(2)
        } icon: {
            Image(systemName: symbol)
                .foregroundColor(Color("AccentPrimary"))
        }
        .font(.caption.weight(.semibold))
        .accessibilityAddTraits(.isHeader)
    }
}

/// The provider credit, always shown; a link when the provider gave one.
struct AnswerCreditLine: View {
    let credit: String
    var url: String? = nil

    var body: some View {
        if let url, let link = AnswerLinks.https(url) {
            Link(destination: link) {
                Text(credit).underline()
            }
            .font(.caption2)
            .foregroundColor(Color("TextSecondary"))
            .accessibilityHint("Opens the source")
        } else {
            Text(credit)
                .font(.caption2)
                .foregroundColor(Color("TextSecondary"))
        }
    }
}

enum AnswerLinks {
    /// Only https links open from a card (the catalog allows nothing else).
    static func https(_ raw: String?) -> URL? {
        guard let raw, let url = URL(string: raw), url.scheme?.lowercased() == "https", url.host != nil else { return nil }
        return url
    }

    /// An Apple Maps search for a place: by coordinates when known.
    static func maps(_ place: AnswerPlaces.Place) -> URL? {
        var components = URLComponents(string: "https://maps.apple.com/")
        var items = [URLQueryItem(name: "q", value: place.name)]
        if let lat = place.lat, let lon = place.lon {
            items.append(URLQueryItem(name: "ll", value: "\(lat),\(lon)"))
        } else {
            items[0] = URLQueryItem(name: "q", value: "\(place.name), \(place.address)")
        }
        components?.queryItems = items
        return components?.url
    }

    static func number(_ value: Double) -> String {
        value.formatted(.number.precision(.fractionLength(0...4)))
    }
}

// MARK: - Weather

struct WeatherAnswerCard: View {
    let title: String
    let data: AnswerWeather

    private var degrees: String { "\(Int(data.temperature.rounded()))°\(data.unit)" }

    var body: some View {
        AnswerCardChrome {
            AnswerCardHeader(symbol: "cloud.sun", title: data.location)

            HStack(alignment: .firstTextBaseline, spacing: 12) {
                Text(degrees)
                    .font(.system(.largeTitle, design: .rounded).weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                VStack(alignment: .leading, spacing: 2) {
                    Text(data.conditions)
                        .font(.headline)
                        .foregroundColor(Color("TextPrimary"))
                    Text("Humidity \(Int(data.humidity.rounded()))% · Wind \(data.wind)")
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(data.location): \(degrees), \(data.conditions). Humidity \(Int(data.humidity.rounded())) percent. Wind \(data.wind).")

            if let days = data.forecast, !days.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(Array(days.enumerated()), id: \.offset) { _, day in
                            forecastDay(day)
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Forecast")
            }

            Text(data.observedAt)
                .font(.caption2)
                .foregroundColor(Color("TextSecondary"))
            AnswerCreditLine(credit: data.credit ?? data.source, url: data.creditUrl)
        }
    }

    private func forecastDay(_ day: AnswerWeather.Day) -> some View {
        VStack(spacing: 4) {
            Text(day.day)
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
                .lineLimit(1)
            Image(systemName: Self.symbol(for: day.conditions))
                .font(.body)
                .symbolRenderingMode(.multicolor)
                .accessibilityHidden(true)
            Text("\(Int(day.high.rounded()))°")
                .font(.subheadline.weight(.semibold))
                .foregroundColor(Color("TextPrimary"))
            Text("\(Int(day.low.rounded()))°")
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))
            if let rain = day.precipitation {
                Text(rain)
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
                    .lineLimit(1)
            }
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 10)
        .frame(minWidth: 56)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(Color("FillNeutral")))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(day.day): high \(Int(day.high.rounded())), low \(Int(day.low.rounded())), \(day.conditions)\(day.precipitation.map { ", \($0)" } ?? "")")
    }

    /// SF Symbol for a conditions phrase ("Light rain", "Partly cloudy").
    static func symbol(for conditions: String) -> String {
        let c = conditions.lowercased()
        if c.contains("thunder") { return "cloud.bolt.rain.fill" }
        if c.contains("snow") || c.contains("sleet") { return "cloud.snow.fill" }
        if c.contains("rain") || c.contains("shower") || c.contains("drizzle") { return "cloud.rain.fill" }
        if c.contains("fog") || c.contains("mist") { return "cloud.fog.fill" }
        if c.contains("partly") || c.contains("fair") { return "cloud.sun.fill" }
        if c.contains("cloud") || c.contains("overcast") { return "cloud.fill" }
        if c.contains("clear") || c.contains("sun") { return "sun.max.fill" }
        return "thermometer.medium"
    }
}

// MARK: - Time zones

struct TimeZonesAnswerCard: View {
    let title: String
    let data: AnswerTimeZones

    var body: some View {
        AnswerCardChrome {
            AnswerCardHeader(symbol: "globe", title: title.isEmpty ? data.reference : title)
            if !title.isEmpty {
                Text(data.reference)
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }
            VStack(spacing: 0) {
                ForEach(Array(data.zones.enumerated()), id: \.offset) { index, zone in
                    if index > 0 { Divider() }
                    HStack(alignment: .firstTextBaseline) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(zone.place)
                                .font(.subheadline.weight(.semibold))
                                .foregroundColor(Color("TextPrimary"))
                            Text("\(zone.date) · \(zone.offset)")
                                .font(.caption)
                                .foregroundColor(Color("TextSecondary"))
                        }
                        Spacer(minLength: 8)
                        Text(zone.time)
                            .font(.title3.weight(.semibold).monospacedDigit())
                            .foregroundColor(Color("TextPrimary"))
                    }
                    .padding(.vertical, 8)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel("\(zone.place): \(zone.time), \(zone.date), \(zone.offset)")
                }
            }
            AnswerCreditLine(credit: data.credit)
        }
    }
}

// MARK: - Conversion

struct ConversionAnswerCard: View {
    let title: String
    let data: AnswerConversion

    private var fromText: String { "\(AnswerLinks.number(data.from.value)) \(data.from.unit)" }
    private var toText: String { "\(AnswerLinks.number(data.to.value)) \(data.to.unit)" }

    var body: some View {
        AnswerCardChrome {
            AnswerCardHeader(symbol: data.kind == "currency" ? "dollarsign.arrow.circlepath" : "arrow.left.arrow.right", title: title.isEmpty ? "Conversion" : title)

            VStack(alignment: .leading, spacing: 2) {
                Text(fromText)
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
                Text("= \(toText)")
                    .font(.title2.weight(.semibold))
                    .foregroundColor(Color("TextPrimary"))
                    .textSelection(.enabled)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(fromText) equals \(toText)")

            if data.rate != nil || data.asOf != nil {
                Text([data.rate.map { "Rate \(AnswerLinks.number($0))" }, data.asOf.map { "as of \($0)" }].compactMap { $0 }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundColor(Color("TextSecondary"))
            }

            if let also = data.also, !also.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(Array(also.enumerated()), id: \.offset) { _, amount in
                        Text("\(AnswerLinks.number(amount.value)) \(amount.unit)")
                            .font(.subheadline.monospacedDigit())
                            .foregroundColor(Color("TextPrimary"))
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Also")
            }

            AnswerCreditLine(credit: data.credit, url: data.creditUrl)
        }
    }
}

// MARK: - Link preview

struct LinkPreviewAnswerCard: View {
    let data: AnswerLinkPreview
    @Environment(\.openURL) private var openURL

    var body: some View {
        let link = AnswerLinks.https(data.url)
        Button {
            if let link { openURL(link) }
        } label: {
            AnswerCardChrome {
                if let image = AnswerLinks.https(data.image) {
                    AsyncImage(url: image) { phase in
                        switch phase {
                        case .success(let picture):
                            picture.resizable().scaledToFill()
                        case .failure:
                            EmptyView()
                        default:
                            Color("FillNeutral")
                        }
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 150)
                    .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                    .accessibilityLabel(data.imageAlt ?? data.title)
                }
                if data.siteName != nil || data.published != nil {
                    Text([data.siteName, data.published].compactMap { $0 }.joined(separator: " · "))
                        .font(.caption)
                        .foregroundColor(Color("TextSecondary"))
                }
                Text(data.title)
                    .font(.headline)
                    .foregroundColor(Color("TextPrimary"))
                    .multilineTextAlignment(.leading)
                if let description = data.description {
                    Text(description)
                        .font(.subheadline)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(4)
                        .multilineTextAlignment(.leading)
                }
                // The card's one main button; the whole card opens the same page.
                HStack(spacing: 6) {
                    Text("Open").lineLimit(1)
                    Text(link?.host ?? data.url).lineLimit(1).opacity(0.7)
                    Image(systemName: "arrow.up.right")
                        .accessibilityHidden(true)
                }
                .font(.caption.weight(.semibold))
                .foregroundColor(Color("BgPrimary"))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(Capsule().fill(Color("TextPrimary")))
                Text(data.credit)
                    .font(.caption2)
                    .foregroundColor(Color("TextSecondary"))
            }
        }
        .buttonStyle(.plain)
        .disabled(link == nil)
        .accessibilityLabel("\(data.title)\(data.siteName.map { ", \($0)" } ?? "")")
        .accessibilityHint("Opens the page in Safari")
    }
}

// MARK: - Places

struct PlacesAnswerCard: View {
    let title: String
    let data: AnswerPlaces
    @Environment(\.openURL) private var openURL

    var body: some View {
        AnswerCardChrome {
            AnswerCardHeader(symbol: "mappin.and.ellipse", title: title.isEmpty ? data.query : title)
            Text(data.near.map { "\(data.query) · near \($0)" } ?? data.query)
                .font(.caption)
                .foregroundColor(Color("TextSecondary"))

            if data.places.isEmpty {
                Text("No places found.")
                    .font(.subheadline)
                    .foregroundColor(Color("TextSecondary"))
            }
            VStack(spacing: 0) {
                ForEach(Array(data.places.enumerated()), id: \.offset) { index, place in
                    if index > 0 { Divider() }
                    Button {
                        if let url = AnswerLinks.maps(place) { openURL(url) }
                    } label: {
                        HStack(alignment: .top, spacing: 10) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(place.name)
                                    .font(.subheadline.weight(.semibold))
                                    .foregroundColor(Color("TextPrimary"))
                                if let meta = Self.meta(place) {
                                    Text(meta)
                                        .font(.caption)
                                        .foregroundColor(Color("TextSecondary"))
                                }
                                Text(place.address)
                                    .font(.caption)
                                    .foregroundColor(Color("TextSecondary"))
                                    .multilineTextAlignment(.leading)
                            }
                            Spacer(minLength: 8)
                            Image(systemName: "map")
                                .font(.body)
                                .foregroundColor(Color("AccentPrimary"))
                                .accessibilityHidden(true)
                        }
                        .padding(.vertical, 8)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel([place.name, Self.meta(place), place.address].compactMap { $0 }.joined(separator: ", "))
                    .accessibilityHint("Opens Maps")
                }
            }
            AnswerCreditLine(credit: data.credit, url: data.creditUrl)
        }
    }

    private static func meta(_ place: AnswerPlaces.Place) -> String? {
        let parts = [place.category, place.distance].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }
}

// MARK: - Image gallery

struct ImageGalleryAnswerCard: View {
    let title: String
    let data: AnswerImageGallery
    @Environment(\.openURL) private var openURL

    var body: some View {
        AnswerCardChrome {
            AnswerCardHeader(symbol: "photo.on.rectangle", title: title.isEmpty ? "Images" : title)
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(alignment: .top, spacing: 10) {
                    ForEach(Array(data.images.enumerated()), id: \.offset) { _, image in
                        tile(image)
                    }
                }
            }
        }
    }

    private func tile(_ image: AnswerImageGallery.Image) -> some View {
        let source = AnswerLinks.https(image.sourceUrl) ?? AnswerLinks.https(image.url)
        let creditLine = [image.credit, image.license].compactMap { $0 }.joined(separator: " · ")
        return Button {
            if let source { openURL(source) }
        } label: {
            VStack(alignment: .leading, spacing: 4) {
                AsyncImage(url: AnswerLinks.https(image.url)) { phase in
                    switch phase {
                    case .success(let picture):
                        picture.resizable().scaledToFill()
                    case .failure:
                        Image(systemName: "photo")
                            .foregroundColor(Color("TextSecondary"))
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .background(Color("FillNeutral"))
                    default:
                        Color("FillNeutral")
                    }
                }
                .frame(width: 200, height: 150)
                .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
                Text(image.caption)
                    .font(.caption)
                    .foregroundColor(Color("TextPrimary"))
                    .lineLimit(2)
                    .multilineTextAlignment(.leading)
                if !creditLine.isEmpty {
                    Text(creditLine)
                        .font(.caption2)
                        .foregroundColor(Color("TextSecondary"))
                        .lineLimit(2)
                        .multilineTextAlignment(.leading)
                }
            }
            .frame(width: 200, alignment: .leading)
        }
        .buttonStyle(.plain)
        .disabled(source == nil)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel([image.alt, image.caption, creditLine.isEmpty ? nil : creditLine].compactMap { $0 }.joined(separator: ". "))
        .accessibilityAddTraits(.isImage)
        .accessibilityHint(source == nil ? "" : "Opens the image source")
    }
}

// MARK: - Text fallback

/// A template this app has no view for: its title, summary and the tool's text.
struct TextAnswerCard: View {
    let title: String
    let summary: String?
    let text: String?

    var body: some View {
        if title.isEmpty, summary == nil, (text ?? "").isEmpty {
            EmptyView()
        } else {
            AnswerCardChrome {
                if !title.isEmpty {
                    Text(title)
                        .font(.subheadline.weight(.semibold))
                        .foregroundColor(Color("TextPrimary"))
                }
                if let summary {
                    Text(summary)
                        .font(.subheadline)
                        .foregroundColor(Color("TextPrimary"))
                        .textSelection(.enabled)
                }
                if let text, !text.isEmpty, text != summary {
                    Text(text)
                        .font(.footnote)
                        .foregroundColor(Color("TextSecondary"))
                        .textSelection(.enabled)
                }
            }
        }
    }
}
