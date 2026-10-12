package com.klip.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.widget.Toast

/** Sends text to the linked devices from a short-lived entry point (share sheet, text menu, tile). */
object SendHelper {
    fun sendText(ctx: Context, text: String) {
        val app = ctx.applicationContext
        val engine = KlipRuntime.engine
        if (engine == null) {
            KlipSyncService.start(app)
            toast(app, "Klip is starting — try again in a moment")
            return
        }
        Thread {
            val msg = try {
                val r = engine.sendText(text)
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
