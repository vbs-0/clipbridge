package com.vbs.clipbridge

import android.content.ContentValues
import android.content.Context
import android.graphics.BitmapFactory
import android.provider.MediaStore

object ImageSink {
    fun save(ctx: Context, png: ByteArray) {
        try {
            val bmp = BitmapFactory.decodeByteArray(png, 0, png.size) ?: return
            val values = ContentValues().apply {
                put(MediaStore.Images.Media.DISPLAY_NAME, "clip_${System.currentTimeMillis()}.png")
                put(MediaStore.Images.Media.MIME_TYPE, "image/png")
                put(MediaStore.Images.Media.RELATIVE_PATH, "Pictures/ClipBridge")
            }
            val uri = ctx.contentResolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values) ?: return
            ctx.contentResolver.openOutputStream(uri)?.use {
                bmp.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it)
            }
            Net.onLog?.invoke("↓ screenshot ${bmp.width}×${bmp.height} → Pictures/ClipBridge")
        } catch (e: Exception) {
            Net.onLog?.invoke("↓ image failed: ${e.javaClass.simpleName}")
        }
    }
}
