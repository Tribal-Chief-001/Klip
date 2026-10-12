package com.klip.app

import android.app.Activity
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import android.os.Bundle
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.Switch
import android.widget.TextView
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import com.google.zxing.BarcodeFormat
import com.journeyapps.barcodescanner.BarcodeEncoder
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import uniffi.klip_android.isValidPairingTicket

class MainActivity : AppCompatActivity() {

    private lateinit var tvStatus: TextView
    private lateinit var switchRelays: Switch

    private val notificationPermission = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { /* the service works without it; the status notification is just hidden */ }

    private val qrScan = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            result.data?.getStringExtra(QrScannerActivity.EXTRA_SCANNED_TICKET)?.let { joinWithTicket(it) }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)
        tvStatus = findViewById(R.id.tvStatus)
        switchRelays = findViewById(R.id.switchRelays)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            notificationPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        }

        switchRelays.isChecked = KlipRuntime.useRelays(this)
        switchRelays.setOnCheckedChangeListener { _, checked ->
            if (checked != KlipRuntime.useRelays(this)) {
                KlipRuntime.setUseRelays(this, checked)
                restartEngine()
            }
        }

        findViewById<Button>(R.id.btnScan).setOnClickListener { qrScan.launch(Intent(this, QrScannerActivity::class.java)) }
        findViewById<Button>(R.id.btnShowQr).setOnClickListener { showTicketDialog() }
        findViewById<Button>(R.id.btnPaste).setOnClickListener { showPasteDialog() }
        findViewById<Button>(R.id.btnSendNow).setOnClickListener {
            startActivity(
                Intent(this, CopyTrampolineActivity::class.java)
                    .setAction(CopyTrampolineActivity.ACTION_SEND_CLIPBOARD)
            )
        }
        findViewById<Button>(R.id.btnBackground).setOnClickListener { showBackgroundSetup() }
        findViewById<Button>(R.id.btnTestBg).setOnClickListener {
            startService(Intent(this, KlipSyncService::class.java).setAction(KlipSyncService.ACTION_TEST_BG_WRITE))
            Toast.makeText(
                this,
                "Switch to any other app now. In 10 seconds Klip puts a test text on the clipboard — paste it. If it pastes, this phone allows background pasting.",
                Toast.LENGTH_LONG
            ).show()
        }
        findViewById<Button>(R.id.btnBattery).setOnClickListener { requestBatteryExemption() }

        KlipSyncService.start(this)
        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                while (isActive) {
                    renderStatus()
                    delay(2000)
                }
            }
        }
    }

    private fun restartEngine() {
        stopService(Intent(this, KlipSyncService::class.java))
        lifecycleScope.launch {
            delay(600)
            KlipSyncService.start(this@MainActivity)
        }
    }

    // ----------------------------------------------------------------------------- status

    private suspend fun renderStatus() {
        val engine = KlipRuntime.engine
        if (engine == null) {
            tvStatus.text = "Starting the engine…"
            return
        }
        val st = withContext(Dispatchers.IO) { runCatching { engine.status() }.getOrNull() }
        if (st == null) {
            tvStatus.text = "Engine not responding"
            return
        }
        val sb = StringBuilder()
        sb.append("This device   ").append(st.deviceFingerprint).append('\n')
        sb.append("Network       ").append(st.relayMode).append('\n')
        sb.append("Connection    ").append(st.relayState).append('\n')
        if (st.paused) sb.append("Sync          PAUSED\n")
        sb.append('\n')
        if (st.peers.isEmpty()) {
            sb.append("No linked devices yet.\nOn your computer run `klip pair`, then scan its QR.")
        } else {
            sb.append("Linked devices\n")
            for (p in st.peers) {
                sb.append(if (p.online) "  ● " else "  ○ ").append(p.name)
                    .append("  [").append(p.fingerprint).append("]  ")
                    .append(if (p.online) "connected" else "not connected").append('\n')
                p.lastError?.let { sb.append("      last send failed: ").append(it).append('\n') }
            }
        }
        st.lastClipPreview?.let {
            sb.append("\nLast clip: ").append(it.take(60))
            st.lastSyncSecsAgo?.let { s -> sb.append("  (").append(s.toLong()).append("s ago)") }
        }
        tvStatus.text = sb.toString()
    }

    // ------------------------------------------------------------------------ pairing (join)

    private fun showPasteDialog() {
        val input = EditText(this).apply {
            hint = "klip1…"
            setSingleLine(false)
            maxLines = 6
            val clip = (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                .primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()
            if (clip != null && isValidPairingTicket(clip)) setText(clip)
        }
        AlertDialog.Builder(this)
            .setTitle("Paste the ticket")
            .setMessage("Run `klip pair` on your computer and paste the ticket it shows:")
            .setView(input)
            .setPositiveButton("Link") { _, _ -> joinWithTicket(input.text.toString()) }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun joinWithTicket(ticket: String) {
        val engine = KlipRuntime.engine
        if (engine == null) {
            Toast.makeText(this, "Klip is still starting — try again in a moment", Toast.LENGTH_LONG).show()
            return
        }
        if (!isValidPairingTicket(ticket.trim())) {
            Toast.makeText(this, "That is not a valid, unexpired Klip ticket", Toast.LENGTH_LONG).show()
            return
        }
        val waiting = AlertDialog.Builder(this)
            .setTitle("Linking…")
            .setMessage("Approve this phone on the other device when it asks.")
            .setCancelable(false)
            .show()
        lifecycleScope.launch {
            val outcome = withContext(Dispatchers.IO) { runCatching { engine.pairJoin(ticket.trim()) } }
            waiting.dismiss()
            outcome.onSuccess {
                Toast.makeText(this@MainActivity, "Linked with ${it.name}", Toast.LENGTH_LONG).show()
                renderStatus()
            }.onFailure {
                AlertDialog.Builder(this@MainActivity)
                    .setTitle("Could not link")
                    .setMessage(it.message ?: "Unknown error")
                    .setPositiveButton("OK", null)
                    .show()
            }
        }
    }

    // ------------------------------------------------------------------------- pairing (host)

    private fun showTicketDialog() {
        val engine = KlipRuntime.engine ?: run {
            Toast.makeText(this, "Klip is still starting — try again in a moment", Toast.LENGTH_LONG).show()
            return
        }
        val image = ImageView(this).apply {
            layoutParams = LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 900)
            setBackgroundColor(android.graphics.Color.WHITE)
            setPadding(24, 24, 24, 24)
        }
        val caption = TextView(this).apply {
            setPadding(24, 16, 24, 8)
            text = "Preparing…"
        }
        val box = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            addView(image)
            addView(caption)
        }
        var watcher: Job? = null
        val dialog = AlertDialog.Builder(this)
            .setTitle("Link another device")
            .setView(box)
            .setNegativeButton("Close", null)
            .create()
        dialog.setOnDismissListener {
            watcher?.cancel()
            lifecycleScope.launch(Dispatchers.IO) { runCatching { engine.pairCancel() } }
        }
        dialog.show()

        watcher = lifecycleScope.launch {
            val ticket = withContext(Dispatchers.IO) {
                runCatching { engine.pairStart(120UL, !KlipRuntime.useRelays(this@MainActivity)) }
            }.getOrElse {
                caption.text = "Error: ${it.message}"
                return@launch
            }
            image.setImageBitmap(BarcodeEncoder().encodeBitmap(ticket.ticket, BarcodeFormat.QR_CODE, 720, 720))
            caption.text = "On the other device choose “Scan”, or run:\n  klip pair ${ticket.ticket.take(18)}…\n(this device: ${ticket.deviceFingerprint})"
            var asked = false
            while (isActive) {
                delay(500)
                val p = withContext(Dispatchers.IO) { runCatching { engine.pairPoll() }.getOrNull() } ?: continue
                p.pairedDevice?.let {
                    Toast.makeText(this@MainActivity, "Linked with $it", Toast.LENGTH_LONG).show()
                    dialog.dismiss()
                    return@launch
                }
                val pending = p.pending
                if (pending != null && !asked) {
                    asked = true
                    AlertDialog.Builder(this@MainActivity)
                        .setTitle("Link “${pending.deviceName}”?")
                        .setMessage("Device fingerprint: ${pending.fingerprint}\n\nOnly approve a device you just set up yourself.")
                        .setCancelable(false)
                        .setPositiveButton("Approve") { _, _ ->
                            lifecycleScope.launch(Dispatchers.IO) { engine.pairDecide(true) }
                        }
                        .setNegativeButton("Decline") { _, _ ->
                            lifecycleScope.launch(Dispatchers.IO) { engine.pairDecide(false) }
                            dialog.dismiss()
                        }
                        .show()
                } else if (!p.windowOpen && !asked) {
                    Toast.makeText(this@MainActivity, "The code expired. Open it again.", Toast.LENGTH_LONG).show()
                    dialog.dismiss()
                    return@launch
                }
            }
        }
    }

    // ------------------------------------------------------------------ background setup (adb)

    private fun showBackgroundSetup() {
        val logsOk = checkSelfPermission(android.Manifest.permission.READ_LOGS) == android.content.pm.PackageManager.PERMISSION_GRANTED
        val overlayOk = Settings.canDrawOverlays(this)
        val cmd1 = "adb shell pm grant $packageName android.permission.READ_LOGS"
        val msg = """
            Android only lets the app you are looking at read the clipboard, so copying on this phone while another app is in front can't be noticed automatically.

            Without any setup you can still send with: the Klip tile in Quick Settings, the “Send clipboard” button in Klip's notification, or “Send to Klip” in the text selection menu.

            Optional automatic mode (same method KDE Connect uses), needs a computer once:
            1. Enable USB debugging, connect the phone.
            2. Run:  $cmd1
            3. Allow “Display over other apps” for Klip (button below).
            4. Reopen Klip.

            It lets Klip read Android's system log, but Klip only looks at its own clipboard-denied line. Undo any time: adb shell pm revoke $packageName android.permission.READ_LOGS

            Status:  log access ${if (logsOk) "✔ granted" else "✘ not granted"}   ·   overlay ${if (overlayOk) "✔ allowed" else "✘ not allowed"}
        """.trimIndent()
        AlertDialog.Builder(this)
            .setTitle("Automatic copy from this phone")
            .setMessage(msg)
            .setPositiveButton("Overlay permission") { _, _ ->
                startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION, Uri.parse("package:$packageName")))
            }
            .setNeutralButton("Copy command") { _, _ ->
                (getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager)
                    .setPrimaryClip(android.content.ClipData.newPlainText("adb", cmd1))
                Toast.makeText(this, "Command copied", Toast.LENGTH_SHORT).show()
            }
            .setNegativeButton("Close", null)
            .show()
    }

    private fun requestBatteryExemption() {
        val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
        if (pm.isIgnoringBatteryOptimizations(packageName)) {
            Toast.makeText(this, "Already allowed", Toast.LENGTH_SHORT).show()
            return
        }
        try {
            startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:$packageName"))
            )
        } catch (e: Throwable) {
            startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
        }
    }
}
