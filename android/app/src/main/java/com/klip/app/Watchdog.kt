package com.klip.app

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.util.Log

/**
 * Brings Klip back if it was stopped without a restart: the "Stop" button in Android's list of active
 * apps (Android 13+) removes the whole app from memory and sends it no callback. Per the Android
 * documentation, scheduled jobs still run afterwards; and an app whose battery optimisation is turned
 * off (the Setup step) may start a foreground service from the background. A "Force stop" in App info
 * cancels jobs too: nothing can bring the app back from that until the person opens it.
 */
object Watchdog {
    private const val JOB_ID = 7001
    private const val PREFS = "klip_ui"
    private const val KEY = "watchdog_enabled"
    private const val PERIOD_MS = 15 * 60 * 1000L // the shortest period Android allows

    fun enabled(c: Context) = c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY, true)

    fun setEnabled(c: Context, on: Boolean) {
        c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY, on).apply()
        if (on) schedule(c) else cancel(c)
    }

    fun schedule(c: Context) {
        if (!enabled(c)) return
        val js = c.getSystemService(JobScheduler::class.java) ?: return
        if (js.getPendingJob(JOB_ID) != null) return
        js.schedule(
            JobInfo.Builder(JOB_ID, ComponentName(c, WatchdogJobService::class.java))
                .setPeriodic(PERIOD_MS)
                .setPersisted(true) // survives a restart of the phone
                .build()
        )
    }

    fun cancel(c: Context) {
        c.getSystemService(JobScheduler::class.java)?.cancel(JOB_ID)
    }

    const val TAG = "KlipWatchdog"
    fun jobId() = JOB_ID
}

class WatchdogJobService : JobService() {
    override fun onStartJob(params: JobParameters): Boolean {
        if (Watchdog.enabled(this) && !KlipSyncService.running) {
            Log.i(Watchdog.TAG, "Klip's service was not running; starting it")
            KlipSyncService.start(this) // refused by Android (and logged) if the app is not exempt from battery limits
        }
        return false
    }

    override fun onStopJob(params: JobParameters): Boolean = false
}
