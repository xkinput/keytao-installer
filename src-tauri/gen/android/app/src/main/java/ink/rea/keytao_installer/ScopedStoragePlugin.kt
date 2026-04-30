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
import java.util.zip.ZipFile as JZipFile
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

                // Directory cache: avoids repeated findFile IPC calls for the same path
                val dirCache = mutableMapOf<String, DocumentFile>()
                fun getOrCreateDir(path: String): DocumentFile {
                    if (path.isEmpty()) return root
                    dirCache[path]?.let { return it }
                    val parts = path.split('/')
                    var current = root
                    val sb = StringBuilder()
                    for (part in parts) {
                        if (part.isEmpty()) continue
                        if (sb.isNotEmpty()) sb.append('/')
                        sb.append(part)
                        val key = sb.toString()
                        current = dirCache[key] ?: run {
                            val dir = current.findFile(part)?.takeIf { it.isDirectory }
                                ?: current.createDirectory(part)
                                ?: throw Exception("Failed to create directory: $part")
                            dirCache[key] = dir
                            dir
                        }
                    }
                    return current
                }

                fun writeToDir(dir: DocumentFile, filename: String, content: ByteArray, relative: String, merged: Boolean = false) {
                    dir.findFile(filename)?.delete()
                    val file = dir.createFile("application/octet-stream", filename)
                        ?: throw Exception("Failed to create: $filename")
                    val ok = activity.contentResolver.openOutputStream(file.uri)?.use { it.write(content); true } ?: false
                    if (!ok) throw Exception("Failed to open output stream: $filename")
                    logs.add(if (merged) "[MERGED] $relative" else "[OK] $relative")
                }

                JZipFile(zipFile).use { zip ->
                    // Enumerate all entries once (metadata only, no data read)
                    val allEntries = zip.entries().toList()

                    // Collect merge candidates and lua filenames from metadata
                    val zipLuaFilenames = mutableSetOf<String>()
                    var dcEntry: java.util.zip.ZipEntry? = null
                    var rimeLuaEntry: java.util.zip.ZipEntry? = null
                    for (entry in allEntries) {
                        val relative = entry.name.trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        when {
                            !entry.isDirectory && isDefaultCustom(filename) && dcEntry == null -> dcEntry = entry
                            !entry.isDirectory && filename == "rime.lua" && !relative.contains('/') && rimeLuaEntry == null -> rimeLuaEntry = entry
                            !entry.isDirectory && relative.startsWith("lua/") && !relative.substring(4).contains('/') -> zipLuaFilenames.add(filename)
                        }
                    }

                    // Read merge candidates directly by random access
                    val zipDcContent = dcEntry?.let { zip.getInputStream(it).bufferedReader().readText() }
                    val zipRimeLuaContent = rimeLuaEntry?.let { zip.getInputStream(it).bufferedReader().readText() }

                    val dcMergeResult = zipDcContent?.let {
                        val existing = readFileFromTree(root, "default.custom.yaml")
                            ?: readFileFromTree(root, "default-custom.yaml")
                        mergeDefaultCustom(existing, it)
                    }

                    val rimeLuaMergeResult = zipRimeLuaContent?.let { zipRl ->
                        val localRl = readFileFromTree(root, "rime.lua")
                        if (localRl != null) mergeRimeLua(localRl, zipRl, zipLuaFilenames)
                        else RimeLuaMergeResult(zipRl, emptyList())
                    }

                    // Save conflicting user lua files before they get overwritten
                    val renamedLuaFiles: List<Pair<String, ByteArray>> =
                        rimeLuaMergeResult?.renames?.mapNotNull { (oldName, newName) ->
                            val luaDir = root.findFile("lua") ?: return@mapNotNull null
                            val bytes = luaDir.findFile("$oldName.lua")?.uri?.let { fileUri ->
                                activity.contentResolver.openInputStream(fileUri)?.use { it.readBytes() }
                            } ?: return@mapNotNull null
                            newName to bytes
                        } ?: emptyList()

                    // Single extraction pass using cached dirs
                    for (entry in allEntries) {
                        val relative = entry.name.trimEnd('/')
                        if (relative.isEmpty()) continue
                        val filename = relative.substringAfterLast('/')
                        val dirPart = relative.substringBeforeLast('/', "")

                        when {
                            entry.isDirectory -> {
                                try { getOrCreateDir(relative) } catch (e: Exception) {
                                    logs.add("[WARN] mkdir $relative: ${e.message}")
                                }
                            }
                            isDefaultCustom(filename) && dcMergeResult != null -> {
                                val dir = getOrCreateDir(dirPart)
                                try { writeToDir(dir, filename, dcMergeResult.mergedContent.toByteArray(), relative, merged = true) }
                                catch (e: Exception) { return@Thread invoke.reject(e.message ?: "Write failed: $relative") }
                            }
                            filename == "rime.lua" && !relative.contains('/') && rimeLuaMergeResult != null -> {
                                val dir = getOrCreateDir(dirPart)
                                try { writeToDir(dir, filename, rimeLuaMergeResult.mergedContent.toByteArray(), relative, merged = true) }
                                catch (e: Exception) { return@Thread invoke.reject(e.message ?: "Write failed: $relative") }
                            }
                            else -> {
                                val dir = try { getOrCreateDir(dirPart) } catch (e: Exception) {
                                    logs.add("[ERROR] mkdir for $relative: ${e.message}")
                                    return@Thread invoke.reject("Failed to create directory for: $relative")
                                }
                                dir.findFile(filename)?.delete()
                                val newFile = dir.createFile("application/octet-stream", filename)
                                    ?: run { logs.add("[ERROR] createFile: $relative"); return@Thread invoke.reject("Failed to create: $filename") }
                                val ok = activity.contentResolver.openOutputStream(newFile.uri)?.use { out ->
                                    zip.getInputStream(entry).copyTo(out); true
                                } ?: false
                                if (ok) logs.add("[OK] $relative")
                                else { logs.add("[ERROR] openOutputStream: $relative"); return@Thread invoke.reject("Failed to open output stream: $relative") }
                            }
                        }
                    }

                    // Write renamed user lua files
                    if (renamedLuaFiles.isNotEmpty()) {
                        val luaDir = try { getOrCreateDir("lua") } catch (e: Exception) {
                            return@Thread invoke.reject("Failed to ensure lua dir: ${e.message}")
                        }
                        for ((newName, bytes) in renamedLuaFiles) {
                            val fname = "$newName.lua"
                            luaDir.findFile(fname)?.delete()
                            val newFile = luaDir.createFile("application/octet-stream", fname)
                            if (newFile == null) logs.add("[ERROR] createFile for renamed: lua/$fname")
                            else {
                                val ok = activity.contentResolver.openOutputStream(newFile.uri)?.use { it.write(bytes); true } ?: false
                                if (ok) logs.add("[RENAMED] lua/$fname")
                                else logs.add("[ERROR] openOutputStream for renamed: lua/$fname")
                            }
                        }
                    }

                    // Verify
                    val verifyArray = JSONArray()
                    fun addVerify(path: String, ok: Boolean, note: String) {
                        verifyArray.put(JSObject().apply { put("path", path); put("ok", ok); put("note", note) })
                    }

                    dcMergeResult?.mergedContent?.let { expected ->
                        val actual = readFileFromTree(root, "default.custom.yaml")
                        if (actual == expected) addVerify("default.custom.yaml", true, "内容一致")
                        else if (actual != null) addVerify("default.custom.yaml", false, "内容与写入时不符，可能写入不完整")
                        else addVerify("default.custom.yaml", false, "文件不存在或无法读取")
                    }
                    rimeLuaMergeResult?.mergedContent?.let { expected ->
                        val actual = readFileFromTree(root, "rime.lua")
                        if (actual == expected) addVerify("rime.lua", true, "内容一致")
                        else if (actual != null) addVerify("rime.lua", false, "内容与写入时不符，可能写入不完整")
                        else addVerify("rime.lua", false, "文件不存在或无法读取")
                    }
                    // Spot-check key files using cached dirs (no additional ZIP pass)
                    for (entry in allEntries) {
                        if (entry.isDirectory) continue
                        val relative = entry.name.trimEnd('/')
                        val filename = relative.substringAfterLast('/')
                        val isKey = filename.endsWith(".schema.yaml")
                            || filename.endsWith(".dict.yaml")
                            || (filename.endsWith(".lua") && !relative.contains('/'))
                            || relative.startsWith("lua/")
                            || relative.startsWith("opencc/")
                        if (!isKey || isDefaultCustom(filename) || filename == "rime.lua") continue
                        val dirPart = relative.substringBeforeLast('/', "")
                        val dir = if (dirPart.isEmpty()) root else dirCache[dirPart]
                        val exists = dir?.findFile(filename) != null
                        if (exists) addVerify(relative, true, "文件存在")
                        else addVerify(relative, false, "文件不存在")
                    }

                    zipFile.delete()

                    val mergedArray = JSONArray()
                    dcMergeResult?.userSchemas?.forEach { mergedArray.put(it) }
                    val logsArray = JSONArray()
                    logs.forEach { logsArray.put(it) }

                    invoke.resolve(JSObject().apply {
                        put("mergedSchemas", mergedArray)
                        put("logs", logsArray)
                        put("verify", verifyArray)
                    })
                }
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
