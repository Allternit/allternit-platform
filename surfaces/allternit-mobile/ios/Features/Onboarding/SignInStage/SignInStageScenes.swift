import SwiftUI

/// The four scenes of the sign-in stage: static replicas of the real
/// Allternit views (Home, Bots, Code, ACI) on a fresh account, each building
/// itself from the web beat sheet (allternit-ai scenes.tsx +
/// sign-in-stage.css). Nothing here talks to an API. Every scene is laid out
/// on the 694 × 470 canvas; positions are the web's `left` / `top` values.

enum StageScene {
    static let width: CGFloat = 694
    static let height: CGFloat = 470

    /// Scene root: halos behind, the app window, then rings and bursts on top.
    struct Layers<Back: View, Window: View, Front: View>: View {
        @ViewBuilder let back: Back
        @ViewBuilder let window: Window
        @ViewBuilder let front: Front

        var body: some View {
            ZStack(alignment: .topLeading) {
                back
                window
                    .beat(.pop, at: 0.05)
                    .place(12, 12)
                front
            }
            .frame(width: StageScene.width, height: StageScene.height, alignment: .topLeading)
        }
    }
}

/// The container that lifts away (and blurs out) when the result lands.
private struct StageAway<Content: View>: View {
    let at: Double
    @ViewBuilder let content: Content

    var body: some View {
        ZStack(alignment: .topLeading) { content }
            .frame(width: StageLayout.mainWidth, height: StageLayout.mainHeight, alignment: .topLeading)
            .beat(.away, at: at)
    }
}

private let mainWidth = StageLayout.mainWidth

// MARK: - Home

struct StageHomeScene: View {
    private let chips = ["Build a site", "Make a deck", "Research it", "Make a sheet", "Put a team on it"]

    var body: some View {
        StageScene.Layers {
            StageHalo(x: 430, y: -40, w: 330, rgb: (217, 119, 87), alpha: 0.3)
            StageHalo(x: -60, y: 220, w: 300, rgb: (242, 157, 56), alpha: 0.22)
        } window: {
            StageAppWindow {
                StageRailItem(icon: .plus, label: "New", isNew: true)
                StageRailItem(icon: .folder, label: "Projects")
                StageRailItem(icon: .doc, label: "Artifacts")
                StageRailItem(icon: .clock, label: "Automation Tasks")
                StageRailItem(icon: .sliders, label: "Customize")
                StageRailItem(icon: .sliders, label: "More")
                StageRailLabel(title: "Recents")
                HStack(spacing: 6) {
                    StageDot()
                    Text("Plan our launch week").fontWeight(.semibold).lineLimit(1)
                }
                .font(StageFont.sans(10.5))
                .padding(.horizontal, 6)
                .frame(maxWidth: .infinity, minHeight: 20, maxHeight: 20, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 5).fill(Color(hex: "#F6F2EE")))
                .beat(.slideIn, at: 2.7)
            } main: {
                StageAway(at: 2.45) {
                    HStack(spacing: 10) {
                        BlockMark(fill: StageInk.amber, size: 20)
                        Text("What\u{2019}s on your mind?").font(StageFont.serif(25))
                    }
                    .frame(width: mainWidth)
                    .beat(.pop, at: 0.25)
                    .place(0, 122)

                    VStack(spacing: 6) {
                        HStack(spacing: 6) {
                            ForEach(0..<4, id: \.self) { i in chip(i) }
                        }
                        chip(4)
                    }
                    .frame(width: 380)
                    .place(70, 200)
                }

                // The composer docks to the bottom as the reply lands.
                ZStack(alignment: .topLeading) {
                    StageTyping(
                        placeholder: "Draft the email you've been putting off",
                        text: "Plan our launch week and draft the announcement",
                        at: 1.0
                    )
                    .place(12, 11)
                    HStack(spacing: 6) {
                        HStack(spacing: 4) {
                            StageIcon(name: .plus)
                            Text("Add files")
                        }
                        .foregroundStyle(StageInk.brown2)
                        .padding(.horizontal, 7)
                        .frame(height: 20)
                        .background(RoundedRectangle(cornerRadius: 6).fill(StageInk.chip))
                        Text("Auto")
                            .fontWeight(.semibold)
                            .padding(.horizontal, 7)
                            .frame(height: 14)
                            .background(RoundedRectangle(cornerRadius: 4).fill(StageInk.chip))
                            .padding(2)
                            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(StageInk.border))
                        StageModelPicker(label: "Allternit Cloud · Kimi K3")
                        StageVSep().padding(.leading, 2)
                        StageIcon(name: .wave, color: StageInk.text)
                        StageSend(at: 2.25)
                    }
                    .font(StageFont.sans(9.5))
                    .foregroundStyle(StageInk.brown)
                    .fixedSize()
                    .place(8, 72 - 7 - 22)
                }
                .frame(width: 390, height: 72, alignment: .topLeading)
                .background(
                    RoundedRectangle(cornerRadius: 12)
                        .fill(Color.white)
                        .shadow(color: StageInk.shadow.opacity(0.07), radius: 11, y: 8)
                )
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(StageInk.border))
                .beat(.pop, at: 0.6)
                .beat(.dock, at: 2.45)
                .place(66, 268)

                Text("Plan our launch week and draft the announcement")
                    .font(StageFont.sans(10.5))
                    .lineSpacing(2)
                    .foregroundStyle(.white)
                    .padding(.vertical, 7)
                    .padding(.horizontal, 11)
                    .frame(maxWidth: 260, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                    .background(
                        UnevenRoundedRectangle(topLeadingRadius: 13, bottomLeadingRadius: 13, bottomTrailingRadius: 3, topTrailingRadius: 13)
                            .fill(StageInk.text)
                    )
                    .beat(.rise, at: 2.55)
                    .frame(width: mainWidth - 22, alignment: .trailing)
                    .place(0, 22)

                HStack(alignment: .top, spacing: 8) {
                    StageOrb(state: .composing, size: 14)
                        .frame(width: 24, height: 24)
                        .background(RoundedRectangle(cornerRadius: 7).fill(StageInk.chip))
                        .beat(.pop, at: 2.75)
                    VStack(alignment: .leading, spacing: 5) {
                        Text("Here\u{2019}s a plan for launch week.").fontWeight(.bold).beat(.slideIn, at: 2.9)
                        Text("Mon–Tue: brief the team and lock the message.").beat(.slideIn, at: 3.05)
                        Text("Wed: publish the announcement. Thu–Fri: follow up.").beat(.slideIn, at: 3.2)
                        HStack(spacing: 6) {
                            StageOrb(state: .searching, size: 10)
                            Text("Searched 6 sources")
                            Text("✓").fontWeight(.bold).foregroundStyle(StageInk.green)
                        }
                        .font(StageFont.mono(9.5))
                        .foregroundStyle(StageInk.brown)
                        .padding(.horizontal, 8)
                        .frame(height: 20)
                        .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(StageInk.border))
                        .beat(.pop, at: 3.35)
                    }
                    .font(StageFont.sans(10.5))
                    .lineSpacing(2)
                    .padding(.top, 3)
                }
                .frame(width: 290, alignment: .leading)
                .place(22, 68)

                homeDoc
                    .beat(.tilt, at: 3.55)
                    .place(332, 58)
            }
        } front: {
            StageRing(x: 598, y: 335, size: 56, at: 2.3)
            StageBurst(x: 598, y: 335, at: 2.3, sparks: [
                .init(33, 10, "#D97757", -180), .init(25, 34, "#F29D38", -83), .init(1, 51, "#2A1F16", 14), .init(-22, 31, "#E8906F", 111),
                .init(-44, 15, "#F2B33D", -152), .init(-33, -10, "#D97757", -55), .init(-25, -34, "#F29D38", 42), .init(-1, -51, "#2A1F16", 139),
                .init(22, -31, "#E8906F", -124), .init(44, -15, "#F2B33D", -27),
            ])
            StageBurst(x: 575, y: 135, at: 3.6, sparks: [
                .init(39, 12, "#D97757", -180), .init(30, 40, "#F29D38", -83), .init(1, 59, "#E8906F", 14), .init(-26, 37, "#F2B33D", 111),
                .init(-52, 18, "#D97757", -152), .init(-39, -12, "#F29D38", -55), .init(-30, -40, "#E8906F", 42), .init(-1, -59, "#F2B33D", 139),
                .init(26, -37, "#D97757", -124), .init(52, -18, "#F29D38", -27),
            ])
        }
    }

    private func chip(_ i: Int) -> some View {
        Text(chips[i])
            .font(StageFont.sans(10))
            .foregroundStyle(StageInk.brown2)
            .padding(.horizontal, 9)
            .frame(height: 22)
            .background(RoundedRectangle(cornerRadius: 6).fill(Color.white))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(StageInk.border))
            .fixedSize()
            .beat(.pop, at: 0.42 + Double(i) * 0.06)
    }

    /// The "Launch plan" draft card.
    private var homeDoc: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 4)
                    .fill(LinearGradient(colors: [Color(hex: "#F6C2AE"), Color(hex: "#E8906F")], startPoint: .top, endPoint: .bottom))
                    .frame(width: 18, height: 22)
                Text("Launch plan").font(StageFont.sans(10.5, .bold))
                Spacer(minLength: 0)
                Text("DRAFT")
                    .font(StageFont.mono(9.5, .semibold))
                    .tracking(0.6)
                    .foregroundStyle(Color(hex: "#9A4426"))
                    .padding(.vertical, 3)
                    .padding(.horizontal, 7)
                    .background(RoundedRectangle(cornerRadius: 5).fill(Color(hex: "#FDF0EB")))
            }
            .padding(.horizontal, 12)
            .frame(height: 30)
            .overlay(alignment: .bottom) { Rectangle().fill(Color(hex: "#E4E4E7")).frame(height: 1) }

            VStack(alignment: .leading, spacing: 6) {
                docHeading("GOAL")
                docBar(0.92, at: 3.85)
                docBar(0.70, at: 3.92)
                docHeading("TIMELINE")
                docRow("Mon", 0.58, "#E8906F", at: 4.0)
                docRow("Wed", 0.80, "#2A1F16", at: 4.08)
                docRow("Fri", 0.44, "#F2B33D", at: 4.16)
            }
            .padding(.vertical, 9)
            .padding(.horizontal, 11)
            Spacer(minLength: 0)
        }
        .frame(width: 172, height: 176)
        .background(Color.white)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(StageInk.border))
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.white)
                .shadow(color: StageInk.shadow.opacity(0.16), radius: 20, y: 20)
        )
    }

    private func docHeading(_ title: String) -> some View {
        Text(title)
            .font(StageFont.sans(11, .bold))
            .tracking(0.66)
            .foregroundStyle(Color(hex: "#8A8A93"))
            .padding(.top, 4)
    }

    private func docBar(_ fraction: CGFloat, at delay: Double) -> some View {
        RoundedRectangle(cornerRadius: 3)
            .fill(Color(hex: "#EDEDEF"))
            .frame(width: 148 * fraction, height: 6)
            .beat(.grow, at: delay)
    }

    private func docRow(_ day: String, _ fraction: CGFloat, _ hex: String, at delay: Double) -> some View {
        HStack(spacing: 8) {
            Text(day)
                .font(StageFont.mono(10, .semibold))
                .foregroundStyle(Color(hex: "#5E5E66"))
                .frame(width: 30, alignment: .leading)
            RoundedRectangle(cornerRadius: 3)
                .fill(Color(hex: hex))
                .frame(width: 148 * fraction, height: 8)
                .beat(.grow, at: delay)
        }
    }
}

// MARK: - Bots

private struct StageTeammate {
    let name: String
    let role: String
    let image: String
    let color: Color
}

struct StageBotsScene: View {
    private let team: [StageTeammate] = [
        .init(name: "Gizzi", role: "Lead", image: StageAvatar.gizzi, color: Color(hex: "#D97757")),
        .init(name: "Drop", role: "Research", image: StageAvatar.drop, color: Color(hex: "#3B6FE0")),
        .init(name: "Pixel", role: "Inbox", image: StageAvatar.pixel, color: Color(hex: "#7B5CF0")),
        .init(name: "Grok", role: "Outreach", image: StageAvatar.grok, color: Color(hex: "#FF2E93")),
        .init(name: "Muse", role: "Recap", image: StageAvatar.muse, color: Color(hex: "#E58A1F")),
        .init(name: "ChatGPT dot", role: "Drafts", image: StageAvatar.nimbus, color: Color(hex: "#E9B91C")),
    ]

    private static let taupeMark = BlockType.markImage(size: 10, fill: UIColor(red: 140 / 255, green: 123 / 255, blue: 107 / 255, alpha: 1))

    var body: some View {
        StageScene.Layers {
            StageHalo(x: -40, y: 200, w: 320, rgb: (59, 111, 224), alpha: 0.22)
            StageHalo(x: 420, y: 180, w: 340, rgb: (123, 92, 240), alpha: 0.2)
            StageHalo(x: 300, y: 40, w: 240, rgb: (95, 212, 198), alpha: 0.28)
        } window: {
            StageAppWindow {
                StageRailItem(icon: .home, label: "Home", more: true)
                mainBotSlot
                StageRailHeading(title: "Projects")
                StageRailSwap(empty: "No projects yet.", item: "Launch week", at: 2.7)
                StageRailHeading(title: "Bots")
                VStack(alignment: .leading, spacing: 1) {
                    ForEach(team.indices, id: \.self) { i in
                        HStack(spacing: 6) {
                            StageImage(name: team[i].image, size: 16)
                            Text(team[i].name).lineLimit(1)
                            Spacer(minLength: 0)
                            Text(team[i].role).font(StageFont.mono(8.5)).foregroundStyle(StageInk.muted)
                        }
                        .font(StageFont.sans(10))
                        .padding(.horizontal, 6)
                        .frame(height: 20)
                        .beat(.slideIn, at: 1.7 + Double(i) * 0.08)
                    }
                }
                Spacer(minLength: 0)
                StageRailHeading(title: "Group chats").padding(.bottom, 2)
            } main: {
                VStack(alignment: .leading, spacing: 5) {
                    Text("What should the team work on?")
                        .font(StageFont.serif(21))
                        .beat(.pop, at: 0.2)
                    Text("Describe it once. \(Image(uiImage: Self.taupeMark)) splits it into threads, picks the bots, and keeps going when you close the app.")
                        .font(StageFont.sans(9.5))
                        .foregroundStyle(StageInk.brown)
                        .beat(.fade, at: 0.35)
                }
                .frame(width: mainWidth - 70, alignment: .leading)
                .place(40, 28)

                HStack(spacing: 5) {
                    Text("To:")
                    HStack(spacing: 4) {
                        BlockMark(fill: StageInk.taupe, size: 10)
                        Text("· new project")
                    }
                    .foregroundStyle(StageInk.brown)
                    .padding(.horizontal, 6)
                    .frame(height: 17)
                    .background(RoundedRectangle(cornerRadius: 5).fill(StageInk.chip))
                    StageIcon(name: .chev, size: 11)
                    Text("or use @ on a bot below to send it this directly").foregroundStyle(StageInk.muted)
                }
                .font(StageFont.sans(9))
                .foregroundStyle(StageInk.tan)
                .fixedSize()
                .beat(.pop, at: 0.42)
                .place(40, 96)

                teamComposer
                    .beat(.pop, at: 0.5)
                    .place(40, 146)

                StageImage(name: StageAvatar.gizzi, size: 44, shadow: true)
                    .beat(.pop, at: 0.55)
                    .place(236, 110)

                agentBar
                    .beat(.fade, at: 0.6)
                    .place(40, 220)

                ForEach(team.indices, id: \.self) { i in
                    StageMate(mate: team[i], index: i)
                        .place(46 + CGFloat(i) * 84 - 42, 262)
                }

                HStack(spacing: 8) {
                    StageCheckBadge(size: 18, radius: 6, icon: 10)
                    Text("6 threads started").fontWeight(.bold)
                    Spacer(minLength: 0)
                    Text("Launch week · new project").font(StageFont.sans(8.5)).foregroundStyle(Color(hex: "#C9BFB6"))
                }
                .font(StageFont.sans(10))
                .foregroundStyle(.white)
                .padding(.horizontal, 10)
                .frame(width: 220, height: 36)
                .background(
                    RoundedRectangle(cornerRadius: 11)
                        .fill(StageInk.text)
                        .shadow(color: StageInk.shadow.opacity(0.3), radius: 15, y: 14)
                )
                .beat(.rise, at: 3.35)
                .place(150, 398)
            }
        } front: {
            StageRing(x: 419, y: 190, size: 70, at: 1)
            StageBurst(x: 419, y: 190, at: 1.02, sparks: [
                .init(50, 16, "#D97757", -180), .init(47, 44, "#3B6FE0", -83), .init(28, 72, "#7B5CF0", 14), .init(-4, 58, "#141413", 111),
                .init(-35, 61, "#E58A1F", -152), .init(-43, 30, "#5FD4C6", -55), .init(-64, 10, "#D97757", 42), .init(-74, -23, "#3B6FE0", 139),
                .init(-43, -40, "#7B5CF0", -124), .init(-26, -66, "#141413", -27), .init(4, -52, "#E58A1F", 70), .init(32, -56, "#5FD4C6", 167),
                .init(64, -43, "#D97757", -96), .init(58, -9, "#3B6FE0", 1),
            ])
            StageBurst(x: 421, y: 425, at: 3.4, sparks: [
                .init(34, 11, "#1FA463", -180), .init(27, 36, "#4ADE80", -83), .init(1, 53, "#3B6FE0", 14), .init(-23, 33, "#D97757", 111),
                .init(-46, 16, "#1FA463", -152), .init(-34, -11, "#4ADE80", -55), .init(-27, -36, "#3B6FE0", 42), .init(-1, -53, "#D97757", 139),
                .init(23, -33, "#1FA463", -124), .init(46, -16, "#4ADE80", -27),
            ])
        }
    }

    /// "Set up your main bot" drop slot; Gizzi lands on its corner.
    private var mainBotSlot: some View {
        VStack(spacing: 2) {
            StageIcon(name: .crown, color: StageInk.tan)
                .frame(width: 22, height: 22)
                .background(RoundedRectangle(cornerRadius: 6).fill(StageInk.chip))
                .padding(.bottom, 2)
            Text("Set up your main bot").font(StageFont.sans(9.5, .semibold))
            Text("Or drag a bot here").font(StageFont.sans(8)).foregroundStyle(StageInk.tan)
        }
        .frame(maxWidth: .infinity)
        .frame(height: 84)
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color(hex: "#E2D8CE"), style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
        .overlay(alignment: .topTrailing) {
            StageImage(name: StageAvatar.gizzi, size: 30, shadow: true)
                .beat(.pop, at: 2.05)
                .offset(x: 6, y: -8)
        }
        .padding(.horizontal, 14)
        .padding(.top, 4)
        .padding(.bottom, 2)
    }

    /// The team composer with its teal focus glow.
    private var teamComposer: some View {
        ZStack(alignment: .topLeading) {
            StageTyping(
                placeholder: "e.g. Get the pricing page ready for Monday and check we still beat Lambda on H100s…",
                text: "Launch week: research, inbox replies, outreach and a recap",
                at: 0.75
            )
            .place(12, 11)
            HStack(spacing: 6) {
                StagePlusButton()
                HStack(spacing: 4) {
                    BlockMark(fill: StageInk.amber, size: 9)
                    Text("Output")
                    StageIcon(name: .chev, size: 8)
                }
                Spacer(minLength: 0)
                StageModelPicker(label: "Allternit Cloud · Kimi K3")
                Spacer(minLength: 0)
                StageVSep()
                StageIcon(name: .wave, color: StageInk.text)
                StageSend(at: 1.0)
            }
            .font(StageFont.sans(9.5))
            .foregroundStyle(StageInk.brown)
            .frame(width: 438 - 16)
            .place(8, 74 - 7 - 22)
        }
        .frame(width: 438, height: 74, alignment: .topLeading)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.white)
                .shadow(color: Color(red: 95 / 255, green: 212 / 255, blue: 198 / 255, opacity: 0.45), radius: 9)
                .shadow(color: StageInk.shadow.opacity(0.06), radius: 11, y: 8)
        )
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#7FDCD2"), lineWidth: 1))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(hex: "#A5E9E1"), lineWidth: 1).padding(-1))
    }

    /// "◆ Agent mode · Plugins" strip under the composer.
    private var agentBar: some View {
        HStack(spacing: 10) {
            HStack(spacing: 3) {
                Text("◆ Agent mode")
                StageIcon(name: .chev, size: 8).rotationEffect(.degrees(180))
            }
            Rectangle().fill(Color(hex: "#E2D8CE")).frame(width: 1, height: 10)
            HStack(spacing: 3) {
                ForEach(["#3B6FE0", "#1FA463", "#D9413A"], id: \.self) { hex in
                    RoundedRectangle(cornerRadius: 2).fill(Color(hex: hex)).frame(width: 6, height: 8)
                }
                Text("Plugins")
                StageIcon(name: .chev, size: 8).rotationEffect(.degrees(180))
            }
            Spacer(minLength: 0)
        }
        .font(StageFont.sans(9, .semibold))
        .foregroundStyle(StageInk.text)
        .padding(.horizontal, 12)
        .frame(width: 438, height: 26)
        .overlay(
            UnevenRoundedRectangle(bottomLeadingRadius: 12, bottomTrailingRadius: 12)
                .stroke(StageInk.border, lineWidth: 1)
                .mask(Rectangle().padding(.top, 1))
        )
    }
}

/// One teammate flying in from the send button, bobbing, then starting its thread.
private struct StageMate: View {
    let mate: StageTeammate
    let index: Int

    @Environment(\.stageTime) private var t
    @Environment(\.stageStill) private var still

    var body: some View {
        let delay = 1.05 + Double(index) * 0.09
        let p = StageMotion.progress(t, delay: delay, duration: 0.9)
        let e = StageCurve.fly(p)
        let opacity = p < 0.35 ? StageCurve.fly(p / 0.35) : 1
        let fx = 212 - Double(index) * 84
        let fy = -96.0
        // astx-bobk: 2.6 s, up 3 pt at the middle, from its own start time.
        let bobStart = 1.8 + Double(index) * 0.3
        var bob = 0.0
        if !still, t > bobStart {
            let phase = ((t - bobStart) / 2.6).truncatingRemainder(dividingBy: 1)
            bob = -3 * StageCurve.easeInOut(phase < 0.5 ? phase * 2 : (1 - phase) * 2)
        }
        return VStack(spacing: 1) {
            StageImage(name: mate.image, size: 58)
                .shadow(color: StageInk.shadow.opacity(0.18), radius: 5, y: 10)
                .offset(y: bob)
            HStack(spacing: 4) {
                Circle().fill(mate.color).frame(width: 6, height: 6)
                Text(mate.name).font(StageFont.sans(10.5, .bold)).lineLimit(1).fixedSize()
            }
            .padding(.top, 3)
            Text(mate.role).font(StageFont.mono(8.5)).tracking(0.17).foregroundStyle(StageInk.taupe)
            HStack(spacing: 4) {
                Circle().fill(StageInk.green).frame(width: 4, height: 4)
                Text("Thread started")
            }
            .font(StageFont.sans(8, .bold))
            .foregroundStyle(Color(hex: "#17724A"))
            .padding(.horizontal, 6)
            .frame(height: 16)
            .background(Capsule().fill(Color(hex: "#E8F5EE")))
            .fixedSize()
            .padding(.top, 4)
            .beat(.pop, at: 2.75 + Double(index) * 0.1)
        }
        .frame(width: 84)
        .scaleEffect(0.25 + 0.75 * e)
        .offset(x: fx * (1 - e), y: fy * (1 - e))
        .opacity(StageMotion.clamp(opacity))
    }
}

// MARK: - Code

struct StageCodeScene: View {
    var body: some View {
        StageScene.Layers {
            StageHalo(x: 150, y: 150, w: 420, h: 320, rgb: (74, 222, 128), alpha: 0.24)
            StageHalo(x: 470, y: -60, w: 280, rgb: (217, 119, 87), alpha: 0.22)
        } window: {
            StageAppWindow {
                StageRailItem(icon: .plus, label: "New Thread", isNew: true)
                StageRailItem(icon: .folder, label: "Projects")
                StageRailItem(icon: .doc, label: "Artifacts")
                StageRailItem(icon: .clock, label: "Automation Tasks")
                StageRailItem(icon: .sliders, label: "More")
                StageRailLabel(title: "Recents")
                StageRailSwap(empty: "No code recents match your filters", item: "Add a pricing page", at: 2.3)
            } main: {
                StageAway(at: 2.25) {
                    HStack(spacing: 6) {
                        StageImage(name: StageAvatar.gizzi, size: 15)
                        PixelWord(text: "GIZZI CODE", fill: StageInk.text, height: 13)
                        Text("in A://").font(StageFont.mono(8.5)).foregroundStyle(StageInk.taupe).padding(.leading, 4)
                        Spacer(minLength: 0)
                        HStack(spacing: 4) {
                            StageIcon(name: .spark, size: 11)
                            Text("What\u{2019}s new")
                        }
                        .font(StageFont.sans(9, .semibold))
                        .padding(.horizontal, 8)
                        .frame(height: 20)
                        .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(StageInk.border))
                        .overlay(alignment: .topTrailing) {
                            Circle().fill(StageInk.coral).frame(width: 5, height: 5).offset(x: 2, y: -2)
                        }
                    }
                    .frame(width: 450)
                    .beat(.pop, at: 0.2)
                    .place(34, 34)

                    Text("Ready to ship something?")
                        .font(StageFont.serif(22, .bold))
                        .fixedSize()
                        .beat(.pop, at: 0.32)
                        .place(34, 62)
                    Text("Run a command or describe a task to start coding.")
                        .font(StageFont.sans(10))
                        .foregroundStyle(StageInk.brown)
                        .fixedSize()
                        .beat(.fade, at: 0.42)
                        .place(34, 96)
                    HStack(spacing: 5) {
                        StageIcon(name: .chart)
                        Text("Show usage")
                    }
                    .font(StageFont.sans(9.5, .semibold))
                    .padding(.horizontal, 9)
                    .frame(height: 22)
                    .background(RoundedRectangle(cornerRadius: 7).fill(Color(hex: "#F7F4F1")))
                    .beat(.pop, at: 0.5)
                    .place(34, 122)
                }

                session.place(34, 24)

                HStack(spacing: 6) {
                    StageIcon(name: .check, color: .white)
                    Text("Tests passed")
                }
                .font(StageFont.sans(10.5, .bold))
                .foregroundStyle(.white)
                .padding(.horizontal, 11)
                .frame(height: 26)
                .background(
                    Capsule().fill(StageInk.green)
                        .shadow(color: StageInk.green.opacity(0.35), radius: 11, y: 10)
                )
                .beat(.pop, at: 3.6)
                .place(34, 226)

                sessionComposer
                    .beat(.pop, at: 0.55)
                    .place(20, 326)

                StageImage(name: StageAvatar.gizzi, size: 40, shadow: true)
                    .beat(.pop, at: 0.7)
                    .place(474, 388)
            }
        } front: {
            StageRing(x: 595, y: 429, size: 48, at: 2.2)
            StageBurst(x: 595, y: 429, at: 2.22, sparks: [
                .init(29, 9, "#D97757", -180), .init(17, 33, "#F29D38", -83), .init(-13, 42, "#2A1F16", 14), .init(-30, 16, "#E8906F", 111),
                .init(-39, -12, "#D97757", -152), .init(-14, -27, "#F29D38", -55), .init(11, -35, "#2A1F16", 42), .init(39, -21, "#E8906F", 139),
            ])
            StageBurst(x: 250, y: 252, at: 3.62, sparks: [
                .init(39, 12, "#22A06B", -180), .init(34, 37, "#4ADE80", -83), .init(13, 58, "#1FA463", 14), .init(-13, 43, "#D97757", 111),
                .init(-40, 37, "#F2B33D", -152), .init(-39, 9, "#22A06B", -55), .init(-48, -15, "#4ADE80", 42), .init(-40, -44, "#1FA463", 139),
                .init(-10, -44, "#D97757", -124), .init(16, -52, "#F2B33D", -27), .init(30, -28, "#22A06B", 70), .init(49, -11, "#4ADE80", 167),
            ])
        }
    }

    /// The gizzi code session that replaces the empty state.
    private var session: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 7) {
                Text("❯").foregroundStyle(StageInk.coral)
                Text("add a pricing page and make the tests pass")
                Spacer(minLength: 0)
            }
            .font(StageFont.mono(10))
            .padding(.horizontal, 10)
            .frame(height: 26)
            .background(RoundedRectangle(cornerRadius: 8).fill(Color(hex: "#F7F4F1")))
            .padding(.bottom, 8)
            .beat(.rise, at: 2.35)

            toolRow("Read", " src/pages/pricing.tsx", at: 2.6)
            toolRow("Edit", " src/pages/pricing.tsx", trailing: Text("+18 −3").foregroundStyle(StageInk.taupe), at: 2.8)

            VStack(alignment: .leading, spacing: 0) {
                Text("src/pages/pricing.tsx")
                    .foregroundStyle(StageInk.brown)
                    .padding(.horizontal, 9)
                    .frame(maxWidth: .infinity, minHeight: 22, maxHeight: 22, alignment: .leading)
                    .background(Color(hex: "#FAF8F6"))
                    .overlay(alignment: .bottom) { Rectangle().fill(StageInk.hairline).frame(height: 1) }
                diffLine("−", "export const plans = []", kind: .removed, at: 3.05)
                diffLine("+", "export const plans = [starter, team, scale]", kind: .added, at: 3.12)
                diffLine("+", "<PlanGrid plans={plans} />", kind: .added, at: 3.19)
                diffLine(" ", "<Footer />", kind: .context, at: 3.26)
            }
            .font(StageFont.mono(9.5))
            .clipShape(RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(StageInk.border))
            .padding(.top, 4)
            .padding(.bottom, 6)
            .beat(.pop, at: 2.95)

            toolRow("Ran", " pnpm test ", trailing: Text("24 passed").fontWeight(.bold).foregroundStyle(Color(hex: "#17724A")), at: 3.4)
        }
        .frame(width: 452, alignment: .topLeading)
    }

    private func toolRow(_ verb: String, _ rest: String, trailing: Text? = nil, at delay: Double) -> some View {
        HStack(spacing: 7) {
            StageOrb(state: .working, size: 11)
            Text("\(Text(verb).foregroundStyle(StageInk.text))\(rest)")
            if let trailing {
                Spacer(minLength: 0)
                trailing
            }
        }
        .font(StageFont.mono(9.5))
        .foregroundStyle(StageInk.brown)
        .frame(maxWidth: .infinity, minHeight: 20, maxHeight: 20, alignment: .leading)
        .beat(.slideIn, at: delay)
    }

    private enum DiffKind { case added, removed, context }

    private func diffLine(_ sign: String, _ code: String, kind: DiffKind, at delay: Double) -> some View {
        let (fg, bg): (Color, Color) = switch kind {
        case .added: (Color(hex: "#17663F"), Color(hex: "#E9F7EE"))
        case .removed: (Color(hex: "#9B2C2C"), Color(hex: "#FDECEC"))
        case .context: (StageInk.brown2, .clear)
        }
        return HStack(spacing: 8) {
            Text(sign).frame(width: 8, alignment: .leading).foregroundStyle(kind == .context ? Color(hex: "#A08F80") : fg)
            Text(code).foregroundStyle(fg)
        }
        .padding(.horizontal, 9)
        .frame(maxWidth: .infinity, minHeight: 16, maxHeight: 16, alignment: .leading)
        .background(bg)
        .beat(.slideIn, at: delay)
    }

    /// The session composer: device, folder, worktree, prompt, Console.
    private var sessionComposer: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 14) {
                HStack(spacing: 4) {
                    StageIcon(name: .monitor, size: 9)
                    Text("This Device")
                    StageIcon(name: .chev, size: 9)
                }
                HStack(spacing: 4) {
                    StageIcon(name: .folder, size: 9)
                    ZStack(alignment: .leading) {
                        Text("Select folder").fontWeight(.medium).beat(.fadeOut, at: 0.95)
                        Text("launch-site").fontWeight(.bold).beat(.fade, at: 1.0)
                    }
                    .frame(width: 64, height: 12, alignment: .leading)
                    StageIcon(name: .chev, size: 9)
                }
                HStack(spacing: 4) {
                    RoundedRectangle(cornerRadius: 2).strokeBorder(StageInk.taupe, lineWidth: 1.3).frame(width: 9, height: 9)
                    Text("Worktree")
                }
                Spacer(minLength: 0)
            }
            .font(StageFont.sans(9.5))
            .lineLimit(1)
            .padding(.horizontal, 10)
            .frame(height: 28)
            .overlay(alignment: .bottom) { Rectangle().fill(StageInk.hairline).frame(height: 1) }

            ZStack(alignment: .topLeading) {
                Text("❯").font(StageFont.sans(10)).foregroundStyle(StageInk.coral).place(10, 6)
                StageTyping(
                    placeholder: "Choose a workspace folder to unlock the session…",
                    text: "add a pricing page and make the tests pass",
                    at: 1.2,
                    font: StageFont.mono(9.5)
                )
                .place(22, 7)
            }
            .frame(maxWidth: .infinity, minHeight: 26, maxHeight: 26, alignment: .topLeading)

            Spacer(minLength: 0)
            HStack(spacing: 6) {
                StagePlusButton()
                StageIcon(name: .wave, color: StageInk.text)
                Spacer(minLength: 0)
                StageModelPicker(label: "Allternit Cloud · Claude…", mono: true)
            }
            .padding(.leading, 8)
            .padding(.trailing, 10)
            .padding(.bottom, 4)
            HStack(spacing: 6) {
                HStack(spacing: 4) {
                    StageIcon(name: .hand, size: 9)
                    Text("Manual")
                    StageIcon(name: .chev, size: 9)
                }
                .padding(.horizontal, 8)
                .frame(height: 20)
                .overlay(Capsule().strokeBorder(StageInk.border))
                Spacer(minLength: 0)
                HStack(spacing: 4) {
                    StageIcon(name: .terminal, size: 9)
                    Text("Console")
                }
                .padding(.horizontal, 8)
                .frame(height: 20)
                .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(StageInk.border))
                .beat(.press, at: 2.2)
            }
            .font(StageFont.sans(9.5, .semibold))
            .padding(.horizontal, 8)
            .padding(.bottom, 6)
        }
        .foregroundStyle(StageInk.text)
        .frame(width: 452, height: 108)
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.white)
                .shadow(color: StageInk.shadow.opacity(0.06), radius: 9, y: 6)
        )
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(StageInk.border))
    }
}

// MARK: - ACI (Allternit Explorer)

struct StageAciScene: View {
    private struct Day {
        let name: String
        let busy: [Int]
        var slot: Int? = nil
    }

    private let week: [Day] = [
        Day(name: "Mon", busy: [1]),
        Day(name: "Tue", busy: [3]),
        Day(name: "Wed", busy: [0]),
        Day(name: "Thu", busy: [0], slot: 3),
        Day(name: "Fri", busy: [2]),
    ]

    var body: some View {
        StageScene.Layers {
            StageHalo(x: -60, y: -40, w: 360, rgb: (59, 130, 246), alpha: 0.2)
            StageHalo(x: 380, y: 170, w: 320, rgb: (217, 119, 87), alpha: 0.26)
        } window: {
            StageAppWindow {
                StageRailItem(icon: .plus, label: "New Session", isNew: true)
                StageRailItem(icon: .panel, label: "MCP Apps")
                StageRailItem(icon: .puzzle, label: "ACI Extensions")
                StageRailItem(icon: .monitor, label: "Computers")
                StageRailItem(icon: .target, label: "Teach")
                StageRailItem(icon: .sliders, label: "More")
                StageRailLabel(title: "Recents")
            } main: {
                // Dot grid behind the Explorer home.
                Canvas { context, size in
                    let dot = Color(hex: "#DDD5CD")
                    var y: CGFloat = 7
                    while y < size.height {
                        var x: CGFloat = 7
                        while x < size.width {
                            context.fill(Path(ellipseIn: CGRect(x: x - 1, y: y - 1, width: 2, height: 2)), with: .color(dot))
                            x += 14
                        }
                        y += 14
                    }
                }
                .opacity(0.8)
                .frame(width: mainWidth, height: StageLayout.mainHeight)

                StageAway(at: 2.25) {
                    BlockMark(fill: StageInk.amber, size: 44)
                        .frame(width: mainWidth)
                        .beat(.pop, at: 0.15)
                        .place(0, 34)
                    Text("Allternit Explorer")
                        .font(StageFont.sans(20, .semibold))
                        .tracking(-0.4)
                        .frame(width: mainWidth)
                        .beat(.pop, at: 0.28)
                        .place(0, 86)
                    HStack(spacing: 10) {
                        Rectangle().fill(Color(hex: "#E2D8CE")).frame(width: 22, height: 1)
                        Text("BROWSE. AUTOMATE. BUILD.").tracking(1.5)
                        Rectangle().fill(Color(hex: "#E2D8CE")).frame(width: 22, height: 1)
                    }
                    .font(StageFont.sans(8.5))
                    .foregroundStyle(StageInk.brown)
                    .frame(width: mainWidth)
                    .beat(.fade, at: 0.38)
                    .place(0, 116)

                    urlField
                        .beat(.pop, at: 0.48)
                        .place(96, 142)

                    agentPill
                        .frame(width: mainWidth)
                        .beat(.pop, at: 0.58)
                        .place(0, 186)

                    Text("Customize")
                        .font(StageFont.sans(8.5))
                        .underline()
                        .fixedSize()
                        .beat(.fade, at: 0.7)
                        .place(384, 222)

                    HStack(spacing: 8) {
                        tile(at: 0.72, label: "Google") {
                            Text("G").font(StageFont.sans(11, .heavy)).foregroundStyle(Color(hex: "#4285F4"))
                                .frame(width: 24, height: 24).background(Circle().fill(.white))
                        }
                        tile(at: 0.78, label: "GitHub") {
                            Text("●").font(StageFont.sans(15, .heavy)).foregroundStyle(Color(hex: "#111111"))
                                .frame(width: 24, height: 24).background(Circle().fill(.white))
                        }
                        tile(at: 0.84, label: "Hacker News") {
                            Text("Y").font(StageFont.sans(11, .heavy)).foregroundStyle(.white)
                                .frame(width: 24, height: 24).background(RoundedRectangle(cornerRadius: 4).fill(Color(hex: "#FF6600")))
                        }
                        tile(at: 0.9, label: "Add", add: true) {
                            Text("+").font(StageFont.sans(16)).foregroundStyle(StageInk.taupe)
                        }
                    }
                    .frame(width: 330)
                    .place(96, 238)
                }

                browser
                    .beat(.rise, at: 2.3)
                    .place(22, 18)

                StageCursor()

                HStack(spacing: 9) {
                    StageCheckBadge(size: 24, radius: 7, icon: 12)
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Booked · Team sync").font(StageFont.sans(11, .bold)).foregroundStyle(StageInk.text)
                        Text("Thu 2:00 PM · invites sent to the team")
                    }
                    Spacer(minLength: 0)
                }
                .font(StageFont.sans(9))
                .foregroundStyle(StageInk.brown)
                .padding(.horizontal, 12)
                .frame(width: 250, height: 48)
                .background(
                    RoundedRectangle(cornerRadius: 12)
                        .fill(Color.white)
                        .shadow(color: StageInk.shadow.opacity(0.18), radius: 19, y: 18)
                )
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(StageInk.border))
                .beat(.rise, at: 3.3)
                .place(150, 382)
            }
        } front: {
            StageRing(x: 422, y: 210, size: 60, at: 1.88)
            StageBurst(x: 422, y: 210, at: 1.9, sparks: [
                .init(32, 10, "#22A06B", -180), .init(19, 36, "#4ADE80", -83), .init(-14, 46, "#D97757", 14), .init(-33, 17, "#22A06B", 111),
                .init(-43, -13, "#4ADE80", -152), .init(-15, -29, "#D97757", -55), .init(12, -39, "#22A06B", 42), .init(43, -23, "#4ADE80", 139),
            ])
            StageRing(x: 512, y: 246, size: 44, at: 2.95)
            StageBurst(x: 436, y: 419, at: 3.35, sparks: [
                .init(40, 12, "#1FA463", -180), .init(35, 38, "#3B6FE0", -83), .init(14, 60, "#7B5CF0", 14), .init(-14, 45, "#F29D38", 111),
                .init(-42, 39, "#D97757", -152), .init(-41, 9, "#1FA463", -55), .init(-49, -15, "#3B6FE0", 42), .init(-42, -45, "#7B5CF0", 139),
                .init(-10, -46, "#F29D38", -124), .init(17, -54, "#D97757", -27), .init(31, -29, "#1FA463", 70), .init(51, -11, "#3B6FE0", 167),
            ])
        }
    }

    private var urlField: some View {
        ZStack(alignment: .topLeading) {
            StageTyping(placeholder: "Search the web or enter a URL...", text: "calendar.google.com", at: 0.9, font: StageFont.sans(10.5))
                .place(12, 9)
            StageIcon(name: .search, color: .white)
                .frame(width: 24, height: 24)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color(hex: "#B08D6E")))
                .beat(.press, at: 1.75)
                .place(297, 4)
        }
        .frame(width: 326, height: 32, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 9).fill(Color(hex: "#F4F1ED")))
        .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Color(hex: "#ECE6DF")))
    }

    /// "Allternit Computer Agent: Off" switching to On.
    private var agentPill: some View {
        ZStack {
            HStack(spacing: 5) {
                BlockMark(fill: StageInk.brown, size: 9)
                Text("Allternit Computer Agent: Off")
            }
            .foregroundStyle(StageInk.brown2)
            .beat(.fadeOut, at: 1.85)
            HStack(spacing: 5) {
                Circle().fill(StageInk.green).frame(width: 6, height: 6)
                Text("Allternit Computer Agent: On").fontWeight(.bold)
            }
            .foregroundStyle(StageInk.greenText)
            .frame(width: 178, height: 22)
            .background(
                Capsule().fill(StageInk.greenSoft)
                    .background(Capsule().stroke(StageInk.green.opacity(0.14), lineWidth: 6))
            )
            .beat(.fade, at: 1.9)
        }
        .font(StageFont.sans(9))
        .lineLimit(1)
        .frame(width: 178, height: 22)
        .background(Capsule().fill(Color.white))
        .overlay(Capsule().strokeBorder(StageInk.border))
    }

    private func tile<Icon: View>(at delay: Double, label: String, add: Bool = false, @ViewBuilder icon: () -> Icon) -> some View {
        VStack(spacing: 5) {
            icon()
            Text(label).lineLimit(1)
        }
        .font(StageFont.sans(9))
        .foregroundStyle(StageInk.brown2)
        .frame(maxWidth: .infinity)
        .frame(height: 66)
        .background(RoundedRectangle(cornerRadius: 9).fill(add ? Color.white : Color(hex: "#F4F1ED")))
        .overlay(
            RoundedRectangle(cornerRadius: 9)
                .strokeBorder(Color(hex: "#ECE6DF"), style: StrokeStyle(lineWidth: 1, dash: add ? [3, 2] : []))
        )
        .beat(.pop, at: delay)
    }

    /// The Explorer browser on Google Calendar, where Gizzi books the meeting.
    private var browser: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 5) {
                ForEach(0..<3, id: \.self) { _ in
                    Circle().fill(Color(hex: "#E2D8CE")).frame(width: 8, height: 8)
                }
                HStack(spacing: 5) {
                    StageIcon(name: .search, size: 9)
                    Text("calendar.google.com").fontWeight(.bold)
                    Spacer(minLength: 0)
                }
                .font(StageFont.mono(9))
                .foregroundStyle(StageInk.brown)
                .padding(.horizontal, 8)
                .frame(height: 18)
                .background(RoundedRectangle(cornerRadius: 6).fill(Color(hex: "#F1ECE6")))
                .padding(.leading, 8)
                HStack(spacing: 4) {
                    Circle().fill(StageInk.green).frame(width: 5, height: 5)
                    Text("Agent on")
                }
                .font(StageFont.sans(8.5, .bold))
                .foregroundStyle(StageInk.greenText)
                .padding(.horizontal, 7)
                .frame(height: 18)
                .background(Capsule().fill(StageInk.greenSoft))
            }
            .padding(.horizontal, 10)
            .frame(height: 30)
            .background(Color(hex: "#FAF8F6"))
            .overlay(alignment: .bottom) { Rectangle().fill(StageInk.hairline).frame(height: 1) }

            VStack(alignment: .leading, spacing: 8) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("October").font(StageFont.sans(12))
                    Text("Week 42").font(StageFont.sans(9)).foregroundStyle(StageInk.taupe)
                }
                HStack(alignment: .top, spacing: 6) {
                    ForEach(week.indices, id: \.self) { i in
                        VStack(spacing: 5) {
                            Text(week[i].name)
                                .font(StageFont.mono(8.5, .semibold))
                                .foregroundStyle(StageInk.taupe)
                            ForEach(0..<5, id: \.self) { row in
                                calendarCell(week[i], row: row)
                            }
                        }
                        .frame(maxWidth: .infinity)
                    }
                }
            }
            .padding(.vertical, 10)
            .padding(.horizontal, 12)
            Spacer(minLength: 0)
        }
        .foregroundStyle(StageInk.text)
        .frame(width: 476, height: 300)
        .background(Color.white)
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(StageInk.border))
        .background(
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.white)
                .shadow(color: StageInk.shadow.opacity(0.18), radius: 25, y: 22)
        )
    }

    @ViewBuilder
    private func calendarCell(_ day: Day, row: Int) -> some View {
        let cell = RoundedRectangle(cornerRadius: 6)
        if row == day.slot {
            cell.fill(Color(hex: "#F7F4F1"))
                .frame(height: 34)
                .overlay(
                    Text("Team sync · 2:00")
                        .font(StageFont.sans(8.5, .bold))
                        .foregroundStyle(.white)
                        .lineLimit(1)
                        .minimumScaleFactor(0.8)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .background(
                            cell.fill(StageInk.coral)
                                .shadow(color: StageInk.coral.opacity(0.4), radius: 7, y: 6)
                        )
                        .beat(.pop, at: 3.0)
                )
        } else if day.busy.contains(row) {
            cell.fill(Color(hex: "#E8EEF9"))
                .frame(height: 34)
                .overlay(alignment: .leading) {
                    Rectangle().fill(Color(hex: "#3B6FE0")).frame(width: 3)
                }
                .clipShape(cell)
        } else {
            cell.fill(Color(hex: "#F7F4F1")).frame(height: 34)
        }
    }
}

/// Gizzi's cursor gliding to the free Thursday slot and clicking it.
private struct StageCursor: View {
    @Environment(\.stageTime) private var t

    var body: some View {
        let p = StageMotion.progress(t, delay: 2.55, duration: 1.1)
        let e = StageCurve.cursor(p)
        let opacity = p <= 0 ? 0 : (p < 0.15 ? StageCurve.cursor(p / 0.15) : 1)
        let press = StageBeatKeyframes.segments(
            StageMotion.progress(t, delay: 2.95, duration: 0.22),
            [(0, 1), (0.5, 0.82), (1, 1)]
        )
        HStack(alignment: .top, spacing: 3) {
            ZStack(alignment: .topLeading) {
                Path { path in
                    path.move(to: CGPoint(x: 2, y: 2))
                    path.addLine(to: CGPoint(x: 19, y: 11))
                    path.addLine(to: CGPoint(x: 11.5, y: 13.2))
                    path.addLine(to: CGPoint(x: 8, y: 21))
                    path.closeSubpath()
                }
                .fill(StageInk.text)
                .overlay(
                    Path { path in
                        path.move(to: CGPoint(x: 2, y: 2))
                        path.addLine(to: CGPoint(x: 19, y: 11))
                        path.addLine(to: CGPoint(x: 11.5, y: 13.2))
                        path.addLine(to: CGPoint(x: 8, y: 21))
                        path.closeSubpath()
                    }
                    .stroke(Color.white, style: StrokeStyle(lineWidth: 1.6, lineJoin: .round))
                )
                Circle().fill(StageInk.coral).frame(width: 6, height: 6)
            }
            .frame(width: 22, height: 26)
            .scaleEffect(18 / 22, anchor: .topLeading)
            .frame(width: 18, height: 22, alignment: .topLeading)
            Text("Gizzi")
                .font(StageFont.sans(8.5, .bold))
                .foregroundStyle(.white)
                .padding(.horizontal, 6)
                .frame(height: 16)
                .background(RoundedRectangle(cornerRadius: 5).fill(StageInk.coral))
                .padding(.top, 14)
        }
        .fixedSize()
        .scaleEffect(press, anchor: .center)
        .opacity(opacity)
        .place(StageMotion.lerp(250, 345, e), StageMotion.lerp(150, 228, e))
    }
}
