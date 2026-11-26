package com.vbs.clipbridge

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import java.io.ByteArrayOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NetworkInterface
import java.net.ServerSocket
import java.net.Socket
import java.util.Collections

/**
 * One networking core shared by the Activity (foreground mode), the IME
 * (background mode) and third-party pushers (pluggable mode). Whichever of them
 * is alive, this is the only thing talking on the wire.
 */
object Net {
    const val DISCOVERY = 47823
    const val SYNC = 47824
    const val MAGIC = "CLIPB1"

    val peers: MutableSet<String> = Collections.synchronizedSet(mutableSetOf())
    private val seen: MutableMap<String, Long> = Collections.synchronizedMap(mutableMapOf())
    /** live sockets to the PC; the PC dials us, so these are the only way back */
    private val links: MutableList<Socket> = Collections.synchronizedList(mutableListOf())
    /** clip id -> kind,payload,sentAt,tries, awaiting SYNCED from the PC */
    private val outbox = java.util.concurrent.ConcurrentHashMap<String, Array<Any>>()
    private val seenIds: MutableSet<String> = Collections.synchronizedSet(mutableSetOf())
    /** clips captured before the PC dialled in; flushed as soon as a link exists */
    private val queue: MutableList<Pair<Int, ByteArray>> =
        Collections.synchronizedList(mutableListOf())
    var onLog: ((String) -> Unit)? = null
    var onPeerCount: ((Int) -> Unit)? = null

    private var started = false
    private var pin = "000000"
    private var last = ""
    @Volatile private var key: ByteArray = Crypto.key("000000")

    fun setPin(ctx: Context, p: String) {
        pin = p.ifBlank { "000000" }
        key = Crypto.key(pin)
        ctx.getSharedPreferences("cb", 0).edit().putString("pin", pin).apply()
    }

    fun pinValue() = pin

    fun start(ctx: Context) {
        if (started) return
        started = true
        val prefs = ctx.getSharedPreferences("cb", 0)
        var p = prefs.getString("pin", null)
        if (p.isNullOrBlank()) {
            p = (100000..999999).random().toString()
            prefs.edit().putString("pin", p).apply()
        }
        pin = p
        key = Crypto.key(pin)
        Thread { discovery() }.start()
        Thread { listener(ctx) }.start()
        advertise(ctx)
        Thread { ackRetry() }.start()
    }

    /** mDNS: the PC browses `_clipbridge._tcp` and dials us, like Smart Connect */
    private fun advertise(ctx: Context) {
        try {
            val nsd = ctx.getSystemService(Context.NSD_SERVICE) as android.net.nsd.NsdManager
            val info = android.net.nsd.NsdServiceInfo().apply {
                serviceName = "ClipBridge"
                serviceType = "_clipbridge._tcp"
                port = SYNC
            }
            nsd.registerService(info, android.net.nsd.NsdManager.PROTOCOL_DNS_SD,
                object : android.net.nsd.NsdManager.RegistrationListener {
                    override fun onServiceRegistered(i: android.net.nsd.NsdServiceInfo) {
                        log("mDNS: advertising ${i.serviceName}")
                    }
                    override fun onRegistrationFailed(i: android.net.nsd.NsdServiceInfo, e: Int) {
                        log("mDNS failed: $e")
                    }
                    override fun onServiceUnregistered(i: android.net.nsd.NsdServiceInfo) {}
                    override fun onUnregistrationFailed(i: android.net.nsd.NsdServiceInfo, e: Int) {}
                })
        } catch (_: Exception) {
        }
    }

    private fun idOf(payload: ByteArray): String {
        val d = java.security.MessageDigest.getInstance("SHA-256").digest(payload)
        val sb = StringBuilder()
        for (i in 0 until 16) sb.append("%02x".format(d[i]))
        return sb.toString()
    }

    /** three attempts, 3 s apart — the reliability ClipboardDataSynced gives */
    private fun ackRetry() {
        while (true) {
            try {
                Thread.sleep(3000)
                val now = android.os.SystemClock.elapsedRealtime()
                for ((id, rec) in outbox.entries.toList()) {
                    val at = rec[2] as Long
                    var tries = rec[3] as Int
                    if (now - at < 3000L) continue
                    if (tries >= 3) {
                        outbox.remove(id)
                        log("! no ack after 3 tries")
                        continue
                    }
                    tries++
                    rec[2] = now
                    rec[3] = tries
                    log("↻ retry $tries/3")
                    writeAll(rec[0] as Int, rec[1] as ByteArray)
                }
            } catch (_: Exception) {
            }
        }
    }

    private fun log(m: String) {
        android.util.Log.i("CLIPBRIDGE", m)
        onLog?.invoke(m)
    }

    private fun discovery() {
        var sock: DatagramSocket? = null
        try {
            sock = DatagramSocket(null).apply {
                reuseAddress = true
                broadcast = true
                bind(InetSocketAddress(DISCOVERY))
                soTimeout = 500
            }
            val hello = "$MAGIC|$SYNC".toByteArray()
            val buf = ByteArray(128)
            while (true) {
                // drop peers that stopped answering: sends to a dead IP just hang
                val now = android.os.SystemClock.elapsedRealtime()
                val gone = seen.entries.filter { now - it.value > 15000L }.map { it.key }
                for (g in gone) {
                    seen.remove(g)
                    if (peers.remove(g)) {
                        log("- peer $g")
                        onPeerCount?.invoke(peers.size)
                    }
                }
                try {
                    sock.send(DatagramPacket(hello, hello.size, InetAddress.getByName("255.255.255.255"), DISCOVERY))
                } catch (_: Exception) {
                }
                try {
                    val p = DatagramPacket(buf, buf.size)
                    sock.receive(p)
                    val ip = p.address.hostAddress ?: continue
                    if (String(p.data, 0, p.length).startsWith(MAGIC) && ip != localIp()) {
                        seen[ip] = android.os.SystemClock.elapsedRealtime()
                        try {
                            sock.send(DatagramPacket(hello, hello.size, p.address, DISCOVERY))
                        } catch (_: Exception) {
                        }
                        if (peers.add(ip)) {
                            log("+ peer $ip")
                            onPeerCount?.invoke(peers.size)
                        }
                    }
                } catch (_: Exception) {
                }
                // one hello per second: without this the receive never blocks
                // and the phone floods the LAN with broadcasts
                try {
                    Thread.sleep(900)
                } catch (_: InterruptedException) {
                    return
                }
            }
        } catch (e: Exception) {
            log("discovery: ${e.javaClass.simpleName}")
        } finally {
            sock?.close()
        }
    }

    private fun listener(ctx: Context) {
        try {
            ServerSocket(SYNC).use { server ->
                while (true) {
                    val s = server.accept()
                    links.add(s)
                    log("+ pc connected")
                    onPeerCount?.invoke(links.size)
                    Thread { receiveLoop(ctx, s) }.start()
                    flushQueue()
                }
            }
        } catch (e: Exception) {
            log("listen: ${e.javaClass.simpleName}")
        }
    }

    /** one persistent socket carries both directions until it drops */
    private fun receiveLoop(ctx: Context, s: Socket) {
        try {
            s.tcpNoDelay = true
            s.soTimeout = 0
            val input = s.getInputStream()
            while (true) {
                val len = ByteArray(4)
                readFully(input, len)
                val n = (len[0].toInt() and 0xff) or ((len[1].toInt() and 0xff) shl 8) or
                        ((len[2].toInt() and 0xff) shl 16) or ((len[3].toInt() and 0xff) shl 24)
                if (n <= 0 || n > 32 shl 20) break
                val data = ByteArray(n)
                readFully(input, data)
                val plain = Crypto.decrypt(key, data)
                if (plain == null) {
                    log("! wrong PIN")
                    continue
                }
                handle(ctx, plain)
            }
        } catch (_: Exception) {
        } finally {
            links.remove(s)
            try {
                s.close()
            } catch (_: Exception) {
            }
            log("- link closed")
            onPeerCount?.invoke(links.size)
        }
    }

    private fun handle(ctx: Context, plain: ByteArray) {
        if (plain.isEmpty()) return
        val kind = plain[0].toInt()
        val payload = plain.copyOfRange(1, plain.size)

        if (kind == 3) {
            val id = payload.joinToString("") { "%02x".format(it) }
            if (outbox.remove(id) != null) log("✓ synced")
            return
        }

        val id = idOf(payload)
        if (!seenIds.add(id)) {
            // duplicate after a retry: acknowledge again, do not re-apply
            writeAll(3, hexToBytes(id))
            return
        }

        if (kind == 1) {
            ImageSink.save(ctx, payload)
            History.add(ctx, 1, "[screenshot ${payload.size / 1024} KB]")
            writeAll(3, hexToBytes(id))
            return
        }
        val text = String(payload, Charsets.UTF_8)
        if (text.isEmpty()) return
        last = text
        History.add(ctx, 0, text)
        val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        cm.setPrimaryClip(ClipData.newPlainText("ClipBridge", text))
        log("↓ ${text.length} chars from PC")
        writeAll(3, hexToBytes(id))
    }

    private fun hexToBytes(h: String): ByteArray =
        ByteArray(h.length / 2) { h.substring(it * 2, it * 2 + 2).toInt(16).toByte() }

    /** raw write to every live link, no dedupe or retry bookkeeping */
    private fun writeAll(kind: Int, body: ByteArray) {
        val live = links.toList()
        if (live.isEmpty()) return
        val plain = ByteArray(body.size + 1)
        plain[0] = kind.toByte()
        System.arraycopy(body, 0, plain, 1, body.size)
        val frame = Crypto.encrypt(key, plain)
        for (s in live) {
            Thread {
                try {
                    synchronized(s) {
                        val out = s.getOutputStream()
                        val n = frame.size
                        out.write(
                            byteArrayOf(
                                (n and 0xff).toByte(),
                                ((n shr 8) and 0xff).toByte(),
                                ((n shr 16) and 0xff).toByte(),
                                ((n shr 24) and 0xff).toByte()
                            )
                        )
                        out.write(frame)
                        out.flush()
                    }
                } catch (_: Exception) {
                    links.remove(s)
                }
            }.start()
        }
    }

    private fun readFully(input: java.io.InputStream, buf: ByteArray) {
        var off = 0
        while (off < buf.size) {
            val n = input.read(buf, off, buf.size - off)
            if (n < 0) throw java.io.IOException("eof")
            off += n
        }
    }

    fun sendText(text: String, label: String = "${text.length} chars") {
        if (text.isEmpty() || text == last) return
        last = text
        send(0, text.toByteArray(Charsets.UTF_8), label)
    }

    /**
     * Read whatever is on the clipboard — text or an image — and ship it.
     * Images are re-encoded to PNG because the PC side only decodes PNG.
     * Returns true when something was actually sent.
     */
    fun sendClipboard(ctx: Context, label: String? = null): Boolean {
        val cm = ctx.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        val item = cm.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0) ?: return false

        val uri = item.uri
        if (uri != null) {
            val type = try {
                ctx.contentResolver.getType(uri) ?: ""
            } catch (_: Exception) {
                ""
            }
            if (type.startsWith("image/")) {
                val png = try {
                    ctx.contentResolver.openInputStream(uri)?.use { input ->
                        val bmp = BitmapFactory.decodeStream(input) ?: return@use null
                        ByteArrayOutputStream().also {
                            bmp.compress(Bitmap.CompressFormat.PNG, 100, it)
                        }.toByteArray()
                    }
                } catch (_: Exception) {
                    null
                }
                if (png != null && png.isNotEmpty()) {
                    val sig = "1:${png.size}:${java.util.Arrays.hashCode(png)}"
                    if (sig == last) return false
                    last = sig
                    History.add(ctx, 1, "[screenshot ${png.size / 1024} KB]")
                    send(1, png, label ?: "screenshot ${png.size / 1024} KB")
                    return true
                }
            }
        }

        val text = try {
            item.coerceToText(ctx)?.toString()
        } catch (_: Exception) {
            null
        } ?: return false
        if (text.isEmpty() || text == last) return false
        last = text
        History.add(ctx, 0, text)
        send(0, text.toByteArray(Charsets.UTF_8), label ?: "${text.length} chars")
        return true
    }

    private fun flushQueue() {
        val pending = synchronized(queue) {
            val c = queue.toList()
            queue.clear()
            c
        }
        for ((k, p) in pending) {
            log("→ flushing queued ${p.size / 1024} KB")
            send(k, p, "queued ${p.size / 1024} KB")
        }
    }

    fun send(kind: Int, payload: ByteArray, label: String) {
        val id = idOf(payload)
        seenIds.add(id)

        if (links.isNotEmpty()) {
            outbox[id] = arrayOf(kind, payload, android.os.SystemClock.elapsedRealtime(), 0)
            writeAll(kind, payload)
            log("↑ $label")
            return
        }

        if (peers.isEmpty()) {
            // cold start: sharing a screenshot before the PC has dialled in used
            // to drop it on the floor
            synchronized(queue) {
                if (queue.size >= 8) queue.removeAt(0)
                queue.add(kind to payload)
            }
            log("… queued $label — waiting for PC")
            return
        }

        val plain = ByteArray(payload.size + 1)
        plain[0] = kind.toByte()
        System.arraycopy(payload, 0, plain, 1, payload.size)
        val frame = Crypto.encrypt(key, plain)
        val targets = peers.toList()
        if (targets.isEmpty()) {
            log("↑ $label — no PC linked")
            return
        }
        for (ip in targets) {
            Thread {
                try {
                    Socket().use { s ->
                        s.connect(InetSocketAddress(ip, SYNC), 2000)
                        val out = s.getOutputStream()
                        val n = frame.size
                        out.write(
                            byteArrayOf(
                                (n and 0xff).toByte(),
                                ((n shr 8) and 0xff).toByte(),
                                ((n shr 16) and 0xff).toByte(),
                                ((n shr 24) and 0xff).toByte()
                            )
                        )
                        out.write(frame)
                        out.flush()
                    }
                    log("↑ $label to $ip")
                } catch (e: Exception) {
                    log("↑ failed: ${e.javaClass.simpleName}")
                }
            }.start()
        }
    }

    fun localIp(): String {
        return try {
            for (iface in Collections.list(NetworkInterface.getNetworkInterfaces())) {
                if (!iface.isUp || iface.isLoopback) continue
                for (addr in Collections.list(iface.inetAddresses)) {
                    if (!addr.isLoopbackAddress && addr is Inet4Address) {
                        return addr.hostAddress ?: ""
                    }
                }
            }
            "no Wi-Fi address"
        } catch (_: Exception) {
            "no Wi-Fi address"
        }
    }
}
