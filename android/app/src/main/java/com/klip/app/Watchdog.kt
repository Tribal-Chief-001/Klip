package com.klip.app

import android.app.AlarmManager
import android.app.PendingIntent
import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.SystemClock
import android.util.Log

/**
 * Dual-Watchdog Architecture:
 * Brings Klip back if it was stopped or killed in the background.
 *
 * Combines two redundant mechanisms:
 * 1. Primary: AlarmManager exact/inexact while-idle rolling heartbeat alarm.
 *    Fires reliably during Android Doze mode without waiting for hours-long maintenance windows.
 * 2. Secondary: JobScheduler periodic job (Job 7001, 15 min), persisted across reboots.
 */
object Watchdog {
    const val TAG = "KlipWatchdog"
    private const val JOB_ID = 7001
    private const val ALARM_REQ_CODE = 7002
    private const val PREFS = "klip_ui"
    private const val KEY = "watchdog_enabled"
    const val PERIOD_MS = 15 * 60 * 1000L // 15 minutes (shortest periodic JobScheduler interval)

    fun enabled(c: Context): Boolean =
        c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY, true)

    fun setEnabled(c: Context, on: Boolean) {
        c.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY, on).apply()
        if (on) schedule(c) else cancel(c)
    }

    /** Schedules both JobScheduler and AlarmManager redundant watchdogs. */
    fun schedule(c: Context) {
        if (!enabled(c)) return
        scheduleJob(c)
        scheduleAlarm(c)
    }

    private fun scheduleJob(c: Context) {
        val js = c.getSystemService(JobScheduler::class.java) ?: return
        if (js.getPendingJob(JOB_ID) != null) return
        try {
            js.schedule(
                JobInfo.Builder(JOB_ID, ComponentName(c, WatchdogJobService::class.java))
                    .setPeriodic(PERIOD_MS)
                    .setPersisted(true) // survives phone reboots
                    .build()
            )
            Log.d(TAG, "JobScheduler watchdog registered (job=$JOB_ID)")
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to schedule JobScheduler watchdog: ${e.message}")
        }
    }

    /** Schedules the next AlarmManager while-idle heartbeat trigger. */
    fun scheduleAlarm(c: Context, delayMs: Long = PERIOD_MS) {
        if (!enabled(c)) return
        val am = c.getSystemService(Context.ALARM_SERVICE) as? AlarmManager ?: return
        val intent = Intent(c, WatchdogReceiver::class.java)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        val pi = PendingIntent.getBroadcast(c, ALARM_REQ_CODE, intent, flags)

        val triggerAt = SystemClock.elapsedRealtime() + delayMs
        try {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                if (am.canScheduleExactAlarms()) {
                    am.setExactAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, triggerAt, pi)
                    Log.d(TAG, "Exact while-idle alarm scheduled in ${delayMs / 1000}s")
                    return
                }
            }
            am.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, triggerAt, pi)
            Log.d(TAG, "Inexact while-idle alarm scheduled in ${delayMs / 1000}s")
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to schedule AlarmManager watchdog: ${e.message}")
        }
    }

    /** Called periodically by active services to extend the dead-man's heartbeat. */
    fun pokeHeartbeat(c: Context) {
        scheduleAlarm(c, PERIOD_MS)
    }

    fun cancel(c: Context) {
        // Cancel JobScheduler
        try {
            c.getSystemService(JobScheduler::class.java)?.cancel(JOB_ID)
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to cancel JobScheduler: ${e.message}")
        }

        // Cancel AlarmManager
        try {
            val am = c.getSystemService(Context.ALARM_SERVICE) as? AlarmManager
            val intent = Intent(c, WatchdogReceiver::class.java)
            val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
            val pi = PendingIntent.getBroadcast(c, ALARM_REQ_CODE, intent, flags)
            am?.cancel(pi)
            pi.cancel()
        } catch (e: Throwable) {
            Log.w(TAG, "Failed to cancel AlarmManager: ${e.message}")
        }
    }

    fun jobId() = JOB_ID
}

class WatchdogJobService : JobService() {
    override fun onStartJob(params: JobParameters): Boolean {
        if (Watchdog.enabled(this)) {
            if (!KlipSyncService.running) {
                Log.i(Watchdog.TAG, "KlipSyncService was not running; starting from JobScheduler watchdog")
                KlipSyncService.start(this)
            }
            // Keep the AlarmManager watchdog armed as well
            Watchdog.scheduleAlarm(this)
        }
        return false
    }

    override fun onStopJob(params: JobParameters): Boolean = false
}
