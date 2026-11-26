package com.allternit.feature.artifacts

import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier

/**
 * The "Show" chips on the Artifacts page (Artifacts redesign 6.5; web `ShowChips.tsx`, iOS ArtifactsShowChips):
 * Everything, Apps, Plans, Shared. They replace the Owner part of the filter sheet. Plans is a placeholder
 * until 6.6 lands the plans list.
 */
enum class ShowChip(val label: String) { Everything("Everything"), Apps("Apps"), Plans("Plans"), Shared("Shared") }

@Composable
fun ArtifactsShowChips(value: ShowChip, onChange: (ShowChip) -> Unit, modifier: Modifier = Modifier) {
    PillRow(ShowChip.entries, value, { it.label }, { "artifactsShow${it.name}Chip" }, modifier, onChange)
}
