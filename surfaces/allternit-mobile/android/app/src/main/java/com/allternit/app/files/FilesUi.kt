package com.allternit.app.files

import android.app.Activity
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.LinearLayout
import android.widget.TextView
import com.allternit.app.R

/**
 * Small view builders for the Files screens. Plain framework views (no appcompat),
 * colours from res/values(-night)/files_theme.xml: white + neutral fill, graphite dark.
 */
internal object FilesUi {
    fun dp(a: Activity, v: Int): Int = (v * a.resources.displayMetrics.density).toInt()

    fun color(a: Activity, id: Int): Int = a.getColor(id)

    fun column(a: Activity, padH: Int = 24, padV: Int = 24): LinearLayout = LinearLayout(a).apply {
        orientation = LinearLayout.VERTICAL
        setPadding(dp(a, padH), dp(a, padV), dp(a, padH), dp(a, padV))
        setBackgroundColor(color(a, R.color.files_bg))
    }

    fun title(a: Activity, text: String): TextView = TextView(a).apply {
        this.text = text
        textSize = 20f
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        setTextColor(color(a, R.color.files_text))
        setPadding(0, 0, 0, dp(a, 8))
    }

    fun body(a: Activity, text: String): TextView = TextView(a).apply {
        this.text = text
        textSize = 15f
        setLineSpacing(0f, 1.2f)
        setTextColor(color(a, R.color.files_text_secondary))
        setPadding(0, 0, 0, dp(a, 16))
    }

    /** A pill button. `primary` = filled; otherwise the neutral fill. */
    fun button(a: Activity, text: String, primary: Boolean, onClick: () -> Unit): TextView = TextView(a).apply {
        this.text = text
        textSize = 15f
        gravity = Gravity.CENTER
        typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        minHeight = dp(a, 48)
        setPadding(dp(a, 20), dp(a, 12), dp(a, 20), dp(a, 12))
        setTextColor(color(a, if (primary) R.color.files_on_primary else R.color.files_text))
        background = GradientDrawable().apply {
            cornerRadius = dp(a, 24).toFloat()
            setColor(color(a, if (primary) R.color.files_primary else R.color.files_fill))
        }
        isClickable = true
        isFocusable = true
        setOnClickListener { onClick() }
    }

    /** A list row: title + optional subtitle, full width, 56dp min. */
    fun row(a: Activity, title: String, subtitle: String?, onClick: (() -> Unit)?): View = LinearLayout(a).apply {
        orientation = LinearLayout.VERTICAL
        minimumHeight = dp(a, 56)
        gravity = Gravity.CENTER_VERTICAL
        setPadding(dp(a, 16), dp(a, 10), dp(a, 16), dp(a, 10))
        addView(TextView(a).apply {
            text = title
            textSize = 16f
            setTextColor(color(a, R.color.files_text))
        })
        if (subtitle != null) {
            addView(TextView(a).apply {
                text = subtitle
                textSize = 13f
                setTextColor(color(a, R.color.files_text_secondary))
            })
        }
        if (onClick != null) {
            isClickable = true
            isFocusable = true
            val attrs = a.obtainStyledAttributes(intArrayOf(android.R.attr.selectableItemBackground))
            background = attrs.getDrawable(0)
            attrs.recycle()
            setOnClickListener { onClick() }
        }
    }

    fun gap(a: Activity, h: Int): View = View(a).apply { layoutParams = LinearLayout.LayoutParams(1, dp(a, h)) }

    fun matchWidth(v: View): View = v.apply {
        layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT)
    }
}
