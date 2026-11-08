package com.klip.app

import android.companion.CompanionDeviceService
import android.util.Log

/**
 * Companion device service for Android's CompanionDeviceManager framework.
 *
 * Associating with a companion device (e.g. computer profile via REQUEST_COMPANION_PROFILE_COMPUTER)
 * grants standard exemptions from battery restrictions and allows the CompanionDeviceManager to
 * bind to this service and wake up the application when nearby paired devices appear.
 */
class KlipCompanionService : CompanionDeviceService() {

    companion object {
        private const val TAG = "KlipCompanionService"
    }

    override fun onCreate() {
        super.onCreate()
        Log.i(TAG, "CompanionDeviceService created; ensuring KlipSyncService is running")
        KlipSyncService.start(this)
    }

    @Deprecated("Deprecated in Java")
    override fun onDeviceAppeared(address: String) {
        Log.i(TAG, "Companion device appeared ($address); starting sync service")
        KlipSyncService.start(this)
    }

    @Deprecated("Deprecated in Java")
    override fun onDeviceDisappeared(address: String) {
        Log.d(TAG, "Companion device disappeared ($address)")
    }
}
