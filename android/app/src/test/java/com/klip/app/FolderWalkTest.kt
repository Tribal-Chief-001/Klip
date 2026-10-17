package com.klip.app

import com.klip.app.FolderWalk.Child
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FolderWalkTest {
    /** A fake document tree: id -> children. */
    private fun tree(vararg entries: Pair<String, List<Child>>): (String) -> List<Child> {
        val map = entries.toMap()
        return { id -> map[id] ?: emptyList() }
    }

    private fun file(id: String, name: String, size: Long = 10) = Child(id, name, isDir = false, size = size)
    private fun dir(id: String, name: String) = Child(id, name, isDir = true, size = 0)

    @Test fun a_tree_becomes_files_with_their_folders_in_a_stable_order() {
        val list = tree(
            "root" to listOf(file("b", "b.txt", 5), dir("d1", "Day 1"), file("a", "a.txt", 7)),
            "d1" to listOf(dir("d2", "raw"), file("c", "c.jpg", 100)),
            "d2" to listOf(file("e", "e.cr2", 1000)),
        )
        val plan = FolderWalk.plan("root", list)
        assertNull(plan.error)
        assertEquals(
            listOf(
                Triple(emptyList<String>(), "a.txt", 7L),
                Triple(emptyList<String>(), "b.txt", 5L),
                Triple(listOf("Day 1"), "c.jpg", 100L),
                Triple(listOf("Day 1", "raw"), "e.cr2", 1000L),
            ),
            plan.files.map { Triple(it.rel, it.name, it.size) }
        )
        assertEquals(listOf("a", "b", "c", "e"), plan.files.map { it.id })
    }

    @Test fun virtual_and_unknown_size_documents_are_skipped_and_counted() {
        val list = tree(
            "root" to listOf(
                file("a", "a.txt"),
                Child("v", "Google Doc", isDir = false, size = -1, virtual = true),
                Child("u", "mystery", isDir = false, size = -1),
            )
        )
        val plan = FolderWalk.plan("root", list)
        assertEquals(listOf("a"), plan.files.map { it.id })
        assertEquals(2, plan.skipped)
    }

    @Test fun names_are_cleaned_like_everywhere_else() {
        val list = tree("root" to listOf(dir("d", "../escape"), file("f", "..\\evil.sh")), "d" to listOf(file("g", "a:b.txt")))
        val plan = FolderWalk.plan("root", list)
        assertNull(plan.error)
        assertTrue(plan.files.none { it.name.contains('/') || it.name.contains('\\') || it.rel.any { r -> r.contains("..") } })
        assertEquals("evil.sh", plan.files.first { it.id == "f" }.name)
        assertEquals(listOf("escape"), plan.files.first { it.id == "g" }.rel)
    }

    @Test fun more_than_a_thousand_files_is_refused_with_a_reason() {
        val many = (0..1000).map { file("f$it", "f$it") }
        val plan = FolderWalk.plan("root", tree("root" to many))
        assertNotNull(plan.error)
        assertTrue(plan.error!!.contains("1000"))
        val ok = FolderWalk.plan("root", tree("root" to many.take(1000)))
        assertNull(ok.error)
        assertEquals(1000, ok.files.size)
    }

    @Test fun seventeen_levels_is_too_deep_and_sixteen_is_fine() {
        fun chain(levels: Int): (String) -> List<Child> {
            val entries = (0 until levels).map { i -> "d$i" to listOf(if (i == levels - 1) file("leaf", "x") else dir("d${i + 1}", "d${i + 1}")) }
            return tree("root" to listOf(dir("d0", "d0")), *entries.toTypedArray())
        }
        assertNull(FolderWalk.plan("root", chain(16)).error)
        assertTrue(FolderWalk.plan("root", chain(17)).error!!.contains("16"))
    }

    @Test fun a_tree_with_thousands_of_empty_folders_is_refused_before_it_costs_minutes() {
        val dirs = (0..FolderWalk.MAX_DIRS).map { dir("d$it", "d$it") }
        val plan = FolderWalk.plan("root", tree("root" to dirs + file("f", "f")))
        assertNotNull(plan.error)
        assertTrue(plan.error!!.contains("${FolderWalk.MAX_DIRS}"))
    }

    @Test fun an_empty_tree_and_a_cycle_do_not_hang_or_crash() {
        assertTrue(FolderWalk.plan("root", tree("root" to emptyList())).error!!.contains("no files"))
        val cyc = tree("root" to listOf(dir("a", "a"), file("f", "f")), "a" to listOf(dir("root", "again")))
        val plan = FolderWalk.plan("root", cyc)
        assertEquals(listOf("f"), plan.files.map { it.id })
    }
}
