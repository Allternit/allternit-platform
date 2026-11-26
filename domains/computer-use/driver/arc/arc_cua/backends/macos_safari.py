"""Safari for agent runs: a window per run in the "Allternit Agent" profile, and
text that never crosses into another Safari window.

Inside one Safari process, key events go to Safari's key window whatever window
they are addressed to. The 2026-11-26 spike showed an agent's keys landing in
the person's window, and two agents' keys interleaving in a third window. So, for
Safari:

* Text goes in as an accessibility value write (AXValue). The field is focused
  first only when the agent's window is already Safari's key window, or is
  Safari's only window: focusing a web field in another window makes that window
  key, and the person's typing follows it (``focus_allowed``).
* Real keystrokes, for a page that rejects a value write, wait in a queue per
  Safari process (``SafariKeys``). A batch is sent only while the person is not
  using Safari: Safari is not their front app, or they have been idle for more
  than 3 s. The agent's window is made key, checked, typed into, and the previous
  key window is handed back. No batch is sent while another Safari window is key,
  and focus is handed back at once when the person returns.
* A run opens its own window from File > New Window > New "Allternit Agent"
  Window (``open_window``), and closes only windows it opened (``close_window``).
  The person's own windows and profiles are never touched.

Not checked on a real Mac yet: creating the profile by script (``ensure_profile``)
and whether a profile window shows the profile name to accessibility. When the
profile cannot be made, ``SafariProfileMissing`` (code ``profile_missing``) tells
Desktop to show its one-time setup card.
"""

from __future__ import annotations

import logging
import threading
import time
from collections.abc import Callable, Iterable
from typing import Any

from ..errors import InvalidArguments, UnsupportedDesktopAction
from . import macos_background as background

logger = logging.getLogger(__name__)

SAFARI_BUNDLE_IDS = ("com.apple.Safari", "com.apple.SafariTechnologyPreview")
PROFILE_NAME = "Allternit Agent"
IDLE_S = 3.0  # The person counts as away from Safari after this long without input.
HOLD_S = 30.0  # How long queued keys wait for their turn before giving up.
_BURST = 16  # Characters per batch; the gate is checked again between batches.
_POLL_S = 0.1
_OWN_INPUT_SLACK_S = 0.25  # Our own posted events reset the HID idle clock; allow this much jitter.
_OPEN_TIMEOUT_S = 5.0
_CLOSE_TIMEOUT_S = 2.0

SETUP_STEPS = (
    "In Safari, choose Safari > Create Profile… (or Safari > Settings > Profiles > New Profile).",
    f"Name it {PROFILE_NAME} and click Create Profile.",
    f'Done. Agent runs open their windows with File > New Window > New "{PROFILE_NAME}" Window.',
)


class SafariProfileMissing(UnsupportedDesktopAction):
    """Safari has no "Allternit Agent" profile and it could not be made by script."""

    code = "profile_missing"

    def __init__(self, message: str, *, reason: str) -> None:
        super().__init__(message)
        self.data = {"reason": reason, "profile": PROFILE_NAME, "steps": list(SETUP_STEPS)}


class SafariKeysHeld(UnsupportedDesktopAction):
    """Queued keystrokes did not get a safe turn in time; nothing more was typed."""

    code = "keys_held"


# ---- which app -------------------------------------------------------------------

_safari_pids: dict[int, bool] = {}


def is_safari(pid: int) -> bool:
    hit = _safari_pids.get(pid)
    if hit is None:
        try:
            import AppKit  # type: ignore

            app = AppKit.NSRunningApplication.runningApplicationWithProcessIdentifier_(pid)
            hit = app is not None and str(app.bundleIdentifier() or "") in SAFARI_BUNDLE_IDS
        except Exception:
            hit = False
        _safari_pids[pid] = hit
    return hit


def safari_pid(*, launch: bool = True, timeout_s: float = 10.0) -> int:
    """Safari's process ID, starting Safari in the background when it is not running."""
    import AppKit  # type: ignore

    def running() -> int | None:
        for bundle in SAFARI_BUNDLE_IDS[:1]:
            for app in AppKit.NSRunningApplication.runningApplicationsWithBundleIdentifier_(bundle) or []:
                if not app.isTerminated():
                    return int(app.processIdentifier())
        return None

    pid = running()
    if pid is not None or not launch:
        if pid is None:
            raise UnsupportedDesktopAction("Safari is not running")
        return pid
    workspace = AppKit.NSWorkspace.sharedWorkspace()
    url = workspace.URLForApplicationWithBundleIdentifier_(SAFARI_BUNDLE_IDS[0])
    if url is None:
        raise UnsupportedDesktopAction("Safari is not installed")
    config = AppKit.NSWorkspaceOpenConfiguration.configuration()
    config.setActivates_(False)  # Never take the front from the person.
    workspace.openApplicationAtURL_configuration_completionHandler_(url, config, None)
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if (pid := running()) is not None:
            return pid
        time.sleep(_POLL_S)
    raise UnsupportedDesktopAction("Safari did not start")


def safari_windows(pid: int) -> list[int]:
    """Safari's normal on-screen windows (layer 0), front to back."""
    Q = background._quartz()
    options = Q.kCGWindowListOptionOnScreenOnly | Q.kCGWindowListExcludeDesktopElements
    out = []
    for info in Q.CGWindowListCopyWindowInfo(options, Q.kCGNullWindowID) or []:
        if int(info.get(Q.kCGWindowOwnerPID, -1)) == pid and int(info.get(Q.kCGWindowLayer, 0) or 0) == 0:
            if (number := int(info.get(Q.kCGWindowNumber, 0) or 0)):
                out.append(number)
    return out


def focus_allowed(pid: int, window: int) -> bool:
    """Whether focusing a field in ``window`` leaves the person's typing alone. Any
    app other than Safari: yes. Safari: only when ``window`` is already its key
    window, or is its only window."""
    if not is_safari(pid):
        return True
    try:
        if background.key_window(pid) == window:
            return True
        return safari_windows(pid) == [window]
    except Exception:
        return False


# ---- the key gate (pure, unit tested) -----------------------------------------------


def person_away(*, safari_front: bool, person_idle_s: float, idle_s: float = IDLE_S) -> bool:
    """The person is not using Safari: it is not their front app, or they are idle."""
    return not safari_front or person_idle_s > idle_s


def may_send(
    *, agent_window: int, key_window: int | None, safari_front: bool, person_idle_s: float, idle_s: float = IDLE_S,
) -> tuple[bool, str]:
    """Whether a batch of keys may go to Safari now, and why not. Keys go only to
    Safari's key window, so the agent's window must be the key window, and the
    person must not be using Safari (keys would race their typing)."""
    if key_window != agent_window:
        return False, "another Safari window is key" if key_window is not None else "Safari has no key window"
    if not person_away(safari_front=safari_front, person_idle_s=person_idle_s, idle_s=idle_s):
        return False, "the person is using Safari"
    return True, ""


class Presence:
    """Seconds since the person's last input. Our own posted events also reset the
    HID idle clock, so when the newest input is ours, the person's last input is
    the one seen before it."""

    def __init__(self, probe: Callable[[], float] | None = None, clock: Callable[[], float] = time.monotonic) -> None:
        self._probe = probe or _hid_idle_s
        self._clock = clock
        self._own: float | None = None  # When we last posted an event.
        self._person: float | None = None  # When the person last gave input, as far as we know.

    def own_input(self) -> None:
        self._own = self._clock()

    def person_idle_s(self) -> float:
        now = self._clock()
        newest = now - self._probe()
        if self._own is not None and abs(newest - self._own) <= _OWN_INPUT_SLACK_S:
            # The newest input is ours: the person has done nothing since before it.
            return now - self._person if self._person is not None else 0.0
        self._person = newest
        return now - newest


def _hid_idle_s() -> float:
    Q = background._quartz()
    return float(Q.CGEventSourceSecondsSinceLastEventType(Q.kCGEventSourceStateHIDSystemState, 0xFFFFFFFF))


class SafariKeys:
    """The keystroke queue: one sender at a time per Safari process, each batch sent
    only when ``may_send`` holds, and the previous key window handed back after."""

    def __init__(
        self,
        *,
        front_pid: Callable[[], int | None] | None = None,
        key_window: Callable[[int], int | None] | None = None,
        make_key: Callable[[int, int], bool] | None = None,
        presence: Presence | None = None,
        sleep: Callable[[float], None] = time.sleep,
        clock: Callable[[], float] = time.monotonic,
        idle_s: float = IDLE_S,
        hold_s: float = HOLD_S,
    ) -> None:
        self._front_pid = front_pid or background.front_pid
        self._key_window = key_window or background.key_window
        self._make_key = make_key or make_key_window
        self.presence = presence or Presence()
        self._sleep = sleep
        self._clock = clock
        self.idle_s = idle_s
        self.hold_s = hold_s
        self._guard = threading.Lock()
        self._locks: dict[int, threading.Lock] = {}

    def _lock(self, pid: int) -> threading.Lock:
        with self._guard:
            return self._locks.setdefault(pid, threading.Lock())

    def send(self, pid: int, window: int, batches: Iterable[Callable[[], None]]) -> None:
        """Run each batch (which posts keys to ``pid``) only while the gate holds.
        Raises SafariKeysHeld when a batch gets no safe turn within ``hold_s``."""
        lock = self._lock(pid)
        if not lock.acquire(timeout=self.hold_s):
            raise SafariKeysHeld(f"Another agent is still typing in Safari; window {window} gets no turn")
        handed: list[int | None] = [None]  # The key window to hand back.
        try:
            deadline = self._clock() + self.hold_s
            for batch in batches:
                self._turn(pid, window, deadline, handed)
                batch()
                self.presence.own_input()
        finally:
            back = handed[0]
            if back is not None and back != window and self._key_window(pid) == window:
                self._make_key(pid, back)
            lock.release()

    def _turn(self, pid: int, window: int, deadline: float, handed: list[int | None]) -> None:
        why = "no turn"
        while True:
            safari_front = self._front_pid() == pid
            idle = self.presence.person_idle_s()
            key = self._key_window(pid)
            if not person_away(safari_front=safari_front, person_idle_s=idle, idle_s=self.idle_s):
                # The person is in Safari: give their window back now, then wait.
                if key == window and handed[0] not in (None, window):
                    self._make_key(pid, handed[0])
                why = "the person is using Safari"
            else:
                if key != window:
                    if handed[0] is None:
                        handed[0] = key
                    self._make_key(pid, window)
                    key = self._key_window(pid)
                ok, why = may_send(agent_window=window, key_window=key, safari_front=self._front_pid() == pid,
                                   person_idle_s=idle, idle_s=self.idle_s)
                if ok:
                    return
            if self._clock() >= deadline:
                raise SafariKeysHeld(f"Keys for Safari window {window} were held: {why}. Nothing more was typed.")
            self._sleep(_POLL_S)



def make_key_window(pid: int, window: int) -> bool:
    """Make ``window`` Safari's main (and key) window without bringing Safari forward."""
    from .macos_app import find_ax_window

    AS = background._ax()
    ax_window = find_ax_window(AS.AXUIElementCreateApplication(pid), window)
    if ax_window is None:
        return False
    return AS.AXUIElementSetAttributeValue(ax_window, "AXMain", True) == 0


KEYS = SafariKeys()  # The one queue for this process: every agent's Safari keys go through it.


def type_text(
    pid: int, window: int, text: str, *, field: Any = None, replace: bool = False,
    check: Callable[[], None] | None = None, keys: SafariKeys | None = None,
) -> None:
    """Type ``text`` into Safari ``window`` through the key queue. With ``field`` (an
    AX element), it is focused first, and with ``replace`` its text is selected so
    the typing replaces it; both happen inside the gate, while the window is key."""
    queue = keys or KEYS
    characters = list(text)

    def burst(chunk: list[str]) -> Callable[[], None]:
        def run() -> None:
            if check is not None:
                check()
            with background.borrowed_focus(pid, window, keys=True):
                for character in chunk:
                    background._post_key(pid, 0, 0, character)
                time.sleep(0.02)
        return run

    batches: list[Callable[[], None]] = []
    if field is not None:
        def prepare() -> None:
            AS = background._ax()
            AS.AXUIElementSetAttributeValue(field, "AXFocused", True)
            had_text = bool(background.copy_attribute(AS, field, "AXValue")[1])
            if replace and had_text:
                background.select_all(field)
                if not characters:
                    with background.borrowed_focus(pid, window, keys=True):
                        background._post_key(pid, 51)  # Backspace clears the selection.
        batches.append(prepare)
    batches.extend(burst(characters[i:i + _BURST]) for i in range(0, len(characters), _BURST))
    queue.send(pid, window, batches)


def press(pid: int, window: int, code: int, flags: int = 0, *, keys: SafariKeys | None = None) -> None:
    """Press one key in Safari ``window`` through the key queue."""
    def run() -> None:
        with background.borrowed_focus(pid, window, keys=True):
            background._post_key(pid, code, flags)
            time.sleep(0.03)
    (keys or KEYS).send(pid, window, [run])


# ---- a window per run in the agent profile -------------------------------------------

_opened: set[tuple[int, int]] = set()  # (pid, window) for every window this process opened.
_open_lock = threading.Lock()


def _plain(title: str) -> str:
    return " ".join(title.replace("“", "").replace("”", "").replace('"', "").split()).casefold()


def profile_window_command(pid: int, profile: str = PROFILE_NAME) -> tuple[str, ...] | None:
    """The menu path that opens a new window in ``profile``: File > New Window > New
    "<profile>" Window. None when Safari has no such profile."""
    from .macos_menus import read_commands

    wanted = _plain(f"New {profile} Window")
    for command in read_commands(pid, query=profile):
        if command.path and command.path[0] == "File" and _plain(command.path[-1]) == wanted:
            return command.path
    return None


def _create_command(pid: int) -> tuple[str, ...] | None:
    from .macos_menus import read_commands

    for command in read_commands(pid, query="Create Profile"):
        if command.path and _plain(command.path[-1]).startswith("create profile"):
            return command.path
    return None


def is_agent_window(pid: int, window: int) -> bool:
    """Whether this process opened ``window`` for an agent run."""
    return (pid, window) in _opened


def open_window(*, pid: int | None = None, profile: str = PROFILE_NAME, create_profile: bool = True) -> Any:
    """Open a new Safari window in the agent profile and return its WindowTarget.
    Raises SafariProfileMissing when there is no profile and it cannot be made."""
    from ..driver import WindowTarget
    from .macos_app import MacOSApp
    from .macos_menus import run_command

    pid = pid or safari_pid()
    command = profile_window_command(pid, profile)
    if command is None:
        ensure_profile(pid, profile, create=create_profile)
        command = profile_window_command(pid, profile)
        if command is None:
            raise SafariProfileMissing(f"Safari has no {profile} profile", reason="not_found")
    app = MacOSApp(pid)
    with _open_lock:  # One open at a time, so a new window is never taken for another run's.
        before = _all_window_ids(app)
        with app.input_scope():
            run_command(pid, command)
        deadline = time.monotonic() + _OPEN_TIMEOUT_S
        new: list[int] = []
        while time.monotonic() < deadline:
            new = [w for w in _all_window_ids(app) if w not in before]
            if new:
                time.sleep(0.2)  # Let a second window (the person opening one) show up too.
                new = [w for w in _all_window_ids(app) if w not in before]
                break
            time.sleep(_POLL_S)
        if len(new) > 1:
            marked = [w for w in new if profile_marked(pid, w, profile)]
            new = marked if len(marked) == 1 else new
        if len(new) != 1:
            raise UnsupportedDesktopAction(
                "Could not tell which new Safari window is the agent's" if new else "Safari opened no new window"
            )
        _opened.add((pid, new[0]))
    logger.info("opened Safari %s window %s", profile, new[0])
    return WindowTarget(pid, new[0])


def close_window(target: Any) -> bool:
    """Close a window ``open_window`` opened; True once it is gone. Any other window
    is refused and left alone. False when Safari kept it open (a sheet asking about
    its tabs): it is left for the person."""
    from .macos_app import find_ax_window

    key = (target.pid, target.window_id)
    if key not in _opened:
        raise InvalidArguments(f"Safari window {target.window_id} was not opened for an agent run; it is left alone")
    AS = background._ax()
    ax_window = find_ax_window(AS.AXUIElementCreateApplication(target.pid), target.window_id)
    if ax_window is None:
        _opened.discard(key)
        return True
    button = background.copy_attribute(AS, ax_window, "AXCloseButton")[1]
    if button is None or AS.AXUIElementPerformAction(button, "AXPress") != 0:
        return False
    deadline = time.monotonic() + _CLOSE_TIMEOUT_S
    while time.monotonic() < deadline:
        if target.window_id not in safari_windows(target.pid):
            _opened.discard(key)
            return True
        time.sleep(_POLL_S)
    return False


def profile_marked(pid: int, window: int, profile: str = PROFILE_NAME) -> bool:
    """Whether the window shows the profile name to accessibility (its title, or a
    toolbar item). Only a tie-break: Safari may not show it at all."""
    from .macos_app import find_ax_window

    AS = background._ax()
    ax_window = find_ax_window(AS.AXUIElementCreateApplication(pid), window)
    if ax_window is None:
        return False
    needle = profile.casefold()

    def seen(element: Any, depth: int) -> bool:
        for name in ("AXTitle", "AXDescription", "AXHelp"):
            value = background.copy_attribute(AS, element, name)[1]
            if isinstance(value, str) and needle in value.casefold():
                return True
        if depth >= 3:
            return False
        children = background.copy_attribute(AS, element, "AXChildren")[1] or ()
        return any(seen(child, depth + 1) for child in children)

    return seen(ax_window, 0)


def ensure_profile(pid: int, profile: str = PROFILE_NAME, *, create: bool = True) -> None:
    """Make sure the profile exists, creating it through Safari > Create Profile…
    when allowed and safe. Raises SafariProfileMissing otherwise, with the steps for
    Desktop's setup card. Only windows this step opened are closed."""
    from .macos_app import MacOSApp
    from .macos_menus import run_command

    if profile_window_command(pid, profile) is not None:
        return
    if not create:
        raise SafariProfileMissing(f"Safari has no {profile} profile", reason="not_found")
    if background.front_pid() == pid and Presence().person_idle_s() <= IDLE_S:
        raise SafariProfileMissing(f"Safari has no {profile} profile, and the person is using Safari",
                                   reason="person_using_safari")
    command = _create_command(pid)
    if command is None:
        raise SafariProfileMissing(f"Safari has no {profile} profile and no Create Profile command",
                                   reason="no_create_command")
    app = MacOSApp(pid)
    before = set(_all_window_ids(app))
    try:
        with app.input_scope():
            run_command(pid, command)
        if not _fill_profile_sheet(pid, profile, before):
            raise SafariProfileMissing(f"Could not fill Safari's Create Profile form for {profile}",
                                       reason="create_failed")
        deadline = time.monotonic() + _OPEN_TIMEOUT_S
        while time.monotonic() < deadline and profile_window_command(pid, profile) is None:
            time.sleep(_POLL_S)
        if profile_window_command(pid, profile) is None:
            raise SafariProfileMissing(f"Safari did not create the {profile} profile", reason="create_failed")
    finally:
        for window in set(_all_window_ids(app)) - before:  # The Settings window this step opened.
            _press_close(pid, window)


def _fill_profile_sheet(pid: int, profile: str, before: set[int]) -> bool:
    """Type the name into the Create Profile form and press its Create button. The
    form is looked for only in windows (and their sheets) that were not there before,
    or in Safari's Settings window."""
    AS = background._ax()
    app_ax = AS.AXUIElementCreateApplication(pid)
    deadline = time.monotonic() + _OPEN_TIMEOUT_S
    while time.monotonic() < deadline:
        for ax_window in background.copy_attribute(AS, app_ax, "AXWindows")[1] or ():
            identifier = background.window_id(ax_window)
            title = background.copy_attribute(AS, ax_window, "AXTitle")[1] or ""
            if identifier in before and "profile" not in str(title).casefold() and "settings" not in str(title).casefold():
                continue  # Never a browsing window that was already open.
            field, button = _form_parts(AS, ax_window)
            if field is None or button is None:
                continue
            if AS.AXUIElementSetAttributeValue(field, "AXValue", profile) != 0:
                return False
            time.sleep(0.1)
            return AS.AXUIElementPerformAction(button, "AXPress") == 0
        time.sleep(_POLL_S)
    return False


def _form_parts(AS: Any, root: Any) -> tuple[Any, Any]:
    field = button = None
    stack = [(root, 0)]
    while stack:
        element, depth = stack.pop()
        role = background.copy_attribute(AS, element, "AXRole")[1]
        if role == "AXTextField" and field is None:
            field = element
        elif role == "AXButton" and button is None:
            title = background.copy_attribute(AS, element, "AXTitle")[1]
            if isinstance(title, str) and _plain(title).startswith("create"):
                button = element
        if depth < 8:
            stack.extend((child, depth + 1) for child in background.copy_attribute(AS, element, "AXChildren")[1] or ())
    return field, button


def _press_close(pid: int, window: int) -> None:
    from .macos_app import find_ax_window

    AS = background._ax()
    ax_window = find_ax_window(AS.AXUIElementCreateApplication(pid), window)
    button = background.copy_attribute(AS, ax_window, "AXCloseButton")[1] if ax_window is not None else None
    if button is not None:
        AS.AXUIElementPerformAction(button, "AXPress")


def _all_window_ids(app: Any) -> list[int]:
    return [int(info["window_id"]) for info in app.all_windows()]
