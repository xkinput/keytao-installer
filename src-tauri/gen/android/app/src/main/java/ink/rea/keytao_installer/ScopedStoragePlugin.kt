package ink.rea.keytao_installer

import android.app.Activity
import android.content.Intent
import android.net.Uri
import androidx.activity.result.ActivityResult
import androidx.documentfile.provider.DocumentFile
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONArray
import java.io.File
import java.util.zip.ZipInputStream

@TauriPlugin
class ScopedStoragePlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun openApp(invoke: Invoke) {
        val args = invoke.getArgs()
        val packageName = args.getString("packageName", null)
            ?: return invoke.reject("Missing packageName")
        val intent = activity.packageManager.getLaunchIntentForPackage(packageName)
        if (intent != null) {
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            activity.startActivity(intent)
            invoke.resolve()
        } else {
            invoke.reject("未检测到已安装的 $packageName")
        }
    }

    @Command
    fun pickDirectory(invoke: Invoke) {
        val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
        startActivityForResult(invoke, intent, "handleDirectoryPicked")
    }

    @ActivityCallback
    private fun handleDirectoryPicked(invoke: Invoke, result: ActivityResult) {
        if (result.resultCode == Activity.RESULT_OK) {
            val uri = result.data?.data ?: return invoke.reject("No URI returned")
            activity.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION
            )
            invoke.resolve(JSObject().apply { put("uri", uri.toString()) })
        } else {
            invoke.reject("User cancelled")
        }
    }

    @Command
    fun listFiles(invoke: Invoke) {
        val args = invoke.getArgs()
        val treeUriString = args.getString("treeUri", null)
            ?: return invoke.reject("Missing treeUri")

        try {
            val uri = Uri.parse(treeUriString)
            val root = DocumentFile.fromTreeUri(activity, uri)
                ?: return invoke.reject("Invalid directory URI")

            val items = root.listFiles()
                .map { Pair(it.name ?: "", it.isDirectory) }
                .sortedWith(compareByDescending<Pair<String, Boolean>> { it.second }.thenBy { it.first })

            val filesArray = JSONArray()
            items.forEach { (name, isDir) ->
                filesArray.put(JSObject().apply {
                    put("name", name)
                    put("isDir", isDir)
                })
            }

            invoke.resolve(JSObject().apply { put("files", filesArray) })
        } catch (ex: Exception) {
            invoke.reject(ex.message ?: "Failed to list files")
        }
    }

    @Command
    fun readLocalSchemas(invoke: Invoke) {
        val args = invoke.getArgs()
        val treeUriString = args.getString("treeUri", null)
            ?: return invoke.reject("Missing treeUri")

        try {
            val uri = Uri.parse(treeUriString)
            val root = DocumentFile.fromTreeUri(activity, uri)
                ?: return invoke.reject("Invalid directory URI")

            val content = readFileFromTree(root, "default.custom.yaml")
                ?: readFileFromTree(root, "default-custom.yaml")
                ?: ""

            val arr = JSONArray()
            parseSchemas(content).forEach { arr.put(it) }

            invoke.resolve(JSObject().apply { put("schemas", arr) })
        } catch (ex: Exception) {
            invoke.resolve(JSObject().apply { put("schemas", JSONArray()) })
        }
    }

    @Command
    fun smartExtractZip(invoke: Invoke) {
        val args = invoke.getArgs()
        val zipPath = args.getString("zipPath", null) ?: return invoke.reject("Missing zipPath")
        val treeUriString = args.getString("treeUri", null) ?: return invoke.reject("Missing treeUri")

        Thread {
            try {
                val uri = Uri.parse(treeUriString)
                val root = DocumentFile.fromTreeUri(activity, uri)
                    ?: return@Thread invoke.reject("Invalid directory URI")

                val zipFile = File(zipPath)
                val logs = mutableListOf<String>()

                // First pass: collect default.custom.yaml, rime.lua, and lua/ filenames
                var zipDefaultCustomPath: String? = null
                var zipDefaultCustomContent: String? = null
                var zipRimeLuaPath: String? = null
                var zipRimeLuaContent: String? = null
                val zipLuaFilenames = mutableSetOf<String>()

                ZipInputStream(zipFile.inputStream().buffered()).use { zis ->
                    var entry = zis.nextEntry
                    while (entry != null) {
                        val relative = entry.name.substringAfter('/').trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        when {
                            !entry.isDirectory && isDefaultCustom(filename) && zipDefaultCustomPath == null -> {
                                zipDefaultCustomPath = relative
                                zipDefaultCustomContent = zis.bufferedReader().readText()
                            }
                            !entry.isDirectory && filename == "rime.lua" && !relative.contains('/') && zipRimeLuaPath == null -> {
                                zipRimeLuaPath = relative
                                zipRimeLuaContent = zis.bufferedReader().readText()
                            }
                            !entry.isDirectory && relative.startsWith("lua/") && !relative.substring(4).contains('/') -> {
                                zipLuaFilenames.add(filename)
                            }
                        }
                        zis.closeEntry()
                        entry = zis.nextEntry
                    }
                }

                // Compute default.custom.yaml merge
                val dcMergeResult = zipDefaultCustomContent?.let {
                    val existing = readFileFromTree(root, "default.custom.yaml")
                        ?: readFileFromTree(root, "default-custom.yaml")
                    mergeDefaultCustom(existing, it)
                }

                // Compute rime.lua merge and save conflicting user lua files before zip overwrites them
                val rimeLuaMergeResult = zipRimeLuaContent?.let { zipRl ->
                    val localRl = readFileFromTree(root, "rime.lua")
                    if (localRl != null) mergeRimeLua(localRl, zipRl, zipLuaFilenames)
                    else RimeLuaMergeResult(zipRl, emptyList())
                }

                val renamedLuaFiles: List<Pair<String, ByteArray>> =
                    rimeLuaMergeResult?.renames?.mapNotNull { (oldName, newName) ->
                        val luaDir = root.findFile("lua") ?: return@mapNotNull null
                        val bytes = luaDir.findFile("$oldName.lua")?.uri?.let { fileUri ->
                            activity.contentResolver.openInputStream(fileUri)?.use { it.readBytes() }
                        } ?: return@mapNotNull null
                        newName to bytes
                    } ?: emptyList()

                // Second pass: full extraction
                ZipInputStream(zipFile.inputStream().buffered()).use { zis ->
                    var entry = zis.nextEntry
                    while (entry != null) {
                        val relative = entry.name.substringAfter('/').trimEnd('/')

                        if (relative.isNotEmpty()) {
                            val filename = relative.substringAfterLast('/')

                            when {
                                entry.isDirectory -> {
                                    try { ensureDir(root, relative) } catch (e: Exception) {
                                        logs.add("[WARN] mkdir $relative: ${e.message}")
                                    }
                                }
                                isDefaultCustom(filename) && dcMergeResult != null -> {
                                    writeFileBytes(root, relative, dcMergeResult.mergedContent.toByteArray(), logs, merged = true)
                                }
                                filename == "rime.lua" && rimeLuaMergeResult != null -> {
                                    writeFileBytes(root, relative, rimeLuaMergeResult.mergedContent.toByteArray(), logs, merged = true)
                                }
                                else -> {
                                    val dirPart = relative.substringBeforeLast('/', "")
                                    val dir = if (dirPart.isEmpty()) root else try {
                                        ensureDir(root, dirPart)
                                    } catch (e: Exception) {
                                        logs.add("[ERROR] mkdir for $relative: ${e.message}")
                                        return@Thread invoke.reject("Failed to create directory for: $relative")
                                    }
                                    val deleted = dir.findFile(filename)?.delete() ?: true
                                    if (!deleted) logs.add("[WARN] delete failed for $relative, will attempt overwrite")
                                    val newFile = dir.createFile("application/octet-stream", filename)
                                    if (newFile == null) {
                                        logs.add("[ERROR] createFile failed: $relative")
                                        return@Thread invoke.reject("Failed to create: $filename")
                                    }
                                    val written = activity.contentResolver.openOutputStream(newFile.uri)?.use { out ->
                                        zis.copyTo(out)
                                        true
                                    } ?: false
                                    if (written) {
                                        logs.add("[OK] $relative")
                                    } else {
                                        logs.add("[ERROR] openOutputStream returned null: $relative")
                                        return@Thread invoke.reject("Failed to open output stream: $relative")
                                    }
                                }
                            }
                        }

                        zis.closeEntry()
                        entry = zis.nextEntry
                    }
                }

                // Write renamed user lua files (read before zip overwrote them)
                if (renamedLuaFiles.isNotEmpty()) {
                    val luaDir = try { ensureDir(root, "lua") } catch (e: Exception) {
                        return@Thread invoke.reject("Failed to ensure lua dir: ${e.message}")
                    }
                    for ((newName, bytes) in renamedLuaFiles) {
                        val filename = "$newName.lua"
                        luaDir.findFile(filename)?.delete()
                        val newFile = luaDir.createFile("application/octet-stream", filename)
                        if (newFile == null) {
                            logs.add("[ERROR] createFile for renamed: lua/$filename")
                        } else {
                            val ok = activity.contentResolver.openOutputStream(newFile.uri)?.use { out ->
                                out.write(bytes); true
                            } ?: false
                            if (ok) logs.add("[RENAMED] lua/$filename")
                            else logs.add("[ERROR] openOutputStream for renamed: lua/$filename")
                        }
                    }
                }

                zipFile.delete()

                val mergedArray = JSONArray()
                dcMergeResult?.userSchemas?.forEach { mergedArray.put(it) }

                val logsArray = JSONArray()
                logs.forEach { logsArray.put(it) }

                invoke.resolve(JSObject().apply {
                    put("mergedSchemas", mergedArray)
                    put("logs", logsArray)
                })
            } catch (ex: Exception) {
                invoke.reject(ex.message ?: "Extraction failed")
            }
        }.start()
    }

    // ─── Helpers ────────────────────────────────────────────────────────────────

    private fun readFileFromTree(root: DocumentFile, filename: String): String? {
        return try {
            root.findFile(filename)?.uri?.let { uri ->
                activity.contentResolver.openInputStream(uri)?.use { it.bufferedReader().readText() }
            }
        } catch (e: Exception) {
            null
        }
    }

    private fun writeFileBytes(root: DocumentFile, relativePath: String, content: ByteArray, logs: MutableList<String>, merged: Boolean = false) {
        val dirPart = relativePath.substringBeforeLast('/', "")
        val filename = relativePath.substringAfterLast('/')
        val dir = if (dirPart.isEmpty()) root else ensureDir(root, dirPart)
        dir.findFile(filename)?.delete()
        val file = dir.createFile("application/octet-stream", filename)
            ?: throw Exception("Failed to create: $filename")
        val ok = activity.contentResolver.openOutputStream(file.uri)?.use {
            it.write(content); true
        } ?: false
        if (!ok) throw Exception("Failed to open output stream: $filename")
        logs.add(if (merged) "[MERGED] $relativePath" else "[OK] $relativePath")
    }

    private fun writeTextFile(root: DocumentFile, relativePath: String, content: String) {
        val dirPart = relativePath.substringBeforeLast('/', "")
        val filename = relativePath.substringAfterLast('/')
        val dir = if (dirPart.isEmpty()) root else ensureDir(root, dirPart)
        dir.findFile(filename)?.delete()
        val file = dir.createFile("application/octet-stream", filename) ?: return
        activity.contentResolver.openOutputStream(file.uri)?.use { it.write(content.toByteArray()) }
    }

    private fun ensureDir(root: DocumentFile, path: String): DocumentFile {
        var current = root
        for (part in path.split('/')) {
            if (part.isEmpty()) continue
            current = current.findFile(part)?.takeIf { it.isDirectory }
                ?: current.createDirectory(part)
                ?: throw Exception("Failed to create directory: $part")
        }
        return current
    }
}
