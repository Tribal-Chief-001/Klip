package com.klip.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FileTextTest {
    @Test fun sizes_are_written_in_the_unit_a_person_reads() {
        assertEquals("0 B", FileText.size(0))
        assertEquals("1023 B", FileText.size(1023))
        assertEquals("1.5 KiB", FileText.size(1536))
        assertEquals("3.0 MiB", FileText.size(3L * 1024 * 1024))
        assertEquals("1.5 GiB", FileText.size(3L * 1024 * 1024 * 1024 / 2))
    }

    @Test fun the_received_notice_has_the_device_and_size_but_never_a_name() {
        assertEquals("Received 1 file from Laptop (3.0 MiB)", FileText.received("Laptop", 3L * 1024 * 1024))
    }

    @Test fun progress_line_counts_files_and_percent() {
        val sending = FileText.Row(outgoing = true, size = 100, done = 45, running = true)
        assertEquals("Sending 1 file, 45%", FileText.progress(listOf(sending)))
        val two = listOf(sending, FileText.Row(true, 100, 45, true))
        assertEquals("Sending 2 files, 45%", FileText.progress(two))
        assertEquals("Receiving 1 file, 10%", FileText.progress(listOf(FileText.Row(false, 1000, 100, true))))
        assertEquals(
            "Sending 1 file, receiving 1 file, 30%",
            FileText.progress(listOf(FileText.Row(true, 100, 50, true), FileText.Row(false, 100, 10, true)))
        )
    }

    @Test fun progress_ignores_finished_rows_and_survives_zero_sizes() {
        assertEquals("", FileText.progress(emptyList()))
        assertEquals("", FileText.progress(listOf(FileText.Row(true, 100, 100, false))))
        assertEquals("Sending 1 file, 100%", FileText.progress(listOf(FileText.Row(true, 0, 0, true))))
    }

    @Test fun mime_follows_the_extension_and_falls_back_to_octet_stream() {
        assertEquals("image/jpeg", FileText.mimeFor("a.JPG"))
        assertEquals("application/pdf", FileText.mimeFor("report.pdf"))
        assertEquals("video/mp4", FileText.mimeFor("clip.mp4"))
        assertEquals("application/octet-stream", FileText.mimeFor("noextension"))
        assertEquals("application/octet-stream", FileText.mimeFor("weird.zzzz"))
    }

    @Test fun safe_names_never_hold_a_path_or_control_characters() {
        assertEquals("evil.sh", FileText.safeName("../../evil.sh"))
        assertEquals("b", FileText.safeName("a/b"))
        assertEquals("b", FileText.safeName("a\\b"))
        assertEquals("x", FileText.safeName("\u0000x"))
        assertEquals("file", FileText.safeName(""))
        assertEquals("file", FileText.safeName(null))
        assertEquals("file", FileText.safeName("///"))
        assertEquals(255, FileText.safeName("x".repeat(400)).length)
        assertEquals("photo 😀.jpg", FileText.safeName("photo 😀.jpg"))
    }

    @Test fun a_relative_folder_is_cleaned_component_by_component() {
        assertEquals("", FileText.safeRelDir(""))
        assertEquals("Trip/Day 1", FileText.safeRelDir("Trip/Day 1"))
        assertEquals("a/b", FileText.safeRelDir("../a/./b/.."))
        assertFalse(FileText.safeRelDir("/etc/../passwd").startsWith("/"))
        assertTrue(FileText.safeRelDir("a/".repeat(30)).split('/').size <= 16)
    }
}
