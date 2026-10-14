package com.klip.app

import android.content.Context
import android.util.Log

/**
 * Starting and describing the optional clipboard helper (helper/ClipHelper.java). Two ways to start it:
 * from a computer once (the command shown in Setup, run through `adb shell`), or by Klip itself on a rooted
 * phone (through `su`, again at every start of the service, so it survives restarts).
 */
object AutoCopy {
    private const val TAG = "KlipAutoCopy"
    private const val PREFS = "klip_ui"
    private const val KEY_ROOT = "helper_via_root"

    @Volatile var lastRootResult: String? = null
        private set
    private var lastRootTry = 0L

    /** The command that starts the helper in the background, to be run by the shell user (or root). */
    fun shellCommand(ctx: Context): String {
        val apk = ctx.applicationInfo.sourceDir
        val token = ClipHelperServer.tokenText(ctx)
        return "CLASSPATH=$apk nohup setsid app_process /system/bin --nice-name=klip-helper " +
            "com.klip.app.helper.ClipHelper ${ClipHelperServer.PORT} $token $apk </dev/null >/dev/null 2>&1 &"
    }

    /** What the person pastes into a terminal on their computer. */
    fun adbCommand(ctx: Context): String = "adb shell \"${shellCommand(ctx)}\""

    fun rootEnabled(ctx: Context) =
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ROOT, false)

    fun setRootEnabled(ctx: Context, on: Boolean) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ROOT, on).apply()
        if (on) startWithRoot(ctx, force = true) else {
            ClipHelperServer.stopHelper(ctx)
            lastRootResult = null
        }
    }

    /**
     * If the root option is on and no helper is connected, ask `su` to start one. Rate limited. Blocking
     * for a moment (spawns a process): call from a background thread. The root manager may show a prompt
     * the first time; the helper itself drops to the shell user (see ClipHelper.dropRootToShell).
     */
    fun startWithRoot(ctx: Context, force: Boolean = false) {
        if (!rootEnabled(ctx) || ClipHelperServer.connected) return
        val now = System.currentTimeMillis()
        if (!force && now - lastRootTry < 60_000) return
        lastRootTry = now
        val cmd = shellCommand(ctx)
        // `su -c` (Magisk, SuperSU) first, then the plain `su 0 sh -c` form some builds use; the PATH may not
        // contain su for an app, so the usual locations are tried as well.
        var denied = false
        var missing = true
        for (su in listOf("su", "/system/xbin/su", "/system/bin/su", "/sbin/su", "/su/bin/su")) {
            for (form in listOf(arrayOf(su, "-c", cmd), arrayOf(su, "0", "sh", "-c", cmd))) {
                try {
                    val p = Runtime.getRuntime().exec(form)
                    missing = false
                    val finished = p.waitFor(8, java.util.concurrent.TimeUnit.SECONDS)
                    if (finished && p.exitValue() == 0) {
                        lastRootResult = "Asked root to start the helper."
                        Log.i(TAG, lastRootResult!!)
                        return
                    }
                    if (!finished) {
                        p.destroy()
                        lastRootResult = "Root did not answer (is a permission prompt waiting?)"
                        return
                    }
                } catch (e: java.io.IOException) {
                    if (e.message?.contains("error=13") == true) { denied = true; missing = false }
                }
            }
        }
        if (missing) {
            lastRootResult = "No root access found on this phone."
            return
        }
        if (denied) {
            lastRootResult = "Root exists but refused Klip (permission denied). Allow Klip in your root manager."
            Log.w(TAG, lastRootResult!!)
            return
        }
        lastRootResult = "Root was denied or failed."
        Log.w(TAG, lastRootResult!!)
    }
}
