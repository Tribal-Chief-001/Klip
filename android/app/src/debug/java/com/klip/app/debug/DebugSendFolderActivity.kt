package com.klip.app.debug

import android.app.Activity
import android.os.Bundle
import android.provider.DocumentsContract
import com.klip.app.FolderSender

/** Debug only: `adb shell am start -n com.klip.app/.debug.DebugSendFolderActivity` sends the debug document tree as a folder. */
class DebugSendFolderActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val tree = DocumentsContract.buildTreeDocumentUri("$packageName.debugtree", "root")
        FolderSender.start(this, tree)
        finish()
    }
}
