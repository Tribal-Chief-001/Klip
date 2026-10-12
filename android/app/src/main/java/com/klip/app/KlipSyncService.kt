package com.klip.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.concurrent.atomic.AtomicLong

class KlipSyncService : Service() {

    companion object {
        const val CHANNEL_ID = "klip_sync_channel"
        const val NOTIFICATION_ID = 40401
        private const val TAG = "KlipSyncService"

        const val ACTION_START = "com.klip.app.ACTION_START"
        const val ACTION_STOP = "com.klip.app.ACTION_STOP"
        const val ACTION_INJECT_CLIP = "com.klip.app.ACTION_INJECT_CLIP"
        const val ACTION_PAIR = "com.klip.app.ACTION_PAIR"
        const val ACTION_BROADCAST_CLIP = "com.klip.app.ACTION_BROADCAST_CLIP"

        const val CLIP_NOTIFICATION_CHANNEL_ID = "klip_clip_channel"
        const val CLIP_NOTIFICATION_ID = 40402

        const val EXTRA_CLIP_TEXT = "extra_clip_text"
        const val EXTRA_IS_SENSITIVE = "extra_is_sensitive"
        const val EXTRA_PAIR_TOKEN = "extra_pair_token"

        const val PREFS_NAME = "klip_prefs"
        const val PREF_PEER_NODE_HEX = "pref_peer_node_hex"
        const val PREF_PSK_HEX = "pref_psk_hex"
        const val PREF_ENDPOINTS = "pref_endpoints"
        const val PREF_DEVICE_NODE_HEX = "pref_device_node_hex"

        const val DEFAULT_PORT = 7123
    }

    private val serviceScope = CoroutineScope(Dispatchers.IO + SupervisorJob())
    private var lastLocalFingerprint: String = ""
    private var lastRemoteFingerprint: String = ""

    private var socket: DatagramSocket? = null
    private val nonceCounter = AtomicLong(1L)

    private var peerNodeId: ByteArray? = null
    private var peerKey: ByteArray? = null
    private var peerEndpoints: List<String> = emptyList()
    private var activePeerAddress: Pair<InetAddress, Int>? = null
    private val localNodeId: ByteArray = ByteArray(32)


    override fun onCreate() {
        super.onCreate()
        initLocalNodeId()
        loadSavedCredentials()
        createNotificationChannel()
        startForeground(NOTIFICATION_ID, buildForegroundNotification("Connecting to Klip Mesh..."))
        setupLocalClipboardWatcher()
        startNetworkEngine()
    }

    private fun initLocalNodeId() {
        val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val savedHex = prefs.getString(PREF_DEVICE_NODE_HEX, null)
        if (savedHex != null && savedHex.length == 64) {
            val bytes = KlipProtocol.hexToBytes(savedHex)
            System.arraycopy(bytes, 0, localNodeId, 0, 32)
        } else {
            java.security.SecureRandom().nextBytes(localNodeId)
            prefs.edit().putString(PREF_DEVICE_NODE_HEX, KlipProtocol.bytesToHex(localNodeId)).apply()
        }
    }

    private fun loadSavedCredentials() {
        val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val pskHex = prefs.getString(PREF_PSK_HEX, null)
        val peerNodeHex = prefs.getString(PREF_PEER_NODE_HEX, null)
        val epsString = prefs.getString(PREF_ENDPOINTS, null)

        if (!pskHex.isNullOrEmpty() && !peerNodeHex.isNullOrEmpty()) {
            peerKey = KlipProtocol.hexToBytes(pskHex)
            peerNodeId = KlipProtocol.hexToBytes(peerNodeHex)
            peerEndpoints = epsString?.split(",")?.filter { it.isNotBlank() } ?: emptyList()
            Log.i(TAG, "Loaded saved pairing: peer=${peerNodeHex.take(12)} endpoints=$peerEndpoints")
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> {
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
                return START_NOT_STICKY
            }
            ACTION_PAIR -> {
                val token = intent.getStringExtra(EXTRA_PAIR_TOKEN) ?: ""
                if (token.isNotEmpty()) {
                    handlePairToken(token)
                }
            }
            ACTION_INJECT_CLIP -> {
                val text = intent.getStringExtra(EXTRA_CLIP_TEXT) ?: ""
                val isSensitive = intent.getBooleanExtra(EXTRA_IS_SENSITIVE, false)
                if (text.isNotEmpty()) {
                    handleIncomingClip(text, isSensitive)
                }
            }
            ACTION_BROADCAST_CLIP -> {
                val text = intent.getStringExtra(EXTRA_CLIP_TEXT) ?: ""
                if (text.isNotEmpty()) {
                    lastLocalFingerprint = text
                    broadcastLocalClip(text)
                    updateNotification("Broadcasting clip (${text.length} chars)")
                }
            }
            else -> {
                if (peerKey != null) {
                    updateNotification("Connected • P2P Mesh Active")
                } else {
                    updateNotification("Waiting for device pairing...")
                }
            }
        }
        return START_STICKY
    }

    private fun handlePairToken(token: String) {
        try {
            val invitation = KlipProtocol.parsePairingToken(token)
            peerNodeId = invitation.hostNodeId
            peerKey = invitation.psk
            peerEndpoints = invitation.endpoints

            val prefs = getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
            prefs.edit()
                .putString(PREF_PEER_NODE_HEX, KlipProtocol.bytesToHex(invitation.hostNodeId))
                .putString(PREF_PSK_HEX, KlipProtocol.bytesToHex(invitation.psk))
                .putString(PREF_ENDPOINTS, invitation.endpoints.joinToString(","))
                .apply()

            Log.i(TAG, "Pairing token applied. Sending handshake to ${invitation.endpoints}")
            updateNotification("Pairing configured • Sending handshake")

            serviceScope.launch {
                sendHandshake()
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to parse pairing token: ${e.message}", e)
        }
    }

    private fun startNetworkEngine() {
        serviceScope.launch {
            try {
                // Bind UDP socket
                socket = DatagramSocket(null).apply {
                    reuseAddress = true
                    bind(InetSocketAddress(DEFAULT_PORT))
                    broadcast = true
                }
                Log.i(TAG, "UDP socket bound on port $DEFAULT_PORT")
            } catch (e: Exception) {
                Log.w(TAG, "Port $DEFAULT_PORT in use, binding ephemeral socket: ${e.message}")
                socket = DatagramSocket().apply { broadcast = true }
            }

            // If paired, send initial handshake / heartbeat
            if (peerKey != null) {
                sendHandshake()
            }

            // Inbound packet receiver loop
            val receiveBuf = ByteArray(65535)
            val packet = DatagramPacket(receiveBuf, receiveBuf.size)

            while (isActive) {
                try {
                    val s = socket ?: break
                    s.receive(packet)
                    val data = packet.data.copyOfRange(packet.offset, packet.offset + packet.length)
                    val remoteAddr = packet.address
                    val remotePort = packet.port

                    handleInboundPacket(data, remoteAddr, remotePort)
                } catch (e: Exception) {
                    if (!isActive) break
                    Log.e(TAG, "UDP receive loop error: ${e.message}")
                }
            }
        }
    }

    private fun handleInboundPacket(data: ByteArray, remoteAddr: InetAddress, remotePort: Int) {
        try {
            val wirePacket = KlipProtocol.WirePacket.decode(data)
            val header = wirePacket.header

            // Heartbeat handling
            if (header.msgType == KlipProtocol.MSG_HEARTBEAT_PING) {
                sendHeartbeatPong(remoteAddr, remotePort)
                return
            }
            if (header.msgType == KlipProtocol.MSG_HEARTBEAT_PONG) {
                return
            }

            val key = peerKey ?: run {
                Log.w(TAG, "Inbound packet dropped: unauthenticated/unpaired")
                return
            }

            // ChaCha20-Poly1305 AEAD decrypt
            val headerBytes = header.toBytes()
            val decryptedBytes = KlipProtocol.decryptWithAad(
                key = key,
                nonce = header.nonce,
                aad = headerBytes,
                ciphertext = wirePacket.payload,
                tag = wirePacket.tag
            )

            when (header.msgType) {
                KlipProtocol.MSG_HANDSHAKE -> {
                    activePeerAddress = Pair(remoteAddr, remotePort)
                    val peerName = String(decryptedBytes, Charsets.UTF_8)
                    Log.i(TAG, "Handshake received from: $peerName at $remoteAddr:$remotePort")
                    sendHandshakeAck(remoteAddr, remotePort)
                    updateNotification("Connected to $peerName (P2P)")
                }
                KlipProtocol.MSG_HANDSHAKE_ACK -> {
                    activePeerAddress = Pair(remoteAddr, remotePort)
                    val peerName = String(decryptedBytes, Charsets.UTF_8)
                    Log.i(TAG, "HandshakeAck received from: $peerName at $remoteAddr:$remotePort")
                    updateNotification("Linked with $peerName (P2P)")
                }
                KlipProtocol.MSG_CLIP_FULL_PAYLOAD -> {
                    val clipText = String(decryptedBytes, Charsets.UTF_8)
                    if (clipText.isNotEmpty() && clipText != lastRemoteFingerprint) {
                        lastRemoteFingerprint = clipText
                        lastLocalFingerprint = clipText
                        val isSensitive = (header.flags.toInt() and KlipProtocol.FLAG_SENSITIVE.toInt()) != 0
                        handleIncomingClip(clipText, isSensitive)
                    }
                }
                else -> {
                    Log.d(TAG, "Unhandled message type: ${header.msgType}")
                }
            }
        } catch (e: Exception) {
            Log.e(TAG, "Failed to process inbound packet: ${e.message}", e)
        }
    }

    private fun sendHandshake() {
        val key = peerKey ?: return
        val deviceName = "Android (${Build.MODEL})"
        val payload = deviceName.toByteArray(Charsets.UTF_8)
        val nonce = KlipProtocol.makeNonce(nonceCounter.getAndIncrement())

        val header = KlipProtocol.WireHeader(
            magic = KlipProtocol.MAGIC_BYTES,
            version = KlipProtocol.CURRENT_VERSION,
            msgType = KlipProtocol.MSG_HANDSHAKE,
            flags = KlipProtocol.FLAG_ENCRYPTED,
            senderNodeId = localNodeId,
            nonce = nonce,
            hlcPhysical = System.currentTimeMillis(),
            hlcLogical = 0,
            hlcNodeSeed = 0,
            payloadLen = payload.size
        )

        val (ciphertext, tag) = KlipProtocol.encryptWithAad(
            key = key,
            nonce = nonce,
            aad = header.toBytes(),
            plaintext = payload
        )

        val wirePacket = KlipProtocol.WirePacket(header, ciphertext, tag)
        sendToPeers(wirePacket.encode())
    }

    private fun sendHandshakeAck(addr: InetAddress, port: Int) {
        val key = peerKey ?: return
        val deviceName = "Android (${Build.MODEL})"
        val payload = deviceName.toByteArray(Charsets.UTF_8)
        val nonce = KlipProtocol.makeNonce(nonceCounter.getAndIncrement())

        val header = KlipProtocol.WireHeader(
            magic = KlipProtocol.MAGIC_BYTES,
            version = KlipProtocol.CURRENT_VERSION,
            msgType = KlipProtocol.MSG_HANDSHAKE_ACK,
            flags = KlipProtocol.FLAG_ENCRYPTED,
            senderNodeId = localNodeId,
            nonce = nonce,
            hlcPhysical = System.currentTimeMillis(),
            hlcLogical = 0,
            hlcNodeSeed = 0,
            payloadLen = payload.size
        )

        val (ciphertext, tag) = KlipProtocol.encryptWithAad(
            key = key,
            nonce = nonce,
            aad = header.toBytes(),
            plaintext = payload
        )

        val wirePacket = KlipProtocol.WirePacket(header, ciphertext, tag)
        sendDirect(wirePacket.encode(), addr, port)
    }

    private fun sendHeartbeatPong(addr: InetAddress, port: Int) {
        val header = KlipProtocol.WireHeader(
            magic = KlipProtocol.MAGIC_BYTES,
            version = KlipProtocol.CURRENT_VERSION,
            msgType = KlipProtocol.MSG_HEARTBEAT_PONG,
            flags = 0,
            senderNodeId = localNodeId,
            nonce = KlipProtocol.makeNonce(0),
            hlcPhysical = 0,
            hlcLogical = 0,
            hlcNodeSeed = 0,
            payloadLen = 0
        )
        val wirePacket = KlipProtocol.WirePacket(header, ByteArray(0), ByteArray(KlipProtocol.TAG_LEN))
        sendDirect(wirePacket.encode(), addr, port)
    }

    private fun sendToPeers(data: ByteArray) {
        serviceScope.launch {
            val s = socket ?: return@launch
            // 1. Send to dynamically discovered peer address
            activePeerAddress?.let { (addr, port) ->
                try {
                    s.send(DatagramPacket(data, data.size, addr, port))
                } catch (e: Exception) {
                    Log.w(TAG, "Failed sending to active peer $addr:$port: ${e.message}")
                }
            }

            // 2. Send to all known endpoints from pairing invitation
            for (endpoint in peerEndpoints) {
                try {
                    val parts = endpoint.split(":")
                    if (parts.size == 2) {
                        val host = parts[0]
                        val port = parts[1].toInt()
                        val inetAddr = InetAddress.getByName(host)
                        val packet = DatagramPacket(data, data.size, inetAddr, port)
                        s.send(packet)
                    }
                } catch (e: Exception) {
                    Log.w(TAG, "Failed sending to endpoint $endpoint: ${e.message}")
                }
            }

            // 2. Broadcast to LAN and localhost
            try {
                val bcastAddr = InetAddress.getByName("255.255.255.255")
                s.send(DatagramPacket(data, data.size, bcastAddr, DEFAULT_PORT))
            } catch (_: Exception) {}

            try {
                val loopback = InetAddress.getByName("127.0.0.1")
                s.send(DatagramPacket(data, data.size, loopback, DEFAULT_PORT))
            } catch (_: Exception) {}
        }
    }

    private fun sendDirect(data: ByteArray, addr: InetAddress, port: Int) {
        serviceScope.launch {
            try {
                val s = socket ?: return@launch
                val packet = DatagramPacket(data, data.size, addr, port)
                s.send(packet)
            } catch (e: Exception) {
                Log.w(TAG, "Failed sending direct packet: ${e.message}")
            }
        }
    }

    private fun setupLocalClipboardWatcher() {
        val clipboard = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
        clipboard.addPrimaryClipChangedListener {
            val clip = clipboard.primaryClip
            if (clip != null && clip.itemCount > 0) {
                val text = clip.getItemAt(0).text?.toString() ?: ""
                if (text.isNotEmpty() && text != lastLocalFingerprint) {
                    lastLocalFingerprint = text
                    Log.i(TAG, "Local Android clip detected (${text.length} chars). Broadcasting to peers...")
                    broadcastLocalClip(text)
                }
            }
        }
    }

    private fun broadcastLocalClip(text: String) {
        val key = peerKey ?: return
        serviceScope.launch {
            try {
                val payload = text.toByteArray(Charsets.UTF_8)
                val nonce = KlipProtocol.makeNonce(nonceCounter.getAndIncrement())

                val header = KlipProtocol.WireHeader(
                    magic = KlipProtocol.MAGIC_BYTES,
                    version = KlipProtocol.CURRENT_VERSION,
                    msgType = KlipProtocol.MSG_CLIP_FULL_PAYLOAD,
                    flags = KlipProtocol.FLAG_ENCRYPTED,
                    senderNodeId = localNodeId,
                    nonce = nonce,
                    hlcPhysical = System.currentTimeMillis(),
                    hlcLogical = 0,
                    hlcNodeSeed = 0,
                    payloadLen = payload.size
                )

                val (ciphertext, tag) = KlipProtocol.encryptWithAad(
                    key = key,
                    nonce = nonce,
                    aad = header.toBytes(),
                    plaintext = payload
                )

                val wirePacket = KlipProtocol.WirePacket(header, ciphertext, tag)
                sendToPeers(wirePacket.encode())
                Log.d(TAG, "Broadcasted local clip (${payload.size} B) to Klip mesh peers")
            } catch (e: Exception) {
                Log.e(TAG, "Failed to broadcast local clip: ${e.message}", e)
            }
        }
    }

    private fun handleIncomingClip(clipText: String, isSensitive: Boolean) {
        val preview = if (clipText.length > 30) clipText.take(30) + "…" else clipText
        val accessibilityActive = KlipAccessibilityService.isServiceRunning()

        if (accessibilityActive) {
            Log.i(TAG, "Silent auto-sync: injecting via scoped AccessibilityService (${clipText.length} chars)")
            KlipAccessibilityService.injectRemoteClip(this, clipText, isSensitive)
            updateNotification("Auto-synced: \"$preview\"")
        } else {
            Log.i(TAG, "Bank-Safe mode: showing interactive notification with Copy action (${clipText.length} chars)")
            updateNotification("Incoming clip: \"$preview\"")
        }

        // Show interactive heads-up notification with Copy Trampoline action button
        showIncomingClipNotification(clipText, isSensitive, accessibilityActive)
    }

    private fun showIncomingClipNotification(clipText: String, isSensitive: Boolean, accessibilityAlreadyInjected: Boolean) {
        val trampolineIntent = Intent(this, CopyTrampolineActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
            putExtra(CopyTrampolineActivity.EXTRA_CLIP_TEXT, clipText)
            putExtra(CopyTrampolineActivity.EXTRA_IS_SENSITIVE, isSensitive)
        }

        val pendingCopyIntent = PendingIntent.getActivity(
            this,
            System.currentTimeMillis().toInt(),
            trampolineIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        )

        val displayText = if (isSensitive) "•••••••• (Sensitive Content)" else clipText
        val snippet = if (displayText.length > 80) displayText.take(80) + "…" else displayText

        val title = if (accessibilityAlreadyInjected) "📋 Klip Synced to Clipboard" else "📎 Klip Received from PC"

        val notification = NotificationCompat.Builder(this, CLIP_NOTIFICATION_CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_menu_agenda)
            .setContentTitle(title)
            .setContentText(snippet)
            .setStyle(NotificationCompat.BigTextStyle().bigText(displayText))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setDefaults(NotificationCompat.DEFAULT_ALL)
            .setAutoCancel(true)
            .setContentIntent(pendingCopyIntent)
            .addAction(
                android.R.drawable.ic_menu_save,
                "📋 Copy to Clipboard",
                pendingCopyIntent
            )
            .build()

        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.notify(CLIP_NOTIFICATION_ID, notification)
    }

    private fun createNotificationChannel() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val manager = getSystemService(NotificationManager::class.java)

            val persistentChannel = NotificationChannel(
                CHANNEL_ID,
                "Klip Engine Service",
                NotificationManager.IMPORTANCE_LOW
            ).apply {
                description = "Maintains persistent E2EE mesh clipboard synchronization"
                setShowBadge(false)
            }
            manager.createNotificationChannel(persistentChannel)

            val clipChannel = NotificationChannel(
                CLIP_NOTIFICATION_CHANNEL_ID,
                "Incoming Clip Notifications",
                NotificationManager.IMPORTANCE_HIGH
            ).apply {
                description = "Heads-up notification for incoming clips with 1-tap Copy action (UPI safe)"
                enableVibration(true)
                setShowBadge(true)
            }
            manager.createNotificationChannel(clipChannel)
        }
    }

    private fun buildForegroundNotification(statusText: String): Notification {
        val pendingIntent = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle("📎 Klip Engine")
            .setContentText(statusText)
            .setSmallIcon(android.R.drawable.ic_menu_agenda)
            .setContentIntent(pendingIntent)
            .setOngoing(true)
            .setPriority(NotificationCompat.PRIORITY_LOW)
            .build()
    }

    private fun updateNotification(statusText: String) {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.notify(NOTIFICATION_ID, buildForegroundNotification(statusText))
    }

    override fun onDestroy() {
        super.onDestroy()
        serviceScope.cancel()
        try {
            socket?.close()
        } catch (_: Exception) {}
    }

    override fun onBind(intent: Intent?): IBinder? = null
}
