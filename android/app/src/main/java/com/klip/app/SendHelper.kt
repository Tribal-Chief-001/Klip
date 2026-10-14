package com.klip.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.widget.Toast

/** Sends text or an image to the linked devices from a short-lived entry point (share sheet, text menu, tile). */
object SendHelper {
    fun sendText(ctx: Context, text: String) = send(ctx) { it.sendText(text) }

    /** [raw] is whatever the other app gave us; it is converted to PNG/WebP on the worker thread. */
    fun sendImage(ctx: Context, raw: ByteArray) = send(ctx) { it.sendImage(ImageSender.prepare(raw)) }

    private fun send(ctx: Context, action: (uniffi.klip_android.KlipEngine) -> uniffi.klip_android.SendReport) {
        val app = ctx.applicationContext
        val engine = KlipRuntime.engine
        if (engine == null) {
            KlipSyncService.start(app)
            toast(app, "Klip is starting — try again in a moment")
            return
        }
        Thread {
            val msg = try {
                val r = action(engine)
                when {
                    r.delivered.isNotEmpty() && r.failed.isEmpty() -> "Klip: sent to ${r.delivered.joinToString()}"
                    r.delivered.isNotEmpty() ->
                        "Klip: sent to ${r.delivered.joinToString()} — failed: " +
                            r.failed.joinToString { "${it.device} (${it.reason})" }
                    r.failed.isNotEmpty() -> "Klip: not delivered — " + r.failed.joinToString { "${it.device}: ${it.reason}" }
                    else -> "Klip: no linked devices yet"
                }
            } catch (e: Throwable) {
                "Klip: ${e.message}"
            }
            toast(app, msg)
        }.start()
    }

    private fun toast(ctx: Context, msg: String) {
        Handler(Looper.getMainLooper()).post { Toast.makeText(ctx, msg, Toast.LENGTH_LONG).show() }
    }
}
