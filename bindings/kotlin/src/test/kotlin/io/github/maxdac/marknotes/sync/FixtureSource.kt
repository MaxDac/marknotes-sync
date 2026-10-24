package io.github.maxdac.marknotes.sync

import java.io.File

/** JVM: reads the spec checkout (`marknotes.specDir`, set by build.gradle.kts) and the parity corpus. */
object FixtureSource {
    private val roots = listOfNotNull(System.getProperty("marknotes.specDir"), System.getProperty("marknotes.parityDir"))
        .map(::File)

    private fun file(path: String) = roots.map { File(it, path) }.firstOrNull { it.exists() }
        ?: error("$path not found under $roots")

    fun read(path: String): String = file(path).readText()

    fun list(folder: String): List<String> = file(folder).list()!!.sorted()

    fun exists(path: String): Boolean = roots.any { File(it, path).exists() }
}
