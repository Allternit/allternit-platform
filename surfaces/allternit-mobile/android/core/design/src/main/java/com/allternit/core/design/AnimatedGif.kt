package com.allternit.core.design

import android.graphics.BitmapFactory
import android.graphics.ImageDecoder
import android.graphics.drawable.AnimatedImageDrawable
import android.os.Build
import android.widget.ImageView
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import java.nio.ByteBuffer

/** True when [bytes] start with a GIF signature. */
fun isGif(bytes: ByteArray): Boolean =
    bytes.size > 6 && bytes[0] == 'G'.code.toByte() && bytes[1] == 'I'.code.toByte() && bytes[2] == 'F'.code.toByte()

/**
 * Plays a GIF: ImageDecoder + AnimatedImageDrawable on API 28+ (starts, loops with the file); below that the
 * first frame via BitmapFactory (minSdk is 26). No image library.
 */
@Composable
fun AnimatedGif(bytes: ByteArray, modifier: Modifier = Modifier, description: String? = null, scaleType: ImageView.ScaleType = ImageView.ScaleType.FIT_CENTER) {
    val drawable = remember(bytes) {
        runCatching {
            if (Build.VERSION.SDK_INT >= 28) ImageDecoder.decodeDrawable(ImageDecoder.createSource(ByteBuffer.wrap(bytes)))
            else BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.let { android.graphics.drawable.BitmapDrawable(null, it) }
        }.getOrNull()
    }
    DisposableEffect(drawable) {
        (drawable as? AnimatedImageDrawable)?.apply { repeatCount = AnimatedImageDrawable.REPEAT_INFINITE; start() }
        onDispose { (drawable as? AnimatedImageDrawable)?.stop() }
    }
    AndroidView(
        factory = { ctx -> ImageView(ctx).apply { this.scaleType = scaleType; contentDescription = description } },
        update = { it.setImageDrawable(drawable); (drawable as? AnimatedImageDrawable)?.start() },
        modifier = modifier,
    )
}
