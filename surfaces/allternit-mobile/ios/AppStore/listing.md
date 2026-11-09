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

- **Contact:** support@allternit.com (forwards to allternitpbc@gmail.com through Cloudflare Email Routing; checked live 2026-11-04).
- **Sign-in:** demo account `appreview@allternit.com`; the password is in `~/.appstoreconnect/demo-account.txt` and in App Store Connect. Never write it here.
- **Demo computer:** a computer paired to the demo account that stays online during review (Eoj: cloud computer on standby). Without it, Code and Automation show "Connect a computer", which is the correct state but hides those features from review.
- **Demo brain and credit (2026-11-09):** Allternit Cloud only, default `allternit/glm-5.3`; Codex is off (it was a personal ChatGPT Plus login with its own quota). $5 Cloud credit. The four bot routines are paused during review so they don't spend it; resume them after approval.
- **Review video:** `reports/app-review-build27.mp4` in the workspace, attached in App Review Information. Recorded over USB on iPhone (4) with build 27 and the demo account.
- **Notes for the reviewer:**

> Allternit is a workspace for AI agents ("bots"). Sign in with the demo account above. Home is a chat. The drawer (☰, top left) has Inbox, Projects, Artifacts, Automation, Decisions and Settings; the dial next to it switches between Home, Bots, Code and ACI. The demo account has a cloud computer attached, so Code and Automation work.
>
> First chat: a new chat asks you to pick a brain before the first message. Choose **GLM 5.3 (Allternit Cloud)**; it is paid for on the demo account. The five demo bots already use GLM 5.3. The attached video walks through sign-in, chat, a bot, MCP Apps, Billing, account deletion and screen sharing.
>
> Account deletion: Settings → Account → Delete account (type DELETE to confirm).
>
> Sign in with Apple is offered on the sign-in screen next to email sign-in, because the account also supports Google sign-in.
>
> Purchases: the app has no in-app purchases. On the United States storefront, Settings → Billing links to our website to manage a plan (guideline 3.1.1(a) US link entitlement); outside the US the app shows plan and usage only, with no link or call to action.
>
> Background audio: bots can call the person in the app (voice over LiveKit). The audio background mode keeps an answered call going when the person leaves the app. It is not used for anything else.
>
> Remote computers: Code and Computers connect to a computer on the person's own account: Allternit Desktop on their Mac or PC, or a cloud computer they added. It shows that whole computer's desktop; it does not stream a single app or game.
>
> Screen sharing: Settings → Computers → Let your bots use this iPhone pairs the app with the person's own Allternit Desktop. A broadcast starts only from the system picker (RPSystemBroadcastPickerView, limited to the com.allternit.mobile.screen extension); a bot can only ask the app to show the picker. While it runs, the app shows "Your bot can see this screen" with Stop, and iOS shows its recording indicator. While a bot uses the app's browser, it shows "Bot is using the browser" with Stop.
>
> What a bot can do on this iPhone: see the screen while the person broadcasts, and use the app's own browser. It can't tap, type in or open other apps; the app uses only public APIs (ReplayKit, WebKit). The demo account has its own cloud computer, so pairing works from Settings → Computers → Let your bots use this iPhone (the computer approves it). The attached video, recorded on an iPhone 15 with this build and the demo account, shows the whole flow: pairing, the system picker, the banner with Stop, the bot using the app's browser, and both Stop buttons.
>
> Encryption: the app uses HTTPS, a WireGuard connection to the person's own computers, and OpenSSL with libssh2 inside libgit2 for git over HTTPS and SSH. All are standard algorithms in a mass-market app (ECCN 5D992).

## Export compliance

The app ships its own encryption code besides Apple's: WireGuard (wireguard-go inside Mesh.xcframework, the Allternit mesh to the person's own computers) and OpenSSL with libssh2 (inside libgit2.xcframework, for git over HTTPS and SSH). Under US law that is mass-market encryption under the Cryptography Note (Note 3 to Category 5, Part 2), ECCN 5D992, so no export license is needed.

App Store Connect asks a narrower question: does the app implement encryption algorithms "instead of, or in addition to" Apple's? Because of WireGuard and OpenSSL the honest answer is "Standard encryption algorithms", not "None". Builds 6–20 set `ITSAppUsesNonExemptEncryption = NO` (platform #2013), which told Apple "None". Build 21 briefly set YES, which fails upload without a declaration code; it is back to NO (see Before submit). Distributing in France needs a French encryption declaration (ANSSI), so France is left out of the storefronts for now.

No year-end self-classification report is needed. 15 CFR 740.17(e)(3) requires it only for mass-market encryption components, "executable software" taken from hardware components, and non-mass-market items (5A002, 5B002, 5D002). Its note excludes "complete binary images of the software running on an end item", which is what an App Store app is. Checked against the eCFR text on 2026-11-01. If the app ever ships its own crypto library to other developers (an SDK or toolkit), check this again.

## App Privacy (nutrition label)

Matches App/PrivacyInfo.xcprivacy. Screen sharing rows and why: `surfaces/allternit-mobile/docs/app-store-privacy.md`. Data linked to the user, used for App Functionality, not used for tracking:

- Contact info: name, email address
- User content: other user content (chats, files), photos (when attached), audio (voice input)
- Identifiers: device ID (push token)
- Usage data: product interaction

No tracking. No third-party advertising.

## Before submit (checklist)

- [x] Privacy policy at allternit.com/privacy covers the iPhone app, push tokens, screen sharing and account deletion (websites PR, 2026-11-04).
- [x] allternit.com/support exists (websites PR, 2026-11-04) and support@/privacy@ forward to allternitpbc@gmail.com.
- [x] Export compliance (Eoj, 2026-11-04): standard algorithms, France left out of the storefronts. App Store Connect refuses an encryption declaration for standard algorithms outside France ("Cannot create appEncryptionDeclarations unless either containsProprietaryCryptography is True or containsThirdPartyCryptography and availableOnFrenchStore are both True"), and a YES without a code fails upload. So `ITSAppUsesNonExemptEncryption = NO` is correct while France is off. Adding France: ANSSI declaration → App Store Connect declaration (third-party crypto, French store yes) → YES + ITSEncryptionExportComplianceCode.
- [ ] Record the screen-sharing flow (pair, broadcast, Stop) and attach it in App Review → Attachments; then change the notes' "on request" to "attached".
- [x] France removed from the storefronts in App Store Connect (2026-11-04).
- [ ] EU Digital Services Act trader status: 26 EU storefronts show TRADER_STATUS_NOT_PROVIDED and won't list the app until it is declared (App Store Connect → Business). A trader's address, phone and email are shown on the product page. Eoj decides, after the Individual → Allternit LLC conversion.
- [x] Review notes in App Store Connect updated to this text (2026-11-04).
- [ ] Select the newest build (21+, bigger A icon) on version 1.0; it's on build 14.
- [ ] Clerk: Sign in with Apple on; users can delete their own account.
- [ ] Server decision on what deletion does (today: account closed, data kept until privacy@ request; paid plans not cancelled) — the delete screen says this plainly.
- [x] Screenshots: 6 uploaded at 6.7" in App Store Connect.
- [ ] Demo account signs in and its computer is online (fresh accounts get PRECONDITION_REQUIRED without one).
- [ ] Apple Developer: if the App Store team is not the personal team MQ85968PQP, delete the identifier `com.allternit.mobile.screen` registered on MQ85968PQP during a device test (identifiers are unique across Apple), then register the app IDs on the paid team.
