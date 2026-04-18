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
                                entry.isDirectory -> ensureDir(root, relative)
                                isDefaultCustom(filename) && dcMergeResult != null ->
                                    writeTextFile(root, relative, dcMergeResult.mergedContent)
                                filename == "rime.lua" && rimeLuaMergeResult != null ->
                                    writeTextFile(root, relative, rimeLuaMergeResult.mergedContent)
                                else -> {
                                    val dirPart = relative.substringBeforeLast('/', "")
                                    val dir = if (dirPart.isEmpty()) root else ensureDir(root, dirPart)
                                    dir.findFile(filename)?.delete()
                                    val newFile = dir.createFile("application/octet-stream", filename)
                                        ?: return@Thread invoke.reject("Failed to create: $filename")
                                    activity.contentResolver.openOutputStream(newFile.uri)?.use { out ->
                                        zis.copyTo(out)
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
                    val luaDir = ensureDir(root, "lua")
                    for ((newName, bytes) in renamedLuaFiles) {
                        val filename = "$newName.lua"
                        luaDir.findFile(filename)?.delete()
                        val newFile = luaDir.createFile("application/octet-stream", filename)
                        newFile?.let {
                            activity.contentResolver.openOutputStream(it.uri)?.use { out -> out.write(bytes) }
                        }
                    }
                }

                zipFile.delete()

                val mergedArray = JSONArray()
                dcMergeResult?.userSchemas?.forEach { mergedArray.put(it) }

                invoke.resolve(JSObject().apply { put("mergedSchemas", mergedArray) })
            } catch (ex: Exception) {
                invoke.reject(ex.message ?: "Extraction failed")
            }
        }.start()
    }

    // ─── Helpers ────────────────────────────────────────────────────────────────

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

    data class RimeLuaMergeResult(val mergedContent: String, val renames: List<Pair<String, String>>)

    private fun extractLuaRequire(line: String): String? {
        val pos = line.indexOf("require").takeIf { it >= 0 } ?: return null
        val after = line.substring(pos + 7).trimStart()
        if (!after.startsWith('(')) return null
        val inner = after.substring(1).trimStart()
        val quote = inner.firstOrNull() ?: return null
        if (quote != '"' && quote != '\'') return null
        val content = inner.substring(1)
        val end = content.indexOf(quote).takeIf { it >= 0 } ?: return null
        return content.substring(0, end)
    }

    private fun mergeRimeLua(
        localContent: String,
        zipContent: String,
        zipLuaFilenames: Set<String>
    ): RimeLuaMergeResult {
        val zipRequires = zipContent.lines()
            .map { it.trim() }
            .filter { it.isNotEmpty() && !it.startsWith("--") }
            .mapNotNull { extractLuaRequire(it) }
            .toSet()

        val renames = mutableListOf<Pair<String, String>>()
        val extraLines = mutableListOf<String>()

        for (line in localContent.lines()) {
            val t = line.trim()
            if (t.isEmpty() || t.startsWith("--")) continue
            val module = extractLuaRequire(t)
            when {
                module == null -> extraLines.add(line)
                module in zipRequires -> continue
                "$module.lua" in zipLuaFilenames -> {
                    val newName = "${module}_user"
                    val newLine = line
                        .replace("\"$module\"", "\"$newName\"")
                        .replace("'$module'", "'$newName'")
                    renames.add(module to newName)
                    extraLines.add(newLine)
                }
                else -> extraLines.add(line)
            }
        }

        val merged = buildString {
            append(zipContent)
            if (extraLines.isNotEmpty()) {
                if (!zipContent.endsWith('\n')) append('\n')
                extraLines.forEach { appendLine(it) }
            }
        }

        return RimeLuaMergeResult(merged, renames)
    }

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
