package com.vbs.clipbridge

import android.content.Context

/**
 * Clipboard history, the equivalent of Smart Connect's `clipboard_history`
 * table. Stored as flat lines in SharedPreferences so there is no Room
 * dependency; newest first, capped at MAX.
 */
object History {

    data class Item(val kind: Int, val text: String, val at: Long)

    private const val MAX = 30
    private const val SEP = "\u0001"

    private fun sp(c: Context) = c.getSharedPreferences("cb", Context.MODE_PRIVATE)

    fun add(c: Context, kind: Int, text: String) {
        if (text.isBlank()) return
        val rows = list(c).toMutableList()
        if (rows.firstOrNull()?.text == text) return
        rows.add(0, Item(kind, text, System.currentTimeMillis()))
        val keep = rows.take(MAX)
        sp(c).edit().putString("history", encode(keep)).apply()
    }

    fun list(c: Context): List<Item> {
        val raw = sp(c).getString("history", "") ?: return emptyList()
        if (raw.isBlank()) return emptyList()
        return raw.split("\n").mapNotNull { line ->
            val p = line.split(SEP)
            if (p.size < 3) null
            else Item(p[0].toIntOrNull() ?: 0, p[2], p[1].toLongOrNull() ?: 0L)
        }
    }

    fun clear(c: Context) = sp(c).edit().remove("history").apply()

    private fun encode(items: List<Item>): String =
        items.joinToString("\n") { "${it.kind}$SEP${it.at}$SEP${it.text.replace("\n", "\\n")}" }
}
