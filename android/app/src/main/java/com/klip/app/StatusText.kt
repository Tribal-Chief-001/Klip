package com.klip.app

/**
 * One place that turns the engine's status into words, used by the always-on notification and the
 * Quick Settings tile. Only metadata (device names, times): never clip content, because notifications
 * can show on a locked screen.
 */
object StatusText {
    class Summary(
        val title: String,
        /** Title for the collapsed notification, where the Send button takes room: no device names. */
        val shortTitle: String,
        /** Short line shown when the notification is collapsed. */
        val line: String,
        /** Full text shown when it is expanded. */
        val body: String,
        val anyOnline: Boolean,
        val firstOnline: String?
    ) {
        /** Equal summaries need no notification update. */
        val key get() = "$title|$body"
    }

    /** Blocking (engine calls): call from a background thread. */
    fun compute(): Summary {
        val engine = KlipRuntime.engine
            ?: return Summary("Klip", "Klip", "Starting…", "Starting…", false, null)
        val st = runCatching { engine.status() }.getOrNull()
            ?: return Summary("Klip", "Klip", "Not responding", "Not responding. Open Klip.", false, null)
        val history = runCatching { engine.history(1u) }.getOrDefault(emptyList()).firstOrNull()

        val online = st.peers.filter { it.online }
        val title = when {
            st.paused -> "Klip · Paused"
            st.peers.isEmpty() -> "Klip · No devices linked"
            online.isNotEmpty() -> "Klip · Connected to " + names(online.map { it.name })
            else -> "Klip · Waiting for " + names(st.peers.map { it.name })
        }

        val shortTitle = when {
            st.paused -> "Klip · Paused"
            st.peers.isEmpty() -> "Klip · No devices"
            online.isNotEmpty() -> "Klip · Connected"
            else -> "Klip · Waiting"
        }

        val lines = mutableListOf<String>()
        if (st.peers.isEmpty()) {
            lines += "Open Klip and tap Add a device."
        } else {
            lines += st.peers.joinToString("   ") { (if (it.online) "● " else "○ ") + it.name }
        }
        lines += if (history != null) {
            val whenText = Fmt.agoMs(history.timestampMs.toLong())
            val me = KlipRuntime.deviceName()
            if (history.fromDevice.isBlank() || history.fromDevice == me) "Last clip: sent from this phone · $whenText"
            else "Last clip: received from ${history.fromDevice} · $whenText"
        } else "No clips yet"
        if (online.isEmpty() && st.peers.isNotEmpty()) {
            st.peers.firstNotNullOfOrNull { it.lastError }?.let { lines += Fmt.plainPeerError(it) }
        }
        return Summary(title, shortTitle, lines.first(), lines.joinToString("\n"), online.isNotEmpty(), online.firstOrNull()?.name)
    }

    private fun names(list: List<String>): String = when {
        list.size <= 2 -> list.joinToString(" and ")
        else -> "${list[0]} and ${list.size - 1} more"
    }
}
