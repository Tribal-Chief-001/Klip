package com.klip.app

import android.accessibilityservice.AccessibilityService
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.util.Log
import android.view.accessibility.AccessibilityEvent

class KlipAccessibilityService : AccessibilityService() {

    companion object {
        private const val TAG = "KlipAccessibility"
        private var instance: KlipAccessibilityService? = null

        fun injectRemoteClip(context: Context, text: String, isSensitive: Boolean = false) {
            instance?.let { service ->
                try {
                    val clipboard = service.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                    val clip = ClipData.newPlainText("Klip", text)
                    if (isSensitive && Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                        clip.description.extras?.putBoolean("android.content.extra.IS_SENSITIVE", true)
                    }
                    clipboard.setPrimaryClip(clip)
                    Log.i(TAG, "Successfully injected remote clip (${text.length} chars) silently via Accessibility Service")
                } catch (e: Exception) {
                    Log.e(TAG, "Failed to inject remote clip: ${e.message}", e)
                }
            } ?: run {
                Log.w(TAG, "Accessibility service instance not bound. Falling back to background clipboard manager")
            }
        }

        fun isServiceRunning(): Boolean = instance != null
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        Log.i(TAG, "Klip Accessibility Service connected and active")
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        // Event listener for optional active input detection or paste shortcuts
    }

    override fun onInterrupt() {
        Log.w(TAG, "Klip Accessibility Service interrupted")
    }

    override fun onDestroy() {
        super.onDestroy()
        if (instance == this) {
            instance = null
        }
        Log.i(TAG, "Klip Accessibility Service destroyed")
    }
}
