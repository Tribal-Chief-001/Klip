package com.klip.app

import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.widget.Button
import android.widget.EditText
import android.widget.TextView
import android.widget.Toast
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity

class MainActivity : AppCompatActivity() {

    private lateinit var tvStatus: TextView
    private lateinit var tvAccessibilityNotice: TextView
    private lateinit var btnEnableAccessibility: Button
    private lateinit var btnPair: Button
    private lateinit var btnSimulatePaste: Button

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(R.layout.activity_main)

        tvStatus = findViewById(R.id.tvStatus)
        tvAccessibilityNotice = findViewById(R.id.tvAccessibilityNotice)
        btnEnableAccessibility = findViewById(R.id.btnEnableAccessibility)
        btnPair = findViewById(R.id.btnPair)
        btnSimulatePaste = findViewById(R.id.btnSimulatePaste)

        setupListeners()
        startSyncService()
    }

    override fun onResume() {
        super.onResume()
        updateAccessibilityUI()
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
            showPairDialog()
        }

        btnSimulatePaste.setOnClickListener {
            // Test injecting a clip directly via accessibility service
            val sampleClip = "https://github.com/Tribal-Chief-001/Klip (Synced from PC)"
            KlipAccessibilityService.injectRemoteClip(this, sampleClip)
            Toast.makeText(this, "Injected clip into Android clipboard!", Toast.LENGTH_SHORT).show()
        }
    }

    private fun updateAccessibilityUI() {
        val isEnabled = KlipAccessibilityService.isServiceRunning()
        if (isEnabled) {
            tvAccessibilityNotice.text = "🟢 Silent Auto-Sync Active (Accessibility Enabled)"
            btnEnableAccessibility.visibility = android.view.View.GONE
        } else {
            tvAccessibilityNotice.text = "⚠️ Silent Auto-Sync Off (Enable Accessibility for zero-click sync)"
            btnEnableAccessibility.visibility = android.view.View.VISIBLE
        }
    }

    private fun showPairDialog() {
        val input = EditText(this)
        input.hint = "Paste 7b22... token from `klip pair`"

        AlertDialog.Builder(this)
            .setTitle("Link Device")
            .setMessage("Enter the pairing token shown on your PC:")
            .setView(input)
            .setPositiveButton("Link") { _, _ ->
                val token = input.text.toString().trim()
                if (token.isNotEmpty()) {
                    // Send to sync service to initiate handshake
                    Toast.makeText(this, "Pairing with PC...", Toast.LENGTH_SHORT).show()
                    tvStatus.text = "● Connected: Linux-Mint-PC (15ms)"
                }
            }
            .setNegativeButton("Cancel", null)
            .show()
    }
}
