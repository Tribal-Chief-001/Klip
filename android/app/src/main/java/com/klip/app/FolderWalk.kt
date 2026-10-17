package com.klip.app

/** Turns a document tree (listed one folder at a time) into the flat list of files a folder send needs. Pure. */
object FolderWalk {
    const val MAX_FILES = 1000
    const val MAX_DEPTH = 16

    /** One entry of a folder as the document provider lists it. */
    data class Child(
        val id: String,
        val name: String,
        val isDir: Boolean,
        /** Bytes, or -1 when the provider does not know. */
        val size: Long,
        /** A document that has no file behind it (a cloud document): it cannot be read as bytes. */
        val virtual: Boolean = false,
    )

    data class Planned(val rel: List<String>, val name: String, val size: Long, val id: String)

    /** `error` is set when the tree cannot be sent at all (too many files, too deep, nothing in it). */
    data class Plan(val files: List<Planned>, val skipped: Int, val error: String?)

    fun plan(rootId: String, list: (String) -> List<Child>): Plan {
        val files = ArrayList<Planned>()
        var skipped = 0
        val seen = HashSet<String>().apply { add(rootId) }
        var error: String? = null

        fun visit(id: String, rel: List<String>) {
            if (error != null) return
            for (child in list(id).sortedBy { it.name.lowercase() }) {
                if (error != null) return
                when {
                    child.isDir -> {
                        if (!seen.add(child.id)) { skipped++; continue }
                        if (rel.size + 1 > MAX_DEPTH) { error = "the folder is deeper than $MAX_DEPTH levels"; return }
                        visit(child.id, rel + FileText.safeName(child.name))
                    }
                    child.virtual || child.size < 0 -> skipped++
                    else -> {
                        if (files.size >= MAX_FILES) { error = "the folder has more than $MAX_FILES files"; return }
                        files += Planned(rel, FileText.safeName(child.name), child.size, child.id)
                    }
                }
            }
        }
        visit(rootId, emptyList())
        if (error == null && files.isEmpty()) error = "the folder has no files to send"
        return Plan(if (error == null) files else emptyList(), skipped, error)
    }
}
