package com.klip.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class WatchdogTest {

    @Test
    fun watchdog_constants_match_spec() {
        assertEquals("KlipWatchdog", Watchdog.TAG)
        assertEquals(7001, Watchdog.jobId())
        assertEquals(15 * 60 * 1000L, Watchdog.PERIOD_MS)
    }

    @Test
    fun watchdog_period_is_within_android_allowed_minimum() {
        // JobScheduler enforces a 15-minute minimum periodic interval
        assertTrue(Watchdog.PERIOD_MS >= 15 * 60 * 1000L)
    }
}
