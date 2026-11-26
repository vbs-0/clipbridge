package com.vbs.clipbridge

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.text.InputType
import android.view.Gravity
import android.view.ViewGroup
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast

/**
 * Mode 2 of 2: foreground fallback. Works whether or not the IME is installed,
 * but the clipboard can only be read while this Activity has focus.
 */
class MainActivity : Activity() {

    private val d by lazy { resources.displayMetrics.density }
    private lateinit var status: TextView
    private lateinit var logView: TextView
    private lateinit var pinField: EditText
    private lateinit var typedField: EditText
    private lateinit var historyList: LinearLayout
    private var tickCount = 0
    private val ui = Handler(Looper.getMainLooper())
    private var lastSent = ""
    private val REQ_IMG = 42

    companion object {
        const val BG = 0xFF14120F.toInt()
        const val CARD = 0xFF1E1B17.toInt()
        const val LINE = 0xFF2C2823.toInt()
        const val INK = 0xFFF2EDE4.toInt()
        const val INK3 = 0xFF6F675C.toInt()
        const val ACCENT = 0xFFD9663A.toInt()
        const val AMBER = 0xFFE8B04B.toInt()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Net.start(this)
        Net.onLog = { m -> ui.post { append(m) } }
        Net.onPeerCount = { n -> ui.post { status.text = if (n == 0) "waiting for peers…" else "$n peer(s) online" } }
        setContentView(buildUi())
        handleIntent(intent)
    }

    override fun onNewIntent(intent: Intent?) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        ui.postDelayed({ pushClipboard(false) }, 400)
        ui.post(clipTick)
    }

    override fun onPause() {
        ui.removeCallbacks(clipTick)
        super.onPause()
    }

    /** foreground mode has no clipboard listener, so poll while we are visible */
    private val clipTick = object : Runnable {
        override fun run() {
            pushClipboard(false)
            if (++tickCount % 5 == 0) renderHistory()
            ui.postDelayed(this, 1200)
        }
    }

    override fun onDestroy() {
        Net.onLog = null
        Net.onPeerCount = null
        super.onDestroy()
    }

    private fun dp(v: Int) = (v * d).toInt()

    private fun rounded(c: Int, r: Int, stroke: Int? = null) = GradientDrawable().apply {
        setColor(c)
        cornerRadius = r * d
        if (stroke != null) setStroke(dp(1), stroke)
    }

    private fun label(t: String, s: Float, c: Int, bold: Boolean = false) = TextView(this).apply {
        text = t
        textSize = s
        setTextColor(c)
        if (bold) typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
    }

    private fun buildUi(): android.view.View {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(BG)
            setPadding(dp(20), dp(20), dp(20), dp(20))
        }
        root.addView(label("ClipBridge", 28f, INK, true))
        root.addView(label("phone ⇄ pc clipboard · AES-256", 12f, INK3))
        root.addView(label(Net.localIp(), 12f, AMBER), lp(8))

        status = label("waiting for peers…", 13f, INK, true)
        root.addView(status, lp(18))

        root.addView(label("PAIRING PIN (same on PC)", 10f, INK3, true), lp(20))
        pinField = EditText(this).apply {
            setText(Net.pinValue())
            setTextColor(INK)
            textSize = 16f
            inputType = InputType.TYPE_CLASS_NUMBER
            background = rounded(CARD, 12, LINE)
            setPadding(dp(14), dp(10), dp(14), dp(10))
        }
        root.addView(pinField, lp(8))

        val save = label("Save PIN", 13f, AMBER, true).apply {
            gravity = Gravity.CENTER
            background = rounded(CARD, 12, LINE)
            setPadding(dp(14), dp(12), dp(14), dp(12))
            setOnClickListener {
                Net.setPin(this@MainActivity, pinField.text.toString())
                toast("PIN saved — set the same pin.txt on the PC")
            }
        }
        root.addView(save, lp(8))

        val send = label("Send clipboard to PC", 14f, BG, true).apply {
            gravity = Gravity.CENTER
            background = rounded(ACCENT, 14)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            setOnClickListener { pushClipboard(true) }
        }
        root.addView(send, lp(14))

        // type-and-send: works even when the clipboard is empty or unreadable
        typedField = EditText(this).apply {
            hint = "type here to send to the PC"
            setTextColor(INK)
            setHintTextColor(INK3)
            textSize = 15f
            inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE
            minLines = 2
            background = rounded(CARD, 12, LINE)
            setPadding(dp(14), dp(10), dp(14), dp(10))
        }
        root.addView(typedField, lp(10))

        val sendTyped = label("Send typed text to PC", 14f, BG, true).apply {
            gravity = Gravity.CENTER
            background = rounded(AMBER, 14)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            setOnClickListener {
                val t = typedField.text.toString()
                if (t.isBlank()) {
                    toast("Type something first")
                    return@setOnClickListener
                }
                Net.send(0, t.toByteArray(Charsets.UTF_8), "${t.length} typed chars")
                append("↑ $t")
                typedField.setText("")
            }
        }
        root.addView(sendTyped, lp(8))

        // guaranteed screenshot path: no share grant needed, the picker hands
        // us a readable URI
        val sendImg = label("Send screenshot / image", 14f, INK, true).apply {
            gravity = Gravity.CENTER
            background = rounded(CARD, 14, LINE)
            setPadding(dp(16), dp(14), dp(16), dp(14))
            setOnClickListener {
                val i = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                    addCategory(Intent.CATEGORY_OPENABLE)
                    type = "image/*"
                }
                startActivityForResult(i, REQ_IMG)
            }
        }
        root.addView(sendImg, lp(8))

        val kb = label("Enable background mode (ClipBridge Keyboard)", 12f, INK3).apply {
            gravity = Gravity.CENTER
            background = rounded(CARD, 12, LINE)
            setPadding(dp(14), dp(12), dp(14), dp(12))
            setOnClickListener {
                startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))
                toast("Pick ClipBridge Keyboard, then it syncs in the background")
            }
        }
        root.addView(kb, lp(10))

        logView = label("", 11f, INK3).apply {
            background = rounded(CARD, 14, LINE)
            setPadding(dp(14), dp(12), dp(14), dp(12))
            minLines = 6
            setTextIsSelectable(true)
        }
        root.addView(logView, lp(16))

        // history — the clipboard list Smart Connect keeps in clipboard_history
        val hc = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            background = rounded(CARD, 16, LINE)
            setPadding(dp(16), dp(14), dp(16), dp(14))
        }
        hc.addView(label("HISTORY", 10f, INK3, true))
        historyList = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        hc.addView(historyList, lp(8))
        hc.addView(label("Clear history", 11f, INK3).apply {
            gravity = Gravity.CENTER
            setPadding(dp(8), dp(10), dp(8), dp(2))
            setOnClickListener {
                History.clear(this@MainActivity)
                renderHistory()
            }
        }, lp(4))
        root.addView(hc, lp(14))

        return ScrollView(this).apply { addView(root) }
    }

    /** newest first; tap an entry to put it back on the clipboard and resend */
    private fun renderHistory() {
        if (!::historyList.isInitialized) return
        historyList.removeAllViews()
        val items = History.list(this).take(8)
        if (items.isEmpty()) {
            historyList.addView(label("Nothing synced yet", 11f, INK3))
            return
        }
        for (entry in items) {
            val preview = entry.text.replace("\\n", " ").take(90)
            val tag = if (entry.kind == 1) "IMG " else ""
            val row = label("$tag$preview", 12f, INK).apply {
                background = rounded(CARD, 10, LINE)
                setPadding(dp(12), dp(10), dp(12), dp(10))
                setOnClickListener {
                    val cm = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    cm.setPrimaryClip(ClipData.newPlainText("ClipBridge", entry.text))
                    Net.sendText(entry.text)
                    toast("copied + sent")
                }
            }
            historyList.addView(row, lp(6))
        }
    }

    private fun lp(top: Int) = LinearLayout.LayoutParams(
        ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
    ).apply { topMargin = dp(top) }

    private fun append(m: String) {
        val t = logView.text.toString()
        logView.text = if (t.isEmpty()) m else "$m\n$t".take(1200)
    }

    private fun pushClipboard(manual: Boolean) {
        val sent = Net.sendClipboard(this)
        if (!sent && manual) toast("Nothing new on the clipboard")
    }

    private fun handleIntent(intent: Intent?) {
        android.util.Log.i("CLIPBRIDGE", "handleIntent action=${intent?.action} type=${intent?.type}")
        if (intent?.action != Intent.ACTION_SEND) return
        if (intent.type?.startsWith("image/") == true) {
            // senders disagree: some set EXTRA_STREAM, some only clipData
            val uri = intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
                ?: intent.clipData?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
            if (uri == null) {
                toast("No image in the share")
                return
            }
            sendImageUri(uri)
            return
        }
        val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
        lastSent = text
        Net.sendText(text)
    }

    /** decode -> PNG (the PC only decodes PNG) -> send */
    private fun sendImageUri(uri: Uri) {
        android.util.Log.i("CLIPBRIDGE", "sendImageUri $uri")
        try {
            val png = contentResolver.openInputStream(uri)?.use { input ->
                val bmp = android.graphics.BitmapFactory.decodeStream(input)
                if (bmp == null) null
                else java.io.ByteArrayOutputStream().also {
                    bmp.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
                }.toByteArray()
            }
            if (png == null || png.isEmpty()) {
                toast("Could not read that image")
                return
            }
            Net.send(1, png, "screenshot ${png.size / 1024} KB")
            History.add(this, 1, "[screenshot ${png.size / 1024} KB]")
            toast("sent ${png.size / 1024} KB")
        } catch (e: Exception) {
            toast("image failed: ${e.javaClass.simpleName}")
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == REQ_IMG && resultCode == RESULT_OK) {
            data?.data?.let { sendImageUri(it) }
        }
    }

    private fun toast(m: String) = Toast.makeText(this, m, Toast.LENGTH_SHORT).show()
}
