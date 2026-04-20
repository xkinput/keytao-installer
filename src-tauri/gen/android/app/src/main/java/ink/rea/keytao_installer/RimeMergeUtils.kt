package ink.rea.keytao_installer

data class MergeResult(val mergedContent: String, val userSchemas: List<String>)
data class RimeLuaMergeResult(val mergedContent: String, val renames: List<Pair<String, String>>)

fun isDefaultCustom(filename: String) =
    filename == "default.custom.yaml" || filename == "default-custom.yaml"

fun extractLuaRequire(line: String): String? {
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

fun parseSchemas(content: String): List<String> {
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

fun mergeRimeLua(
    localContent: String,
    zipContent: String,
    zipLuaFilenames: Set<String>
): RimeLuaMergeResult {
    val zipRequires = buildSet<String> {
        var inBlock = false
        for (t in zipContent.lines().map { it.trim() }) {
            when {
                inBlock -> if (t.contains("--]]")) inBlock = false
                t.startsWith("--[[") -> inBlock = true
                t.isNotEmpty() && !t.startsWith("--") -> extractLuaRequire(t)?.let { add(it) }
            }
        }
    }

    val renames = mutableListOf<Pair<String, String>>()
    val extraLines = mutableListOf<String>()
    var inBlockComment = false

    for (line in localContent.lines()) {
        val t = line.trim()
        when {
            inBlockComment -> {
                if (t.contains("--]]")) inBlockComment = false
            }
            t.startsWith("--[[") -> inBlockComment = true
            t.isEmpty() || t.startsWith("--") -> { /* skip */ }
            else -> {
                val module = extractLuaRequire(t)
                when {
                    module == null -> extraLines.add(line)
                    module in zipRequires -> { /* already in zip, skip */ }
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

fun mergeDefaultCustom(existing: String?, zipContent: String): MergeResult {
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
