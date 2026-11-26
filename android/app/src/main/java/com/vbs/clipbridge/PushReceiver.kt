package com.vbs.clipbridge

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * PLUGGABLE ENTRY POINT.
 *
 * Any app — including any open-source keyboard you already use — can push a clip
 * into ClipBridge with three lines:
 *
 *     val i = Intent("com.vbs.clipbridge.PUSH")
 *     i.setPackage("com.vbs.clipbridge")
 *     i.putExtra("text", myClipboardText)
 *     sendBroadcast(i)
 *
 * ClipBridge then encrypts it with the paired PIN and sends it to the PC. This is
 * how FlorisBoard / HeliBoard / AnySoftKeyboard can adopt ClipBridge without
 * depending on our IME.
 */
class PushReceiver : BroadcastReceiver() {
    override fun onReceive(ctx: Context, intent: Intent) {
        if (intent.action != ACTION_PUSH) return
        val text = intent.getStringExtra(EXTRA_TEXT) ?: return
        if (text.isEmpty()) return
        Net.start(ctx)
        Net.sendText(text)
    }

    companion object {
        const val ACTION_PUSH = "com.vbs.clipbridge.PUSH"
        const val EXTRA_TEXT = "text"
    }
}
