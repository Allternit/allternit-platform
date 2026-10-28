# App Store privacy notes (iOS app)

What to answer on App Store Connect's App Privacy form, and why. Keep this in
step with `ios/App/PrivacyInfo.xcprivacy` and
`ios/AllternitScreen/PrivacyInfo.xcprivacy`.

## iPhone link (Settings → Phones)

The owner can pair the app with one of their own Allternit Desktops so their
bots can see the iPhone screen and use the app's ACI browser
(`SPEC-iphone-app-screen-2026-10-27.md`).

| Data | Collected? | Linked to user | Tracking | Purpose | Notes |
|---|---|---|---|---|---|
| Screen content and app sound (ReplayKit broadcast) | Other User Content | Yes | No | App Functionality | Only while the owner broadcasts from the system picker. Sent to the owner's own Desktop, sealed with a key made on the phone and that Desktop during pairing; api.allternit.com relays it and can't read it. Nothing is encoded until someone watches. The microphone is never sent. |
| ACI browser page snapshots (element list + JPEG) | Other User Content | Yes | No | App Functionality | Only when a paired bot calls `web.snapshot` while the app is open, sealed the same way. Password field values are never read. |
| Phone id (random UUID made by the app) and phone name | Device ID | Yes | No | App Functionality | Stored with the owner's account (`app_phones`) so the Desktop knows which phone is which. Not the IDFA or any hardware id. |

## Review notes (guideline 2.5.14)

- A broadcast starts only from `RPSystemBroadcastPickerView`, limited to the
  `com.allternit.mobile.screen` extension; a bot can only ask the app to show
  the picker. iOS's own red recording indicator shows throughout.
- While broadcasting, the app shows "Your bot can see this screen" with Stop.
- While a bot drives the ACI browser, the browser shows "Bot is using the
  browser" with Stop.
- Public APIs only: ReplayKit, WebKit (`WKContentWorld` user script),
  URLSession WebSockets, CryptoKit.
