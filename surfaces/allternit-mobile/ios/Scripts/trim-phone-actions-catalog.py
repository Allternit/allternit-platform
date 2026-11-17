"""Trim phone-actions-catalog.json to what the iOS app needs (contract 7.2).

Usage: python3 Scripts/trim-phone-actions-catalog.py <phone-actions-catalog.json> Core/Device/phone-actions-catalog.json
Source: handoffs/phone-use-2026-11-16/phone-actions-catalog.json (plan step 5).

Keeps App Intents with isDiscoverable != false (widget/config intents dropped),
community web-research entries (listed with confidence, runnable only through a
shortcut the user added), and derives `kind` per contract 7.2.
"""
import json, re, sys

src, dst = sys.argv[1], sys.argv[2]
d = json.load(open(src))

RULES = [
    ("send", r"^(send|reply|post|message|call)\b"),
    ("create", r"^(create|add|new|log)\b"),
    ("open", r"^open\b"),
    ("search", r"^(find|search)\b"),
    ("read", r"^(get|show|read)\b"),
    ("control", r"^(play|pause|toggle|set|start|stop)\b"),
]

def kind_of(title):
    t = title.strip().lower()
    for k, rx in RULES:
        if re.search(rx, t):
            return k
    return "other"

# Built-in Apple actions the "Allternit Actions" shortcut dispatches on (JSON `action`).
ACTIONS = [
    ("messages.send", "Send a message", "send", ["to", "text"]),
    ("mail.send", "Send an email", "send", ["to", "subject", "body"]),
    ("phone.call", "Call a number", "send", ["to"]),
    ("facetime.call", "FaceTime someone", "send", ["to"]),
    ("notes.create", "Create a note", "create", ["title", "body"]),
    ("reminders.create", "Add a reminder", "create", ["title", "due"]),
    ("calendar.create", "Add a calendar event", "create", ["title", "start", "end"]),
    ("maps.directions", "Get directions", "open", ["to", "mode"]),
    ("music.play", "Play music", "control", ["query"]),
    ("music.pause", "Pause music", "control", []),
    ("photos.latest", "Get latest photos", "read", ["count"]),
    ("files.save", "Save a file", "create", ["name", "text"]),
    ("safari.open", "Open a web page", "open", ["url"]),
    ("translate.text", "Translate text", "other", ["text", "to"]),
    ("clipboard.get", "Get the clipboard", "read", []),
    ("clipboard.set", "Set the clipboard", "control", ["text"]),
    ("focus.set", "Set a Focus", "control", ["focus", "on"]),
    ("wifi.set", "Turn Wi-Fi on or off", "control", ["on"]),
    ("bluetooth.set", "Turn Bluetooth on or off", "control", ["on"]),
    ("low_power.set", "Turn Low Power Mode on or off", "control", ["on"]),
    ("brightness.set", "Set screen brightness", "control", ["level"]),
    ("volume.set", "Set volume", "control", ["level"]),
    ("flashlight.set", "Turn the flashlight on or off", "control", ["on"]),
    ("dnd.set", "Turn Do Not Disturb on or off", "control", ["on"]),
    ("alarm.create", "Set an alarm", "create", ["time", "label"]),
    ("timer.start", "Start a timer", "control", ["seconds"]),
    ("health.log", "Log a health sample", "create", ["type", "value"]),
    ("weather.get", "Get the weather", "read", ["place"]),
    ("wallet.open", "Open a Wallet pass", "open", ["pass"]),
]

out_apps = []
for a in d["apps"]:
    ios = a.get("ios", {})
    intents = []
    for i in ios.get("appIntents", []):
        if i.get("isDiscoverable") is False:
            continue
        intents.append({
            "id": i["id"], "title": i["title"], "kind": kind_of(i["title"]),
            "params": i.get("params", []), "opensApp": bool(i.get("opensApp", False)),
        })
    web = []
    for w in ios.get("webResearchIntents", []):
        web.append({
            "action": w["action"], "kind": w.get("kind", "other"),
            "params": w.get("params", []), "confidence": w.get("confidence", "low"),
        })
    if not intents and not web:
        continue
    out_apps.append({
        "id": a["id"], "name": a["name"], "category": a.get("category", ""),
        "shortcut": f"Allternit {a['name']}", "link": None,
        "intents": intents, "web": web,
    })

out = {
    "built": d["built"],
    "note": "Trimmed for the iOS app from the 2026-11-16 Phone Use catalog: usable App Intents only (isDiscoverable != false), kind derived per contract 7.2. Shortcut links are null until the owner publishes them (gate G3).",
    "actions": {"name": "Allternit Actions", "link": None,
                "actions": [{"id": i, "title": t, "kind": k, "params": p} for i, t, k, p in ACTIONS]},
    "apps": out_apps,
}
json.dump(out, open(dst, "w"), indent=1, ensure_ascii=False)
open(dst, "a").write("\n")
n = sum(len(x["intents"]) for x in out_apps)
print(f"apps={len(out_apps)} intents={n} web={sum(len(x['web']) for x in out_apps)}")
