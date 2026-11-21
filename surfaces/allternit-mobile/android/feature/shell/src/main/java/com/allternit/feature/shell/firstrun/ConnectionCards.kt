package com.allternit.feature.shell.firstrun

import androidx.compose.foundation.border
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp

/**
 * One connection as a Grid or Deck card (iOS ConnectionCardView). It reuses [ConnLogo], [ConnStatusLines] and
 * [ConnRowTrailing] from [ConnectionRow], so connect, disconnect, fallback and fix behave the same in every layout.
 * White/neutral surface, never tan; the button takes the full card width.
 */
@Composable
internal fun ConnectionCard(
    entry: ConnEntry,
    store: ConnectionsStore,
    trailing: (@Composable (Modifier) -> Unit)?,
    onConnect: () -> Unit,
    onFallback: (String) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = setupColors
    val shape = RoundedCornerShape(16.dp)
    Column(
        modifier.background(c.card, shape).border(1.dp, c.line, shape).padding(14.dp).testTag("conn-card-${entry.id}"),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        ConnLogo(entry)
        Column { ConnStatusLines(entry, store, null, maxLines = 2) }
        Spacer(Modifier.weight(1f))
        Spacer(Modifier.height(2.dp))
        ConnRowTrailing(entry, store, trailing, onConnect, onFallback, fill = true)
    }
}
