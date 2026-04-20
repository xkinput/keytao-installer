package ink.rea.keytao_installer

import org.junit.Assert.*
import org.junit.Test

class RimeMergeUtilsTest {

    // ── extractLuaRequire ─────────────────────────────────────────────────────

    @Test
    fun `extractLuaRequire handles double quotes`() {
        assertEquals("keytao_filter", extractLuaRequire("keytao_filter = require(\"keytao_filter\")"))
    }

    @Test
    fun `extractLuaRequire handles single quotes`() {
        assertEquals("bar", extractLuaRequire("foo = require('bar')"))
    }

    @Test
    fun `extractLuaRequire returns null for non-require line`() {
        assertNull(extractLuaRequire("local x = 1"))
    }

    // ── parseSchemas ──────────────────────────────────────────────────────────

    @Test
    fun `parseSchemas extracts schema list`() {
        val content = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n"
        assertEquals(listOf("keytao_b", "keytao_bg"), parseSchemas(content))
    }

    @Test
    fun `parseSchemas stops at non-schema line`() {
        val content = "patch:\n  schema_list:\n    - schema: foo\n  other_key: val\n"
        assertEquals(listOf("foo"), parseSchemas(content))
    }

    // ── mergeRimeLua ──────────────────────────────────────────────────────────

    @Test
    fun `mergeRimeLua appends unique local require`() {
        val local = "my_mod = require(\"my_mod\")\n"
        val zip   = "keytao_filter = require(\"keytao_filter\")\n"
        val result = mergeRimeLua(local, zip, emptySet())
        assertTrue(result.mergedContent.contains("require(\"keytao_filter\")"))
        assertTrue(result.mergedContent.contains("require(\"my_mod\")"))
        assertTrue(result.renames.isEmpty())
    }

    @Test
    fun `mergeRimeLua skips require already present in zip`() {
        val local = "keytao_filter = require(\"keytao_filter\")\n"
        val zip   = "keytao_filter = require(\"keytao_filter\")\n"
        val result = mergeRimeLua(local, zip, emptySet())
        assertEquals(1, result.mergedContent.split("require(\"keytao_filter\")").size - 1)
    }

    @Test
    fun `mergeRimeLua renames conflicting module`() {
        val local = "my_mod = require(\"my_mod\")\n"
        val zip   = "keytao = require(\"keytao\")\n"
        val result = mergeRimeLua(local, zip, setOf("my_mod.lua"))
        assertEquals(listOf("my_mod" to "my_mod_user"), result.renames)
        assertTrue(result.mergedContent.contains("require(\"my_mod_user\")"))
        assertFalse(result.mergedContent.contains("require(\"my_mod\")"))
    }

    @Test
    fun `mergeRimeLua ignores block comment content`() {
        // Reproduces the Android bug: block comment lines such as ``` and
        // engine: were appended verbatim because --[[ ... --]] was not tracked.
        val local = """
            --[[
            librime-lua 样例
            ```
              engine:
                translators:
            ```
            --]]
            --[[
            各例可使用 `require` 引入。
            ```
              foo = require("bar")
            ```
            --]]
            my_mod = require("my_mod")
        """.trimIndent()
        val zip = "keytao_filter = require(\"keytao_filter\")\n"
        val result = mergeRimeLua(local, zip, emptySet())
        assertFalse("block comment line leaked", result.mergedContent.contains("librime-lua"))
        assertFalse("block comment line leaked", result.mergedContent.contains("engine:"))
        assertFalse("backticks leaked", result.mergedContent.contains("```"))
        assertFalse("in-comment require leaked", result.mergedContent.contains("require(\"bar\")"))
        assertTrue(result.mergedContent.contains("require(\"my_mod\")"))
        assertTrue(result.renames.isEmpty())
    }

    @Test
    fun `mergeRimeLua zip block comment require not treated as zip module`() {
        // A require() inside the zip's --[[ ]] block should not prevent
        // the same-named local require from being appended.
        val local = "bar = require(\"bar\")\n"
        val zip   = "--[[\n  foo = require(\"bar\")\n--]]\nkeytao = require(\"keytao\")\n"
        val result = mergeRimeLua(local, zip, emptySet())
        assertTrue(result.mergedContent.contains("require(\"bar\")"))
    }

    // ── mergeDefaultCustom ────────────────────────────────────────────────────

    @Test
    fun `mergeDefaultCustom preserves user schemas and adds keytao schemas`() {
        val existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: another\n"
        val zip      = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n"
        val result = mergeDefaultCustom(existing, zip)
        assertTrue(result.mergedContent.contains("- schema: my_schema"))
        assertTrue(result.mergedContent.contains("- schema: another"))
        assertTrue(result.mergedContent.contains("- schema: keytao_b"))
        assertEquals(listOf("my_schema", "another"), result.userSchemas)
    }

    @Test
    fun `mergeDefaultCustom excludes existing keytao schemas from user list`() {
        val existing = "patch:\n  schema_list:\n    - schema: my_schema\n    - schema: keytao_b\n"
        val zip      = "patch:\n  schema_list:\n    - schema: keytao_b\n    - schema: keytao_bg\n"
        val result = mergeDefaultCustom(existing, zip)
        assertEquals(listOf("my_schema"), result.userSchemas)
        assertTrue(result.mergedContent.contains("- schema: keytao_b"))
        assertTrue(result.mergedContent.contains("- schema: keytao_bg"))
    }

    @Test
    fun `mergeDefaultCustom handles null existing`() {
        val zip = "patch:\n  schema_list:\n    - schema: keytao_b\n"
        val result = mergeDefaultCustom(null, zip)
        assertTrue(result.userSchemas.isEmpty())
        assertTrue(result.mergedContent.contains("- schema: keytao_b"))
    }

    @Test
    fun `mergeDefaultCustom user schemas appear before keytao schemas`() {
        val existing = "patch:\n  schema_list:\n    - schema: user_schema\n"
        val zip      = "patch:\n  schema_list:\n    - schema: keytao_b\n"
        val result = mergeDefaultCustom(existing, zip)
        val userPos   = result.mergedContent.indexOf("user_schema")
        val keytaoPos = result.mergedContent.indexOf("keytao_b")
        assertTrue("user schema must precede keytao schema", userPos < keytaoPos)
    }

    // ── real keytao rime.lua ──────────────────────────────────────────────────

    private val keytaoRimeLua = """
        --[[
        librime-lua 样例
        ```
          engine:
            translators:
              - lua_translator@lua_function3
              - lua_translator@lua_function4
            filters:
              - lua_filter@lua_function1
              - lua_filter@lua_function2
        ```
        其中各 `lua_function` 为在本文件所定义变量名。
        --]]

        --[[
        本文件的后面是若干个例子，按照由简单到复杂的顺序示例了 librime-lua 的用法。
        每个例子都被组织在 `lua` 目录下的单独文件中，打开对应文件可看到实现和注解。

        各例可使用 `require` 引入。
        ```
          foo = require("bar")
        ```
        可认为是载入 `lua/bar.lua` 中的例子，并起名为 `foo`。
        配方文件中的引用方法为：`...@foo`。
        --]]

        date_time_translator = require("date_time")

        -- single_char_filter = require("single_char")

        -- keytao_filter: 单字模式 & 630 即 ss 词组提示
        -- 详见 `lua/keytao_filter.lua`
        keytao_filter = require("keytao_filter")

        -- 顶功处理器
        topup_processor = require("for_topup")

        -- 声笔笔简码提示 | 顶功提示 | 补全处理
        hint_filter = require("for_hint")

        -- number_translator: 将 `=` + 阿拉伯数字 翻译为大小写汉字
        number_translator = require("xnumber")

        -- 用 ' 作为次选键
        smart_2 = require("smart_2")
    """.trimIndent()

    @Test
    fun `parseSchemas on keytao rime lua excludes in-comment require`() {
        // The second --[[ block contains `foo = require("bar")` — must not appear.
        // (parseSchemas is for yaml, but mergeRimeLua uses similar require parsing)
        val result = mergeRimeLua(keytaoRimeLua, "keytao_filter = require(\"keytao_filter\")\n", emptySet())
        assertFalse("in-comment require leaked", result.mergedContent.contains("require(\"bar\")"))
    }

    @Test
    fun `mergeRimeLua reinstall produces no duplicates`() {
        val result = mergeRimeLua(keytaoRimeLua, keytaoRimeLua, emptySet())
        assertTrue(result.renames.isEmpty())
        for (module in listOf("date_time", "keytao_filter", "for_topup", "for_hint", "xnumber", "smart_2")) {
            val count = result.mergedContent.split("require(\"$module\")").size - 1
            assertEquals("require(\"$module\") duplicated after reinstall", 1, count)
        }
    }

    @Test
    fun `mergeRimeLua appends user extra module to keytao rime lua`() {
        val local = keytaoRimeLua + "\nmy_custom = require(\"my_custom\")\n"
        val result = mergeRimeLua(local, keytaoRimeLua, emptySet())
        assertTrue(result.renames.isEmpty())
        assertTrue(result.mergedContent.contains("require(\"my_custom\")"))
        assertEquals(1, result.mergedContent.split("require(\"keytao_filter\")").size - 1)
    }

    // ── zip overwrites local keytao content ──────────────────────────────────

    @Test
    fun `mergeRimeLua zip content is base local keytao requires not duplicated`() {
        // Local already has the same keytao rime.lua; merged result must equal zip content
        // (no keytao requires appended again, since they are all in zipRequires).
        val result = mergeRimeLua(keytaoRimeLua, keytaoRimeLua, emptySet())
        assertEquals(keytaoRimeLua, result.mergedContent)
        assertTrue(result.renames.isEmpty())
    }

    @Test
    fun `mergeRimeLua old local keytao missing module zip new module appears exactly once`() {
        // Local = older keytao rime.lua that lacks smart_2.
        // Zip = new keytao rime.lua that includes smart_2.
        // After merge the zip-provided smart_2 must appear exactly once.
        val oldLocal = keytaoRimeLua.lines()
            .filterNot { it.trimStart().startsWith("smart_2") }
            .joinToString("\n")
        val result = mergeRimeLua(oldLocal, keytaoRimeLua, emptySet())
        assertEquals(1, result.mergedContent.split("require(\"smart_2\")").size - 1)
        assertTrue(result.renames.isEmpty())
    }

    @Test
    fun `mergeRimeLua user extra preserved zip overwrites keytao no duplicates`() {
        // Local = keytao rime.lua + one user-defined module.
        // Zip = same keytao rime.lua (e.g. re-install or upgrade).
        // Expected: merged starts with zip content; user module appended once;
        //           every keytao module appears exactly once.
        val localWithExtra = keytaoRimeLua + "\nuser_plugin = require(\"user_plugin\")\n"
        val result = mergeRimeLua(localWithExtra, keytaoRimeLua, emptySet())
        assertTrue(result.mergedContent.startsWith(keytaoRimeLua))
        assertTrue(result.mergedContent.contains("require(\"user_plugin\")"))
        for (m in listOf("date_time", "keytao_filter", "for_topup", "for_hint", "xnumber", "smart_2")) {
            assertEquals(
                "require(\"$m\") must appear exactly once",
                1,
                result.mergedContent.split("require(\"$m\")").size - 1
            )
        }
        assertTrue(result.renames.isEmpty())
    }

    @Test
    fun `mergeRimeLua no block comment content leaked when local is keytao rime lua`() {
        val zip = "keytao_filter = require(\"keytao_filter\")\n"
        val result = mergeRimeLua(keytaoRimeLua, zip, emptySet())
        assertFalse("block comment header leaked", result.mergedContent.contains("librime-lua"))
        assertFalse("block comment content leaked", result.mergedContent.contains("engine:"))
        assertFalse("backticks leaked", result.mergedContent.contains("```"))
        assertFalse("in-comment require leaked", result.mergedContent.contains("require(\"bar\")"))
    }
}
