# In-App Events — Allternit for iPhone

Source for the three In-App Events (plan step 13). Limits: name 30, short description 50, long description 120. Numbers in brackets are character counts. Events are drafts until Eoj approves them; none is submitted.

Image specs, all three events:

| Asset | Size | Note |
|---|---|---|
| Event card | 1920 x 1080 | Shown on the product page and in search |
| Event details image | 1080 x 1920 | Vertical; Apple's current guidance, confirm in App Store Connect when uploading |

Images are made from real app captures, not mockups, and opened at full size for a defect pass before upload (overlap, clipping, wraps).

Badge for all three: Major Update.

| # | Name | Short description | Long description |
|---|---|---|---|
| 1 | Plans and credits in the app (28) | Buy a plan or credits without leaving the app. (46) | Plus, Max and Ultra plans and credit packs are now in Allternit for iPhone. Credits work on all your devices. (109) |
| 2 | Bots in iMessage (16) | Message your bots from the Messages app. (40) | Add Allternit to Messages and talk to your bots in a thread. They ask before anything that matters. (99) |
| 3 | Your computer on your phone (27) | Watch your computer's screen and take control. (46) | See what your bots are doing on your Mac or PC, approve what they ask, and take control from your iPhone. (105) |

## Event settings

| Setting | Event 1 | Event 2 | Event 3 |
|---|---|---|---|
| Event type | Major Update | Major Update | Major Update |
| Badge | Major Update | Major Update | Major Update |
| Deep link | None. The app has no link that opens the plan screen from outside it; the event opens the app. Add one only if a universal link route exists by submission. | None, same reason | None, same reason |
| Event start | `<start date and time, set at submission>` | `<start date and time, set at submission>` | `<start date and time, set at submission>` |
| Event end | `<end date and time, at most 31 days after start>` | `<end date and time>` | `<end date and time>` |
| Publish date | `<publish date, up to 14 days before start>` | `<publish date>` | `<publish date>` |
| Purchase requirements | None (the plans themselves need the Paid Apps Agreement, which is active) | None | None |
| Priority | Normal | Normal | Normal |

Event 1 should start only after the in-app purchase build is approved and live. Event 2 should start only after the Messages extension is in the shipping build.

## Risk

Event 3 is not tied to a date or a new change, so it is the one most likely to be rejected. If App Review rejects it, drop it; the other two stand alone.
