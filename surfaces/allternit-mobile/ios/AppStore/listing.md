# App Store listing — Allternit for iPhone (com.allternit.mobile)

Draft for App Store Connect. Register 1 voice: plain, no hype, no guarantees. Eoj approves before it is entered.

## App information

| Field | Value |
|---|---|
| Name (30) | Allternit |
| Subtitle (30) | Your bots, chats and computer |
| Bundle ID | com.allternit.mobile |
| Primary category | Productivity |
| Secondary category | Business |
| Privacy policy URL | https://allternit.com/privacy |
| Support URL | https://allternit.com/support |
| Marketing URL | https://allternit.com |
| Copyright | 2026 Allternit LLC |
| Age rating | Answer the questionnaire "None" for every content row. The in-app browser (ACI, links) is unrestricted web access, which Apple puts in its highest age band. |

## Promotional text (170)

Talk to your bots, answer what they ask, and check on the work your computer is doing, from your iPhone.

## Description (4000)

Allternit for iPhone is the app version of m.allternit.com. It uses the same account as ai.allternit.com and Allternit Desktop, so your chats, bots, projects and settings are the same everywhere.

Chat
Start a chat, attach files and see answers stream in. Artifacts made in a chat are saved to your library.

Bots
Your bots are listed with their status. Open one for its threads, tasks, routines, memory and channels. Group chats let several bots work on one request. Bots greet you, ask before anything that matters, and you approve from your phone.

Your computer
Code, automations and a bot's computer run on a computer paired to your account: Allternit Desktop on your Mac or PC, or a cloud computer. The app reaches it through Allternit's cloud, so your phone doesn't need to be on the same network. You can watch a computer's screen and take control.

Needs you
The Inbox collects what your bots are waiting on: approvals, held messages, missed calls and failed runs. Decisions shows the questions agents ask, with the answers on the card. Notifications tell you when a bot needs you, and tapping one opens the right place.

Account
Sign in with Apple or your Allternit account. You can export your data and delete your account in Settings.

Allternit needs an Allternit account. Code and automation features need a computer paired to that account.

## Keywords (100)

ai,agents,bots,assistant,automation,chat,remote desktop,approvals,workflow,code

## What's New (1.0)

First release of Allternit for iPhone.

## App Review information

- **Sign-in:** demo account — Eoj to create (email + password, no 2FA). Fill here before submit:
  - User name: `<demo email>`
  - Password: `<demo password>`
- **Demo computer:** a computer paired to the demo account that stays online during review (Eoj: cloud computer on standby). Without it, Code and Automation show "Connect a computer", which is the correct state but hides those features from review.
- **Notes for the reviewer:**

> Allternit is a workspace for AI agents ("bots"). Sign in with the demo account above. Home is a chat. The drawer (☰, top left) has Inbox, Projects, Artifacts, Automation, Decisions and Settings; the dial next to it switches between Home, Bots, Code and ACI. The demo account has a cloud computer attached, so Code and Automation work.
>
> Account deletion: Settings → Account → Delete account (type DELETE to confirm).
>
> Sign in with Apple is offered on the sign-in screen next to email sign-in, because the account also supports Google sign-in.
>
> Purchases: the app has no in-app purchases. On the United States storefront, Settings → Billing links to our website to manage a plan (guideline 3.1.1(a) US link entitlement); outside the US the app shows plan and usage only, with no link or call to action.
>
> Background audio: bots can call the person in the app (voice over LiveKit). The audio background mode keeps an answered call going when the person leaves the app. It is not used for anything else.
>
> Screen sharing: Settings → Computers → Let your bots use this iPhone pairs the app with the person's own Allternit Desktop. A broadcast starts only from the system picker (RPSystemBroadcastPickerView, limited to the com.allternit.mobile.screen extension); a bot can only ask the app to show the picker. While it runs, the app shows "Your bot can see this screen" with Stop, and iOS shows its recording indicator. While a bot uses the app's browser, it shows "Bot is using the browser" with Stop.
>
> Encryption: the app uses standard HTTPS and a WireGuard-based network connection to the person's own computers.

## Export compliance

The app uses encryption: HTTPS, and WireGuard (via the Allternit mesh) for the user's own data. Eoj's answer (2026-10-31): yes, it uses encryption, and it is exempt (mass-market, Category 5 Part 2, note 3), so no export documents are needed. `ITSAppUsesNonExemptEncryption` is set to NO in Info.plist, so App Store Connect no longer asks on each build. A year-end self-classification report to the US Bureau of Industry and Security may still be required for the mass-market exemption; check before the first public release.

## App Privacy (nutrition label)

Matches App/PrivacyInfo.xcprivacy. Screen sharing rows and why: `surfaces/allternit-mobile/docs/app-store-privacy.md`. Data linked to the user, used for App Functionality, not used for tracking:

- Contact info: name, email address
- User content: other user content (chats, files), photos (when attached), audio (voice input)
- Identifiers: device ID (push token)
- Usage data: product interaction

No tracking. No third-party advertising.

## Before submit (checklist)

- [ ] Privacy policy at allternit.com/privacy covers the iPhone app, push tokens and account deletion (the page renders client-side; read it in a browser).
- [ ] Clerk: Sign in with Apple on; users can delete their own account.
- [ ] Server decision on what deletion does (today: account closed, data kept until privacy@ request; paid plans not cancelled) — the delete screen says this plainly.
- [ ] Screenshots: 6.9" (1320×2868) and 6.5" (1284×2778), 3–5 each: Home chat, Bots, Inbox/Decisions, computer screen, Settings.
- [ ] Demo account + demo computer online.
- [ ] Apple Developer: if the App Store team is not the personal team MQ85968PQP, delete the identifier `com.allternit.mobile.screen` registered on MQ85968PQP during a device test (identifiers are unique across Apple), then register the app IDs on the paid team.
