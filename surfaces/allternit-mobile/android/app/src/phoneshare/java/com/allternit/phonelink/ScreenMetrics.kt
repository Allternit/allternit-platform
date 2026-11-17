package com.allternit.phonelink

import android.content.Context
import android.hardware.display.DisplayManager
import android.util.DisplayMetrics
import android.view.Display
import com.allternit.phonelink.core.DisplayInfo

/** The default display in real pixels, current orientation (works from a service: no window context needed). */
object ScreenMetrics {
    data class Size(val width: Int, val height: Int)

    @Suppress("DEPRECATION")
    fun displayInfo(context: Context): DisplayInfo {
        val display = context.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val m = DisplayMetrics()
        display.getRealMetrics(m)
        return DisplayInfo(m.widthPixels, m.heightPixels, m.densityDpi, display.rotation * 90)
    }

    fun real(context: Context): Size = displayInfo(context).let { Size(it.width, it.height) }
}
