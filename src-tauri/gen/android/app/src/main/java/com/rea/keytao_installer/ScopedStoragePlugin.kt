package com.rea.keytao_installer

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

                // First pass: read default.custom.yaml from zip
                var zipDefaultCustomPath: String? = null
                var zipDefaultCustomContent: String? = null
                ZipInputStream(zipFile.inputStream().buffered()).use { zis ->
                    var entry = zis.nextEntry
                    while (entry != null) {
                        val relative = entry.name.substringAfter('/').trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        if (!entry.isDirectory && isDefaultCustom(filename)) {
                            zipDefaultCustomPath = relative
                            zipDefaultCustomContent = zis.bufferedReader().readText()
                            break
                        }
                        zis.closeEntry()
                        entry = zis.nextEntry
                    }
                }

                // Read existing default.custom.yaml from target dir
                val existingContent = readFileFromTree(root, "default.custom.yaml")
                    ?: readFileFromTree(root, "default-custom.yaml")

                // Compute merge
                val mergeResult = zipDefaultCustomContent?.let {
                    mergeDefaultCustom(existingContent, it)
                }

                // Second pass: smart extraction
                ZipInputStream(zipFile.inputStream().buffered()).use { zis ->
                    var entry = zis.nextEntry
                    while (entry != null) {
                        val relative = entry.name.substringAfter('/').trimEnd('/')

                        if (relative.isNotEmpty()) {
                            val filename = relative.substringAfterLast('/')

                            when {
                                entry.isDirectory -> {
                                    if (isKeytaoPath(relative)) ensureDir(root, relative)
                                }
                                isDefaultCustom(filename) && mergeResult != null -> {
                                    writeTextFile(root, relative, mergeResult.mergedContent)
                                    // don't read from zis — closeEntry below handles skip
                                }
                                isKeytaoPath(relative) -> {
                                    val dirPart = relative.substringBeforeLast('/', "")
                                    val dir = if (dirPart.isEmpty()) root else ensureDir(root, dirPart)
                                    dir.findFile(filename)?.delete()
                                    val newFile = dir.createFile("application/octet-stream", filename)
                                        ?: return@Thread invoke.reject("Failed to create: $filename")
                                    activity.contentResolver.openOutputStream(newFile.uri)?.use { out ->
                                        zis.copyTo(out)
                                    }
                                }
                                // else: skip
                            }
                        }

                        zis.closeEntry()
                        entry = zis.nextEntry
                    }
                }

                zipFile.delete()

                val mergedArray = JSONArray()
                mergeResult?.userSchemas?.forEach { mergedArray.put(it) }

                invoke.resolve(JSObject().apply { put("mergedSchemas", mergedArray) })
            } catch (ex: Exception) {
                invoke.reject(ex.message ?: "Extraction failed")
            }
        }.start()
    }

    // ─── Helpers ────────────────────────────────────────────────────────────────

    private fun isKeytaoPath(relativePath: String): Boolean {
        val top = relativePath.substringBefore('/')
        if (top == "opencc" || top == "lua") return true
        val filename = relativePath.substringAfterLast('/')
        return filename.startsWith("keytao")
    }

    private fun isDefaultCustom(filename: String) =
        filename == "default.custom.yaml" || filename == "default-custom.yaml"

    private fun readFileFromTree(root: DocumentFile, filename: String): String? {
        return try {
            root.findFile(filename)?.uri?.let { uri ->
                activity.contentResolver.openInputStream(uri)?.use { it.bufferedReader().readText() }
            }
        } catch (e: Exception) {
            null
        }
    }

    private fun writeTextFile(root: DocumentFile, relativePath: String, content: String) {
        val dirPart = relativePath.substringBeforeLast('/', "")
        val filename = relativePath.substringAfterLast('/')
        val dir = if (dirPart.isEmpty()) root else ensureDir(root, dirPart)
        dir.findFile(filename)?.delete()
        val file = dir.createFile("application/octet-stream", filename) ?: return
        activity.contentResolver.openOutputStream(file.uri)?.use { it.write(content.toByteArray()) }
    }

    private fun parseSchemas(content: String): List<String> {
        val schemas = mutableListOf<String>()
        var inList = false
        for (line in content.lines()) {
            val t = line.trim()
            when {
                t.contains("schema_list:") -> inList = true
                inList -> {
                    val m = Regex("^- schema:\\s*(\\S+)").find(t)
                    if (m != null) {
                        schemas.add(m.groupValues[1])
                    } else if (t.isNotEmpty() && !t.startsWith('#') && !t.startsWith('-')) {
                        inList = false
                    }
                }
            }
        }
        return schemas
    }

    data class MergeResult(val mergedContent: String, val userSchemas: List<String>)

    private fun mergeDefaultCustom(existing: String?, zipContent: String): MergeResult {
        val keytaoSchemas = parseSchemas(zipContent).filter { it.startsWith("keytao") }
        val userSchemas = existing?.let { c ->
            parseSchemas(c).filter { !it.startsWith("keytao") }
        } ?: emptyList()
        val allSchemas = userSchemas + keytaoSchemas

        val out = StringBuilder()
        var inList = false
        for (line in zipContent.lines()) {
            val t = line.trim()
            if (!inList) {
                out.appendLine(line)
                if (t.contains("schema_list:")) {
                    inList = true
                    allSchemas.forEach { out.appendLine("    - schema: $it") }
                }
            } else {
                if (Regex("^- schema:").containsMatchIn(t)) {
                    // skip original entries
                } else {
                    inList = false
                    out.appendLine(line)
                }
            }
        }

        return MergeResult(out.toString(), userSchemas)
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
