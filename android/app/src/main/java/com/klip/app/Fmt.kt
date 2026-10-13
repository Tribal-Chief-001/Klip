package com.klip.app

/** Small text helpers: relative times and plain-language versions of engine messages. */
object Fmt {
    fun ago(secs: Long): String = when {
        secs < 5 -> "just now"
        secs < 60 -> "${secs}s ago"
        secs < 3600 -> "${secs / 60} min ago"
        secs < 86400 -> "${secs / 3600} h ago"
        else -> "${secs / 86400} d ago"
    }

    fun agoMs(timestampMs: Long): String =
        ago(((System.currentTimeMillis() - timestampMs) / 1000).coerceAtLeast(0))

    /** Engine/network errors rewritten for a person. Unknown messages pass through unchanged. */
    fun plainPeerError(raw: String): String {
        val r = raw.lowercase()
        return when {
            "unpaired by the other device" in r ->
                "The other device removed this phone. Remove it here too, then link again."
            "paused" in r -> "That device has syncing paused."
            "clipboard" in r && "write" in r -> "That device couldn't write to its clipboard."
            "no addressing information" in r || "cannot reach" in r || "unreachable" in r ->
                "Can't find it right now. It reconnects by itself when it's back."
            "timed out" in r -> "It didn't answer in time."
            else -> raw.take(90)
        }
    }
}
