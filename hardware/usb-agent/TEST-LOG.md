# USB Agent test log

Tests of the Allternit USB Agent on real hardware. One line per case: date, result (pass / fail / not run), and where the evidence is. Do not write "pass" without a photo, a screen recording or command output.

Spec: Allternit Link, Phase 1, steps 1, 2 and 7 (`Research/specs/allternit-link-keychain-bot.md` in the Brain).

## Setup under test

| Item | Value |
|---|---|
| Board | not recorded yet |
| Firmware version | not recorded yet |
| Desktop version | not recorded yet |
| iPhone model and iOS | not recorded yet |
| UxPlay version and how it was installed | not recorded yet |
| Wi-Fi band | not recorded yet |

## Findings before the first run

| Date | Finding | State |
|---|---|---|
| 2026-11-10 | `brew install uxplay` fails on macOS: Homebrew has no `uxplay` formula ("No formulae or casks found for uxplay"). The guide and the README both tell users to run it. | fixed in this PR (docs + wizard copy); real-phone run still not done |

## Step 1: the shipped USB Agent on a real iPhone

| # | Case | Result | Evidence |
|---|---|---|---|
| 1.1 | Desktop **Set up** finds the board over USB, installs the firmware and writes Wi-Fi and the key | not run | |
| 1.2 | The board joins Wi-Fi and Desktop shows it online | not run | |
| 1.3 | Plugged into the iPhone with AssistiveTouch on, a pointer appears | not run | |
| 1.4 | The iPhone shows no accessory prompt, or the prompt is recorded here | not run | |
| 1.5 | Screen Mirroring to "Allternit" works with the four-digit code | not run | |
| 1.6 | The button arms the board and its light turns on | not run | |
| 1.7 | **Calibrate** finds the pointer and finishes | not run | |
| 1.8 | **Try it: go home** goes to the home screen | not run | |
| 1.9 | A bot sets an alarm through `phone.*` tools | not run | |
| 1.10 | Button pressed in the middle of a swipe: input stops at once and the tool call answers `disarmed` | not run | |
| 1.11 | Board unplugged in the middle of a task: the call answers `uncertain` or an error, never ok | not run | |
| 1.12 | Wi-Fi dropped for 15 s: the board releases every key and refuses input | not run | |
| 1.13 | iPhone locked in the middle of a task: status says to unlock, no blind taps | not run | |
| 1.14 | A TCP client without the key is refused | not run | |
| 1.15 | A fresh boot is disarmed, and Desktop cannot arm it | not run | |

Measurements to record:

| What | Value |
|---|---|
| Current drawn from the phone, idle and during a task | not measured (needs an inline USB-C meter) |
| Does the on-screen keyboard hide while the board is plugged in? | not recorded |
| Board surface temperature after 30 min | not measured |
| Behaviour after one hour locked | not recorded |

## Step 2: the iPhone app's screen share

| # | Case | Result | Evidence |
|---|---|---|---|
| 2.1 | The iOS app pairs with this node | not run | |
| 2.2 | The screen broadcast starts from the system sheet | not run | |
| 2.3 | `phone.screenshot` on the app phone returns a frame | not run | |
| 2.4 | Frame size, rate and delay recorded | not run | |

## Step 7: Bluetooth mouse

| # | Case | Result | Evidence |
|---|---|---|---|
| 7.1 | The AssistiveTouch pointer moves over Bluetooth, armed | not run | |
| 7.2 | Disarmed, the pointer does not move | not run | |
