package io.github.maxdac.marknotes.sync

import androidx.test.platform.app.InstrumentationRegistry

/** Android: the spec fixtures and the parity corpus are packaged as test assets (see build.gradle.kts). */
object FixtureSource {
    private val assets get() = InstrumentationRegistry.getInstrumentation().context.assets

    fun read(path: String): String = assets.open(path).bufferedReader().use { it.readText() }

    fun list(folder: String): List<String> = assets.list(folder)!!.sorted()

    fun exists(path: String): Boolean =
        runCatching { assets.open(path).close() }.isSuccess
}
