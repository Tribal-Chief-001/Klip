package com.klip.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class OemTest {

    @Test
    fun vivo_profile_contains_all_vital_defenses() {
        val vivo = Oem.VIVO
        assertEquals("vivo", vivo.id)
        assertEquals("Vivo / iQOO", vivo.title)
        assertTrue(vivo.needsExtraSteps)

        // Check autostart guidance
        assertTrue(
            "Vivo steps must instruct enabling Autostart",
            vivo.steps.any { it.contains("Autostart", ignoreCase = true) }
        )

        // Check proprietary background power consumption guidance
        assertTrue(
            "Vivo steps must instruct configuring Background power consumption management",
            vivo.steps.any { it.contains("Background power consumption management", ignoreCase = true) }
        )

        // Check recent apps padlock guidance
        assertTrue(
            "Vivo steps must instruct padlocking Klip in Recent Apps",
            vivo.steps.any { it.contains("Recent apps", ignoreCase = true) && it.contains("Lock", ignoreCase = true) }
        )

        // Check app info unrestricted battery guidance
        assertTrue(
            "Vivo steps must instruct setting App info battery to Unrestricted",
            vivo.steps.any { it.contains("App info", ignoreCase = true) && it.contains("Unrestricted", ignoreCase = true) }
        )
    }

    @Test
    fun oem_needs_extra_steps_accurately_flags_custom_skins() {
        assertTrue(Oem.VIVO.needsExtraSteps)
        assertTrue(Oem.XIAOMI.needsExtraSteps)
        assertTrue(Oem.OPPO.needsExtraSteps)
        assertTrue(Oem.SAMSUNG.needsExtraSteps)
        assertTrue(Oem.HUAWEI.needsExtraSteps)
        assertFalse(Oem.STOCK.needsExtraSteps)
    }

    @Test
    fun stock_profile_does_not_require_autostart_lock() {
        val stock = Oem.STOCK
        assertEquals("stock", stock.id)
        assertFalse(stock.needsExtraSteps)
        assertTrue(stock.steps.first().contains("Unrestricted"))
    }
}
