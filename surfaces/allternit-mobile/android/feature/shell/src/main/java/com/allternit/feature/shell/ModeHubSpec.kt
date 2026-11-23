package com.allternit.feature.shell

import android.provider.Settings
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.annotation.DrawableRes
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.platform.LocalContext
import com.allternit.core.design.R as DesignR

/*
 * Mode hub spec: the Android port of the frozen contract
 *   handoffs/composer-cleanup-2026-11-21/CONTRACT.md  (source: allternit-ai feat/mode-hub, src/shell/mode-hub/contract.ts).
 * iOS ports the same file as ModeHubSpec.swift. Changes after the freeze need a main-session OK.
 *
 * A0 inventory (Android, base PR #2793)
 *
 *  Contract section -> existing Android screen it opens ("none" = hidden on Android, no new screen)
 *
 *  | Mode  | Section id     | Opens on Android                                                          |
 *  |-------|----------------|---------------------------------------------------------------------------|
 *  | Home  | recents        | ShellPage.AllChats (All chats)                                            |
 *  | Home  | projects       | rail "projects" (ProjectsListScreen)                                      |
 *  | Home  | artifacts      | rail "artifacts-library" (ArtifactsLibraryScreen)                         |
 *  | Home  | automation     | rail "automation-tasks" (AutomationRoot)                                  |
 *  | Home  | meetings       | rail "meetings" (MeetingsScreen); drawer has it with no flag              |
 *  | Home  | customize      | rail "customize" (CustomizeRoot)                                          |
 *  | Home  | freeweights    | rail "models" (ModelsRoot, Catalog = Freeweights browse)                  |
 *  | Bots  | main-bot       | rail "bots" with BotsLaunch(openMain = true)                              |
 *  | Bots  | bots           | rail "bots" (BotsModeRoot launchpad)                                      |
 *  | Bots  | groups         | rail "bots" with BotsLaunch(route = BotRoute.Groups)                      |
 *  | Bots  | office         | rail "bots" with BotsLaunch(route = BotRoute.Office)                      |
 *  | Bots  | bot-projects   | rail "bots" (launchpad lists bot projects; BotRoute.Project needs a row)  |
 *  | Code  | sessions       | ShellPage.AllChats filtered to Code (the rail list)                       |
 *  | Code  | code-projects  | rail "projects"                                                           |
 *  | Code  | code-artifacts | rail "artifacts-library"                                                  |
 *  | Code  | code-automation| rail "code-automations" (AutomationRoot)                                  |
 *  | ACI   | computers      | rail "computers" (AciRoot, AciTab.Computers)                              |
 *  | ACI   | mcp-apps       | rail "mcp-apps" (AciTab.Apps)                                             |
 *  | ACI   | extensions     | rail "browser-extensions" (AciTab.Extensions)                             |
 *  | ACI   | teach          | rail "site-apis" (AciNav.openTeach)                                       |
 *
 *  No contract section is "none": every row has a screen, so none is hidden. Icon gaps (no Phosphor drawable in
 *  core/design yet): Shapes, Timer, Barbell, Robot. A1 draws the nearest existing glyph (see [HubSection.icon]);
 *  adding the four drawables is a design-asset task, not part of this lane.
 */

/** How a section opens. A2/A3 map these onto ShellPage / BotsLaunch in WorkspaceShell. */
internal sealed interface HubTarget {
    data object AllChats : HubTarget
    data class Rail(val id: String) : HubTarget
    /** Bots mode root with a launch hint. */
    data class Bots(val hint: BotsHint) : HubTarget
}

internal enum class BotsHint { Root, MainBot, Groups, Office }

/** One contract row (CONTRACT.md "Sections per mode"). [more] is the "More" row label. */
internal data class HubSection(
    val id: String,
    val label: String,
    val short: String,
    @DrawableRes val icon: Int,
    val target: HubTarget,
    val more: String,
)

internal object ModeHubSpec {
    /** MODE_BUTTONS order: Home, Bots, Code, ACI (ShellMode declaration order). */
    val modes: List<ShellMode> = ShellMode.entries

    private val clockBack = DesignR.drawable.ph_clock_counter_clockwise
    private val folder = DesignR.drawable.ph_folder
    private val artifactsIcon = DesignR.drawable.ph_file_text // Shapes: no drawable yet
    private val timer = DesignR.drawable.ph_clock // Timer: no drawable yet

    private val home = listOf(
        HubSection("recents", "Recents", "Recents", clockBack, HubTarget.AllChats, "All chats"),
        HubSection("projects", "Projects", "Projects", folder, HubTarget.Rail("projects"), "All projects"),
        HubSection("artifacts", "Artifacts", "Artifacts", artifactsIcon, HubTarget.Rail("artifacts-library"), "All artifacts"),
        HubSection("automation", "Automation Tasks", "Automations", timer, HubTarget.Rail("automation-tasks"), "All tasks"),
        HubSection("meetings", "Meetings", "Meetings", DesignR.drawable.ph_video_camera, HubTarget.Rail("meetings"), "Open Meetings"),
        HubSection("customize", "Customize", "Customize", DesignR.drawable.ph_sliders_horizontal, HubTarget.Rail("customize"), "Open Customize"),
        HubSection("freeweights", "Freeweights Studio", "Freeweights", DesignR.drawable.ph_cpu /* Barbell: none */, HubTarget.Rail("models"), "Open Freeweights"),
    )
    private val bots = listOf(
        HubSection("main-bot", "Main bot", "Main bot", DesignR.drawable.ph_star, HubTarget.Bots(BotsHint.MainBot), "Open main bot"),
        HubSection("bots", "Bots", "Bots", DesignR.drawable.ph_cpu /* Robot: none */, HubTarget.Bots(BotsHint.Root), "All bots"),
        HubSection("groups", "Group chats", "Groups", DesignR.drawable.ph_users_three, HubTarget.Bots(BotsHint.Groups), "All group chats"),
        HubSection("office", "Office", "Office", DesignR.drawable.ph_buildings, HubTarget.Bots(BotsHint.Office), "Open Office"),
        HubSection("bot-projects", "Projects", "Projects", folder, HubTarget.Bots(BotsHint.Root), "All projects"),
    )
    private val code = listOf(
        HubSection("sessions", "Sessions", "Sessions", DesignR.drawable.ph_terminal_window, HubTarget.AllChats, "All sessions"),
        HubSection("code-projects", "Projects", "Projects", folder, HubTarget.Rail("projects"), "All projects"),
        HubSection("code-artifacts", "Artifacts", "Artifacts", artifactsIcon, HubTarget.Rail("artifacts-library"), "All artifacts"),
        HubSection("code-automation", "Automation Tasks", "Automations", timer, HubTarget.Rail("code-automations"), "All tasks"),
    )
    private val aci = listOf(
        HubSection("computers", "Computers", "Computers", DesignR.drawable.ph_desktop, HubTarget.Rail("computers"), "All computers"),
        HubSection("mcp-apps", "MCP Apps", "Apps", DesignR.drawable.ph_squares_four, HubTarget.Rail("mcp-apps"), "All apps"),
        HubSection("extensions", "ACI Extensions", "Extensions", DesignR.drawable.ph_puzzle_piece, HubTarget.Rail("browser-extensions"), "All extensions"),
        HubSection("teach", "Teach", "Teach", DesignR.drawable.ph_record_fill, HubTarget.Rail("site-apis"), "Open Teach"),
    )

    /** The sections shown on Android for [mode]; a section with no native screen would be filtered here (none today). */
    fun sections(mode: ShellMode): List<HubSection> = when (mode) {
        ShellMode.Home -> home
        ShellMode.Bots -> bots
        ShellMode.Code -> code
        ShellMode.Aci -> aci
    }

    /** New orb main item per mode (CONTRACT.md "New orb"). Same strings as [ShellMode.newLabel]. */
    fun newItem(mode: ShellMode): String = mode.newLabel

    /** Quick task shows in the New pill only when a session is open. */
    const val QUICK_TASK_LABEL = "Quick task"

    // Timings (CONTRACT.md "Timings and motion").
    const val HOLD_MILLIS = 450L
    const val PRESS_SCALE = 0.94f
    const val OPEN_ITEMS = 4 // Frozen contract: up to four rows, then More.
    const val ROW_HEIGHT_DP = 42
    const val CARD_WIDTH_DP = 128
    const val CARD_HEIGHT_DP = 66

    // Spring (stiffness, damping) -> Compose (stiffness / mass, ratio = damping / (2 sqrt(stiffness * mass))).
    const val ORB_STIFFNESS = 380f; const val ORB_DAMPING_RATIO = 0.77f
    const val LENS_STIFFNESS = 743f; const val LENS_DAMPING_RATIO = 0.68f
    const val SHEET_STIFFNESS = 467f; const val SHEET_DAMPING_RATIO = 0.98f

    // Events (web names; Android hosts call these through the shell, not a broadcast).
    const val EVENT_OPEN_HUB = "allternit:open-mode-hub"
    const val EVENT_MODE_ARRIVE = "allternit:mode-arrive"
}

/** Hub haptics (CONTRACT.md "Haptics"). */
internal enum class HubHaptic {
    /** Hold completes, open an item, New pick. */
    Medium,
    /** Lens moves. */
    Selection,
    /** Orb tap, card pick. */
    Light;

    fun play(view: View) {
        view.performHapticFeedback(
            when (this) {
                Medium -> HapticFeedbackConstants.LONG_PRESS
                Selection -> HapticFeedbackConstants.CLOCK_TICK
                Light -> HapticFeedbackConstants.VIRTUAL_KEY
            },
        )
    }
}

/** System reduce motion (animator duration scale 0): no springs or slides, states change at once. */
@Composable
internal fun rememberHubReducedMotion(): Boolean {
    val context = LocalContext.current
    return remember(context) {
        runCatching {
            Settings.Global.getFloat(context.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
        }.getOrDefault(false)
    }
}
