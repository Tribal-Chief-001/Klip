package com.klip.app

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import android.widget.Toast
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity

class MainActivity : AppCompatActivity() {

    private lateinit var tvStatus: TextView
    private lateinit var tvAccessibilityNotice: TextView
    private lateinit var btnEnableAccessibility: Button
    private lateinit var btnPair: Button
    private lateinit var btnSimulatePaste: Button

    private val notificationPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission()
    ) { granted ->
        if (!granted) {
            Toast.makeText(this, "Notifications required for UPI-safe clip sync", Toast.LENGTH_SHORT).show()
        }
    }

    private val qrScanLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            val token = result.data?.getStringExtra(QrScannerActivity.EXTRA_SCANNED_TOKEN)
            if (!token.isNullOrEmpty()) {
                applyPairingToken(token)
            }
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        tvStatus = findViewById(R.id.tvStatus)
        tvAccessibilityNotice = findViewById(R.id.tvAccessibilityNotice)
        btnEnableAccessibility = findViewById(R.id.btnEnableAccessibility)
        btnPair = findViewById(R.id.btnPair)
        btnSimulatePaste = findViewById(R.id.btnSimulatePaste)

        checkNotificationPermission()
        setupListeners()
        startSyncService()
        updateStatusUI()
    }

    private fun checkNotificationPermission() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
                notificationPermissionLauncher.launch(android.Manifest.permission.POST_NOTIFICATIONS)
            }
        }
    }

    override fun onResume() {
        super.onResume()
        updateAccessibilityUI()
        updateStatusUI()
    }

    private fun startSyncService() {
        val intent = Intent(this, KlipSyncService::class.java).apply {
            action = KlipSyncService.ACTION_START
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            startForegroundService(intent)
        } else {
            startService(intent)
        }
    }

    private fun setupListeners() {
        btnEnableAccessibility.setOnClickListener {
            val intent = Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)
            startActivity(intent)
            Toast.makeText(this, "Find 'Klip' in downloaded services and turn it ON", Toast.LENGTH_LONG).show()
        }

        btnPair.setOnClickListener {
            showPairSelectionDialog()
        }

        btnSimulatePaste.setOnClickListener {
            // Test injecting a clip via the dual-mode mechanism
            val sampleClip = "https://github.com/Tribal-Chief-001/Klip (Synced from Klip companion)"
            val intent = Intent(this, KlipSyncService::class.java).apply {
                action = KlipSyncService.ACTION_INJECT_CLIP
                putExtra(KlipSyncService.EXTRA_CLIP_TEXT, sampleClip)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(intent)
            } else {
                startService(intent)
            }
            Toast.makeText(this, "Simulated incoming clip processed!", Toast.LENGTH_SHORT).show()
        }
    }

    private fun updateAccessibilityUI() {
        val isEnabled = KlipAccessibilityService.isServiceRunning()
        if (isEnabled) {
            tvAccessibilityNotice.text = "🟢 Active (Scoped strictly to Klip • Zero screen reading/keylogging • RASP-safe)"
            btnEnableAccessibility.text = "⚙️ Configure Accessibility"
        } else {
            tvAccessibilityNotice.text = "⚪ Inactive (UPI & Banking Safe Mode fully active via Notifications & QS Tile)"
            btnEnableAccessibility.text = "⚡ Enable Silent Auto-Sync"
        }
    }

    private fun updateStatusUI() {
        val prefs = getSharedPreferences(KlipSyncService.PREFS_NAME, Context.MODE_PRIVATE)
        val peerNodeHex = prefs.getString(KlipSyncService.PREF_PEER_NODE_HEX, null)
        val endpoints = prefs.getString(KlipSyncService.PREF_ENDPOINTS, null)

        if (!peerNodeHex.isNullOrEmpty()) {
            val shortId = peerNodeHex.take(8)
            val epInfo = if (!endpoints.isNullOrEmpty()) " ($endpoints)" else ""
            tvStatus.text = "● Paired with Node: $shortId$epInfo"
            tvStatus.setTextColor(android.graphics.Color.parseColor("#10B981"))
        } else {
            tvStatus.text = "● Mesh Status: Waiting for pairing..."
            tvStatus.setTextColor(android.graphics.Color.parseColor("#94A3B8"))
        }
    }

    private fun showPairSelectionDialog() {
        val options = arrayOf("📷 Scan QR Code with Camera", "⌨️ Enter Pairing Token Manually", "🌐 Enter PC IP:Port Manually")
        AlertDialog.Builder(this)
            .setTitle("Link New Device")
            .setItems(options) { _, which ->
                when (which) {
                    0 -> launchQrScanner()
                    1 -> showManualTokenDialog()
                    2 -> showManualIpDialog()
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun launchQrScanner() {
        val intent = Intent(this, QrScannerActivity::class.java)
        qrScanLauncher.launch(intent)
    }

    private fun showManualTokenDialog() {
        val input = EditText(this).apply {
            hint = "Paste token or JSON from 'klip pair'"
            setSingleLine(false)
            maxLines = 6
        }

        AlertDialog.Builder(this)
            .setTitle("Enter Pairing Token")
            .setMessage("Paste the hex token or JSON generated by 'klip pair' on your computer:")
            .setView(input)
            .setPositiveButton("Link") { _, _ ->
                val token = input.text.toString().trim()
                if (token.isNotEmpty()) {
                    applyPairingToken(token)
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun showManualIpDialog() {
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(50, 20, 50, 10)
        }
        val etIp = EditText(this).apply { hint = "Host IP: e.g. 192.168.1.100" }
        val etPort = EditText(this).apply {
            hint = "Port: 7123"
            setText("7123")
        }
        val etPsk = EditText(this).apply { hint = "32-byte hex PSK (from klip pair)" }

        layout.addView(etIp)
        layout.addView(etPort)
        layout.addView(etPsk)

        AlertDialog.Builder(this)
            .setTitle("Direct IP Connection")
            .setView(layout)
            .setPositiveButton("Connect") { _, _ ->
                val ip = etIp.text.toString().trim()
                val port = etPort.text.toString().trim()
                val psk = etPsk.text.toString().trim()
                if (ip.isNotEmpty() && psk.length == 64) {
                    val synthJson = org.json.JSONObject().apply {
                        put("host_node", org.json.JSONArray((1..32).map { 0 }))
                        put("psk_hex", psk)
                        put("timestamp", System.currentTimeMillis() / 1000)
                        put("endpoints", org.json.JSONArray(listOf("$ip:$port")))
                    }.toString()
                    applyPairingToken(synthJson)
                } else {
                    Toast.makeText(this, "Please provide valid IP and 64-char hex PSK", Toast.LENGTH_LONG).show()
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }

    private fun applyPairingToken(token: String) {
        try {
            val invitation = KlipProtocol.parsePairingToken(token)
            val intent = Intent(this, KlipSyncService::class.java).apply {
                action = KlipSyncService.ACTION_PAIR
                putExtra(KlipSyncService.EXTRA_PAIR_TOKEN, token)
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                startForegroundService(intent)
            } else {
                startService(intent)
            }

            Toast.makeText(this, "Pairing initiated with ${invitation.endpoints}", Toast.LENGTH_LONG).show()
            updateStatusUI()
        } catch (e: Exception) {
            Toast.makeText(this, "Invalid pairing token: ${e.message}", Toast.LENGTH_LONG).show()
        }
    }
}
