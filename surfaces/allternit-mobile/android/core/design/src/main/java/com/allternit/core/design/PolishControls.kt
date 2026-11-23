package com.allternit.core.design

import android.os.Build
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInParent
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Haptic kinds from the polish contract (light open, soft close/dock, selection pick, medium add/Continue). */
object PolishHaptic {
    enum class Kind { Light, Soft, Selection, Medium }

    fun constantFor(kind: Kind, sdkInt: Int = Build.VERSION.SDK_INT): Int = when (kind) {
        Kind.Light -> HapticFeedbackConstants.VIRTUAL_KEY
        Kind.Soft -> HapticFeedbackConstants.CLOCK_TICK
        Kind.Selection ->
            if (sdkInt >= Build.VERSION_CODES.R) HapticFeedbackConstants.SEGMENT_TICK else HapticFeedbackConstants.CLOCK_TICK
        Kind.Medium ->
            if (sdkInt >= Build.VERSION_CODES.R) HapticFeedbackConstants.CONTEXT_CLICK else HapticFeedbackConstants.LONG_PRESS
    }

    fun perform(view: View, kind: Kind): Boolean = view.performHapticFeedback(constantFor(kind))
}

/** Neutral glass and ink from the approved mockup: no tan; dark is graphite. */
object PolishGlass {
    val LightFill = Color(0xB8FFFFFF) // white .72
    val LightBorder = Color(0x171F1E1D) // rgba(31,30,29,.09)
    val DarkFill = Color(0xB82C2C2E)
    val DarkBorder = Color(0x1AFFFFFF)
    val Coral = Color(0xFFD97757)
    val GreyTrack = Color(0xFFF4F4F4)
    val DarkTrack = Color(0xFF2A2A2C)

    @Composable
    fun isDark(): Boolean = AllternitTheme.colors == AllternitColors.Dark

    @Composable
    fun fill() = if (isDark()) DarkFill else LightFill

    @Composable
    fun border() = if (isDark()) DarkBorder else LightBorder

    @Composable
    fun track() = if (isDark()) DarkTrack else GreyTrack
}

/** Springs from the contract: press 600/24, lens 500/34, base 420/34 (ratio = damping / (2 sqrt stiffness)). */
private fun <T> pressSpring() = spring<T>(dampingRatio = 24f / (2f * 24.495f), stiffness = 600f)
private fun <T> lensSpring() = spring<T>(dampingRatio = 34f / (2f * 22.361f), stiffness = 500f)
private fun <T> baseSpring() = spring<T>(dampingRatio = 34f / (2f * 20.494f), stiffness = 420f)

@Composable
private fun Modifier.glass(shape: Shape): Modifier {
    val dark = PolishGlass.isDark()
    return this
        .shadow(
            elevation = 6.dp, shape = shape,
            ambientColor = Color.Black.copy(alpha = if (dark) .35f else .08f),
            spotColor = Color.Black.copy(alpha = if (dark) .35f else .12f),
        )
        .clip(shape)
        .background(PolishGlass.fill())
        .border(BorderStroke(1.dp, PolishGlass.border()), shape)
}

/** Round glass button (the ☰, back and close buttons). Press scale .88. */
@Composable
fun GlassCircleButton(
    onClick: () -> Unit,
    contentDescription: String,
    modifier: Modifier = Modifier,
    size: Dp = 40.dp,
    content: @Composable () -> Unit,
) {
    val view = LocalView.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val s by androidx.compose.animation.core.animateFloatAsState(if (pressed) 0.88f else 1f, pressSpring(), label = "circlePress")
    Box(
        modifier
            .size(size)
            .scale(s)
            .glass(CircleShape)
            .semantics { this.contentDescription = contentDescription }
            .clickable(interactionSource = source, indication = null, role = Role.Button) {
                PolishHaptic.perform(view, PolishHaptic.Kind.Light)
                onClick()
            },
        contentAlignment = Alignment.Center,
    ) { content() }
}

/** Glass pill (the right-hand pill). Press scale .94. */
@Composable
fun GlassPill(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    height: Dp = 40.dp,
    contentPadding: PaddingValues = PaddingValues(horizontal = 14.dp),
    content: @Composable () -> Unit,
) {
    val view = LocalView.current
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val s by androidx.compose.animation.core.animateFloatAsState(if (pressed) 0.94f else 1f, pressSpring(), label = "pillPress")
    Box(
        modifier
            .height(height)
            .scale(s)
            .glass(CircleShape)
            .clickable(interactionSource = source, indication = null, role = Role.Button) {
                PolishHaptic.perform(view, PolishHaptic.Kind.Light)
                onClick()
            }
            .padding(contentPadding),
        contentAlignment = Alignment.Center,
    ) { content() }
}

/**
 * Filter pills. The picked pill is an ink chip that slides to the new pill on a spring,
 * with a selection haptic. Scrolls sideways when the row is wider than the screen.
 */
@Composable
fun FilterPills(
    options: List<String>,
    selectedIndex: Int,
    onSelect: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = AllternitTheme.colors
    val view = LocalView.current
    val density = LocalDensity.current
    val frames = remember { mutableStateMapOf<Int, Pair<Float, Float>>() } // index -> x, width in px
    val target = frames[selectedIndex]
    val x by animateDpAsState(with(density) { (target?.first ?: 0f).toDp() }, baseSpring(), label = "pillX")
    val w by animateDpAsState(with(density) { (target?.second ?: 0f).toDp() }, baseSpring(), label = "pillW")
    Box(modifier.horizontalScroll(rememberScrollState())) {
        if (target != null) {
            Box(Modifier.offset(x = x).width(w).height(34.dp).clip(CircleShape).background(colors.textPrimary))
        }
        Row(horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(8.dp)) {
            options.forEachIndexed { i, label ->
                val picked = i == selectedIndex
                val text by animateColorAsState(if (picked) colors.bgPrimary else colors.textSecondary, baseSpring(), label = "pillText")
                Box(
                    Modifier
                        .height(34.dp)
                        .onGloballyPositioned { frames[i] = it.positionInParent().x to it.size.width.toFloat() }
                        .clip(CircleShape)
                        .then(if (picked) Modifier else Modifier.background(colors.fillNeutral))
                        .semantics { selected = picked }
                        .clickable(role = Role.Tab) {
                            if (!picked) {
                                PolishHaptic.perform(view, PolishHaptic.Kind.Selection)
                                onSelect(i)
                            }
                        }
                        .padding(horizontal = 14.dp),
                    contentAlignment = Alignment.Center,
                ) { Text(label, color = text, fontSize = 13.sp, fontWeight = FontWeight.Medium) }
            }
        }
    }
}

/** Equal-width segmented control: a white thumb slides on a grey track (List / Graph / Calendar, Yours / Discover). */
@Composable
fun SlidingSegmented(
    options: List<String>,
    selectedIndex: Int,
    onSelect: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val colors = AllternitTheme.colors
    val view = LocalView.current
    val dark = PolishGlass.isDark()
    val shape = RoundedCornerShape(12.dp)
    BoxWithConstraints(modifier.fillMaxWidth().height(40.dp).clip(shape).background(PolishGlass.track()).padding(3.dp)) {
        val segment = maxWidth / options.size.coerceAtLeast(1)
        val x by animateDpAsState(segment * selectedIndex, lensSpring(), label = "thumbX")
        Box(
            Modifier
                .offset(x = x)
                .width(segment)
                .height(34.dp)
                .shadow(2.dp, RoundedCornerShape(10.dp), ambientColor = Color.Black.copy(alpha = .06f), spotColor = Color.Black.copy(alpha = .08f))
                .clip(RoundedCornerShape(10.dp))
                .background(if (dark) Color(0xFF48484A) else Color.White),
        )
        Row {
            options.forEachIndexed { i, label ->
                Box(
                    Modifier
                        .width(segment)
                        .height(34.dp)
                        .semantics { selected = i == selectedIndex }
                        .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, role = Role.Tab) {
                            if (i != selectedIndex) {
                                PolishHaptic.perform(view, PolishHaptic.Kind.Selection)
                                onSelect(i)
                            }
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        label, fontSize = 13.sp, fontWeight = FontWeight.Medium,
                        color = if (i == selectedIndex) colors.textPrimary else colors.textSecondary,
                    )
                }
            }
        }
    }
}

/** Small caps-style section label above a group. */
@Composable
fun SectionLabel(text: String, modifier: Modifier = Modifier) {
    Text(
        text, color = AllternitTheme.colors.textSecondary, fontSize = 12.sp, fontWeight = FontWeight.SemiBold,
        letterSpacing = 0.6.sp, modifier = modifier.padding(top = 18.dp, bottom = 8.dp),
    )
}

/** Empty state: neutral fill, radius 20, a + circle, a title and one grey line. Tap runs [onClick] when given. */
@Composable
fun EmptyBox(
    title: String,
    description: String,
    modifier: Modifier = Modifier,
    onClick: (() -> Unit)? = null,
    icon: @Composable () -> Unit = { Text("+", color = AllternitTheme.colors.textPrimary, fontSize = 18.sp, fontWeight = FontWeight.Medium) },
) {
    val colors = AllternitTheme.colors
    val shape = RoundedCornerShape(20.dp)
    Column(
        modifier
            .fillMaxWidth()
            .clip(shape)
            .background(PolishGlass.track())
            .then(if (onClick != null) Modifier.clickable(role = Role.Button, onClick = onClick) else Modifier)
            .padding(horizontal = 18.dp, vertical = 22.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier.size(34.dp).clip(CircleShape).background(if (PolishGlass.isDark()) Color(0xFF3A3A3C) else Color.White)
                .border(1.dp, PolishGlass.border(), CircleShape),
            contentAlignment = Alignment.Center,
        ) { icon() }
        Text(title, color = colors.textPrimary, fontSize = 14.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
        Text(
            description, color = colors.textSecondary, fontSize = 12.sp, lineHeight = 17.sp,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center, modifier = Modifier.padding(top = 6.dp),
        )
    }
}

/**
 * A tab page: [title] slot (the block-type title lives in another lane), a grey
 * description, then [content], with 14 dp gutters. The caller owns scrolling.
 */
@Composable
fun TabPage(
    description: String,
    modifier: Modifier = Modifier,
    title: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    Column(modifier.fillMaxWidth().padding(horizontal = 14.dp)) {
        title()
        Text(
            description, color = AllternitTheme.colors.textSecondary, fontSize = 14.sp, lineHeight = 20.sp,
            modifier = Modifier.padding(top = 6.dp, bottom = 14.dp),
        )
        content()
    }
}
