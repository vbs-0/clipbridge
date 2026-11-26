package com.vbs.clipbridge

import android.content.ClipboardManager
import android.graphics.drawable.GradientDrawable
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.EditorInfo
import android.widget.LinearLayout
import android.widget.TextView

/**
 * Mode 1 of 2: a real QWERTY keyboard. Being the active IME is the only way
 * Android lets an app read the clipboard in the background.
 */
class ClipIME : InputMethodService() {

    private var lastPushed = ""
    private val ui = Handler(Looper.getMainLooper())
    private var upper = false
    private val d by lazy { resources.displayMetrics.density }

    private fun dp(v: Int) = (v * d).toInt()

    override fun onCreate() {
        super.onCreate()
        Net.start(this)
    }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        pushClipboard()
        ui.post(clipTick)
    }

    override fun onFinishInputView(finishingInput: Boolean) {
        ui.removeCallbacks(clipTick)
        super.onFinishInputView(finishingInput)
    }

    /** being the active IME is the only way to read the clipboard in the background */
    private val clipTick = object : Runnable {
        override fun run() {
            pushClipboard()
            ui.postDelayed(this, 1200)
        }
    }

    private fun pushClipboard() {
        try {
            Net.sendClipboard(this)
        } catch (_: Exception) {
        }
    }

    override fun onCreateInputView(): View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(0xFF14120F.toInt())
            setPadding(dp(4), dp(8), dp(4), dp(10))
        }
        root.addView(letterRow("1234567890", 0.92f, 34))
        root.addView(letterRow("qwertyuiop", 1f, 46))
        root.addView(letterRow("asdfghjkl", 1f, 46))
        root.addView(shiftRow())
        root.addView(spaceRow())
        return root
    }

    private fun keyBg(special: Boolean): GradientDrawable = GradientDrawable().apply {
        setColor(if (special) 0xFF2E2A22.toInt() else 0xFF262119.toInt())
        cornerRadius = dp(9).toFloat()
    }

    private fun key(label: String, weight: Float, height: Int, special: Boolean = false, onTap: () -> Unit): TextView =
        TextView(this).apply {
            text = label
            gravity = Gravity.CENTER
            setTextColor(if (special) 0xFFE8B04B.toInt() else 0xFFF2EDE4.toInt())
            textSize = if (height > 40) 17f else 14f
            background = keyBg(special)
            layoutParams = LinearLayout.LayoutParams(0, dp(height), weight).apply {
                setMargins(dp(3), dp(3), dp(3), dp(3))
            }
            setOnClickListener { onTap() }
        }

    private fun letterRow(chars: String, weight: Float, height: Int): View {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }
        for (c in chars) {
            val label = if (upper && c.isLetter()) c.uppercase() else c.toString()
            row.addView(key(label, weight, height) {
                commit(if (upper && c.isLetter()) c.uppercase() else c.toString())
            })
        }
        return row
    }

    private fun shiftRow(): View {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }
        row.addView(key(if (upper) "⇧ ON" else "⇧", 1.5f, 46, true) {
            upper = !upper
            setInputView(onCreateInputView())
        })
        for (c in "zxcvbnm") {
            val label = if (upper) c.uppercase() else c.toString()
            row.addView(key(label, 1f, 46) {
                commit(if (upper) c.uppercase() else c.toString())
            })
        }
        row.addView(key("⌫", 1.5f, 46, true) {
            currentInputConnection?.deleteSurroundingText(1, 0)
        })
        return row
    }

    private fun spaceRow(): View {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
            )
        }
        row.addView(key(",", 1f, 44, true) { commit(",") })
        row.addView(key("space", 4.5f, 44) { commit(" ") })
        row.addView(key(".", 1f, 44, true) { commit(".") })
        row.addView(key("↵", 1.6f, 44, true) { commit("\n") })
        return row
    }

    private fun commit(s: String) {
        currentInputConnection?.commitText(s, 1)
    }
}
